//! Minimal MCP JSON-RPC server over stdio for repotracer.
//! NEVER write non-protocol text to stdout.

mod conversations;
mod evidence;
use evidence::evidence_excerpts;
mod transport;

use repotracer_core::{
    validate_request, ScoutBackend, ScoutBackendError, ScoutRequest, ScoutResult, ValidatedCitation,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "repotracer";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_SCOUT_DESC: &str = "Delegate an investigation to a separately configured model. Supply the task and relevant context you already know; do not search first to prepare the request. The investigator follows useful leads and uses available tools or experiments. Set repository to the current target when different from the startup directory. Reuse conversation.id when prior context helps, including related work in another repository; independent calls can run in parallel. The answer includes selected source and experimental evidence. structuredContent is the machine-readable answer; content[].text is a readable alternative. Use either representation, not both.";

#[derive(Clone)]
pub struct McpServer {
    scout: Arc<dyn ScoutBackend>,
    root: PathBuf,
    conversations: Arc<conversations::Conversations>,
}

impl McpServer {
    pub fn new(scout: Arc<dyn ScoutBackend>, root: PathBuf) -> Self {
        Self {
            scout,
            root,
            conversations: Arc::new(conversations::Conversations::default()),
        }
    }

    /// Serve MCP over stdin/stdout until EOF.
    pub async fn serve_stdio(&self) -> anyhow::Result<()> {
        let server = self.clone();
        transport::serve(tokio::io::stdin(), tokio::io::stdout(), move |message| {
            let server = server.clone();
            async move { server.handle_message(message).await }
        })
        .await
    }

    async fn handle_message(&self, msg: Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));

        // Notifications have no id — no response.
        let is_notification = id.is_none() || id.as_ref().is_some_and(|v| v.is_null());

        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {},
                    "prompts": {}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION
                }
            })),
            "notifications/initialized" | "initialized" => {
                return None;
            }
            "ping" => Ok(json!({})),
            "tools/list" => {
                let mut tool = repo_scout_tool_def();
                let root = self
                    .root
                    .canonicalize()
                    .unwrap_or_else(|_| self.root.clone());
                tool["description"] = json!(format!(
                    "{REPO_SCOUT_DESC} Server startup repository: {}.",
                    json!(root)
                ));
                Ok(json!({"tools": [tool]}))
            }
            "tools/call" => self.tools_call(params).await,
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [repo_scout_prompt_def()] })),
            "prompts/get" => repo_scout_prompt(params),
            "" if msg.get("result").is_some() || msg.get("error").is_some() => {
                return None;
            }
            other => Err(rpc_error(-32601, format!("Method not found: {other}"))),
        };

        if is_notification {
            return None;
        }

        Some(match result {
            Ok(r) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": r
            }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": e
            }),
        })
    }

    async fn tools_call(&self, params: Value) -> Result<Value, Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name != "repo_scout" {
            return Err(rpc_error(-32602, format!("Unknown tool: {name}")));
        }

        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if query.is_empty() {
            return Ok(tool_text("Error: `query` is required.", true));
        }

        let focus = args
            .get("focus")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);

        let explicit_root = match args.get("repository") {
            Some(Value::String(path)) => Some(path.as_str()),
            None => None,
            _ => {
                return Err(rpc_error(
                    -32602,
                    "repository must be a directory path string".into(),
                ))
            }
        };
        let mut investigation: repotracer_core::InvestigationSpec =
            serde_json::from_value(args.get("investigation").cloned().unwrap_or(json!({})))
                .map_err(|e| rpc_error(-32602, format!(
                    "Invalid investigation: {e}. Supply a JSON object, for example \"investigation\": {{\"intent\": \"diagnose\", \"reasoning_effort\": \"high\"}}, or omit investigation and use query alone."
                )))?;
        let id = investigation
            .conversation_id
            .clone()
            .unwrap_or_else(|| format!("rt-{}", uuid::Uuid::new_v4()));
        // Reserve FIFO order before repository selection can yield. Related
        // requests also read remembered roots only after the prior turn binds them.
        let _conversation_turn = match self.conversations.enter(&id).await {
            Ok(guard) => guard,
            Err(error) => return Ok(tool_text(&format!("Error: {error}"), true)),
        };
        let remembered = self.conversations.root(&id);
        if investigation.conversation_id.is_some()
            && id.starts_with("rt-")
            && remembered.is_none()
            && explicit_root.is_none()
        {
            return Ok(tool_text("Error: conversation handle is no longer known. Supply repository and necessary context to start fresh, or omit conversation_id for a new investigation.", true));
        }
        let default = self.root.clone();
        let explicit_root = explicit_root.map(str::to_owned);
        let repository_focus = focus.clone();
        let selection = tokio::task::spawn_blocking(move || {
            conversations::select_repository(
                &default,
                explicit_root.as_deref(),
                repository_focus.as_deref(),
                remembered.as_deref(),
            )
        })
        .await
        .map_err(|error| rpc_error(-32603, format!("Repository selection failed: {error}")))?;
        let root = match selection {
            Ok(root) => root,
            Err(error) => return Ok(tool_text(&format!("Error: {error}"), true)),
        };
        investigation.conversation_id = Some(id.clone());

        let mut request = ScoutRequest {
            investigation,
            query,
            root: root.clone(),
            focus,
            max_turns: None,
            timeout: None,
        };
        if let Err(error) = request.normalize_paths() {
            return Ok(tool_text(&format!("Error: {error}"), true));
        }
        if let Err(error) = validate_request(&request) {
            return Ok(tool_text(&format!("Error: {error}"), true));
        }

        if let Err(error) = self.conversations.bind(&id, &root) {
            return Ok(tool_text(&format!("Error: {error}"), true));
        }

        let mut result = match self.scout.scout(request).await {
            Ok(result) => result,
            Err(error) => {
                if let Some(error) = error.downcast_ref::<ScoutBackendError>() {
                    let mut error = error.clone();
                    set_conversation(&mut error.stats, &id, &root);
                    return Ok(terminal_failure_response(&error));
                }
                let mut error_response = rpc_error(-32000, error.to_string());
                error_response["data"] =
                    json!({"conversation": {"id": id, "repository": root, "status": "unknown"}});
                return Err(error_response);
            }
        };

        set_conversation(&mut result.stats, &id, &root);
        tokio::task::spawn_blocking(move || handoff_response(&root, result))
            .await
            .map_err(|error| rpc_error(-32603, format!("Source handoff failed: {error}")))
    }
}

fn set_conversation(stats: &mut repotracer_core::ScoutStats, id: &str, root: &Path) {
    // An adaptive continuation can make the aggregate turn count > 1 even
    // though the parent's initial request started a fresh thread.
    let turn = stats
        .attempts
        .first()
        .map_or(stats.thread_turn, |attempt| attempt.thread_turn);
    stats.conversation = Some(repotracer_core::ConversationInfo {
        id: id.to_owned(),
        repository: root.display().to_string(),
        status: match turn {
            0 => "unknown",
            1 => "fresh",
            _ => "resumed",
        }
        .into(),
    });
}

fn terminal_failure_response(error: &ScoutBackendError) -> Value {
    let context = error
        .stats
        .conversation
        .as_ref()
        .map(|c| {
            format!(
                "\nRepository: {}\nConversation: {} ({})",
                c.repository, c.id, c.status
            )
        })
        .unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": format!("{error}{context}") }],
        "structuredContent": { "stats": error.stats, "conversation": error.stats.conversation },
        "isError": true
    })
}

#[derive(Debug, Clone)]
struct EvidenceSpan {
    path: String,
    start_line: u32,
    end_line: u32,
    text: String,
    truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
struct EvidenceAttachmentError {
    path: String,
    start_line: u32,
    end_line: u32,
    message: String,
}

#[derive(Debug, Default)]
struct EvidenceBundle {
    spans: Vec<EvidenceSpan>,
    omitted_citations: usize,
    omitted_spans: usize,
    errors: Vec<EvidenceAttachmentError>,
}

#[derive(Debug, Default)]
struct HandoffOmissions {
    omitted_source_citations: usize,
    omitted_source_spans: usize,
    truncated_spans: usize,
    errors: Vec<EvidenceAttachmentError>,
}

fn handoff_response(root: &Path, mut result: ScoutResult) -> Value {
    result.citations = unique_citations(result.citations);
    let bundle = evidence_excerpts(root, &result.citations);
    let omissions = HandoffOmissions {
        omitted_source_citations: bundle.omitted_citations,
        omitted_source_spans: bundle.omitted_spans,
        truncated_spans: bundle.spans.iter().filter(|span| span.truncated).count(),
        errors: bundle.errors,
    };
    build_handoff_response(&result, &bundle.spans, &omissions)
}

fn unique_citations(citations: Vec<ValidatedCitation>) -> Vec<ValidatedCitation> {
    let mut unique = Vec::with_capacity(citations.len());
    for citation in citations {
        if !unique.iter().any(|kept: &ValidatedCitation| {
            kept.path == citation.path
                && kept.start_line == citation.start_line
                && kept.end_line == citation.end_line
        }) {
            unique.push(citation);
        }
    }
    unique
}

fn build_handoff_response(
    result: &ScoutResult,
    spans: &[EvidenceSpan],
    omissions: &HandoffOmissions,
) -> Value {
    let report = handoff_explanation(result);
    let mut text = report.clone();
    let evidence = append_evidence(&mut text, spans, &omissions.errors);
    let citations = source_delivery_citations(&result.citations, spans, &omissions.errors);
    if let Some(conversation) = &result.stats.conversation {
        text.push_str(&format!(
            "\n\nRepository: {}\nConversation: {} ({})",
            conversation.repository, conversation.id, conversation.status
        ));
    }
    let structured = json!({
        "handoff_version": 3,
        "repository": root_from_stats(&result.stats),
        "conversation": result.stats.conversation,
        "report": report,
        "citations": citations,
        "continuation": result.investigation.continuation,
        "evidence": evidence,
        "evidence_omissions": {
            "omitted_citations": omissions.omitted_source_citations,
            "omitted_spans": omissions.omitted_source_spans,
            "truncated_spans": omissions.truncated_spans,
            "explicit": omissions.omitted_source_spans > 0
                || omissions.truncated_spans > 0
                || !omissions.errors.is_empty(),
            "errors": omissions.errors,
        },
        "stats": result.stats,
    });

    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

fn root_from_stats(stats: &repotracer_core::ScoutStats) -> Option<&str> {
    stats.conversation.as_ref().map(|c| c.repository.as_str())
}

/// Delivery describes the actual source text, not the truth of a finding.
/// Keep original citation fields for existing clients; status is additive.
fn source_delivery_citations(
    citations: &[ValidatedCitation],
    spans: &[EvidenceSpan],
    errors: &[EvidenceAttachmentError],
) -> Vec<Value> {
    citations
        .iter()
        .map(|citation| {
            let status = spans
                .iter()
                .find(|span| {
                    span.path == citation.path
                        && span.start_line <= citation.start_line
                        && span.end_line >= citation.end_line
                })
                .map_or("omitted", |span| {
                    if span.truncated {
                        "truncated"
                    } else {
                        "included"
                    }
                });
            let mut value = serde_json::to_value(citation).expect("citation is serializable");
            value["source_status"] = json!(status);
            if let Some(error) = errors.iter().find(|error| {
                error.path == citation.path
                    && error.start_line == citation.start_line
                    && error.end_line == citation.end_line
            }) {
                value["source_error"] = json!(error.message);
            }
            value
        })
        .collect()
}

fn handoff_explanation(result: &ScoutResult) -> String {
    let mut out = result.summary.clone();
    if !result.investigation.findings.is_empty() {
        out.push_str("\n\nFindings:\n");
        for finding in &result.investigation.findings {
            out.push_str(&format!(
                "\nQuestion: {}\nExplanation: {}\n",
                finding.question, finding.answer
            ));
            if !finding.citations.is_empty() {
                out.push_str("Sources: ");
                for (index, citation) in finding.citations.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format!(
                        "{}:{}-{}",
                        citation.path, citation.start_line, citation.end_line
                    ));
                }
                out.push('\n');
            }
        }
    }
    if !result.investigation.searched_scope.is_empty() {
        out.push_str("\nSearched scope:\n");
        for scope in &result.investigation.searched_scope {
            out.push_str(&format!("- {scope}\n"));
        }
    }
    if !result.investigation.unresolved.is_empty() {
        out.push_str("\nUnresolved questions:\n");
        for question in &result.investigation.unresolved {
            out.push_str(&format!("- {question}\n"));
        }
    }
    if !result.investigation.limitations.is_empty() {
        out.push_str("\nInvestigation limitations:\n");
        for limitation in &result.investigation.limitations {
            out.push_str(&format!("- {limitation}\n"));
        }
    }
    out
}

fn append_evidence(
    out: &mut String,
    spans: &[EvidenceSpan],
    errors: &[EvidenceAttachmentError],
) -> Vec<Value> {
    let mut evidence = Vec::new();
    if !spans.is_empty() {
        out.push_str("\n\nSource context (validated repository spans):");
        for span in spans {
            out.push_str(&format!(
                "\n\n--- {}:{}-{} ---\n",
                span.path, span.start_line, span.end_line
            ));
            out.push_str(&span.text);
            evidence.push(json!({
                "path": span.path, "start_line": span.start_line, "end_line": span.end_line,
                "text": span.text, "truncated": span.truncated,
            }));
        }
    }
    if !errors.is_empty() {
        out.push_str("\n\nSource attachment issues:");
        for error in errors {
            out.push_str(&format!(
                "\n- {}:{}-{}: {}",
                error.path, error.start_line, error.end_line, error.message
            ));
        }
    }
    evidence
}

fn repo_scout_tool_def() -> Value {
    json!({
        "name": "repo_scout",
        "description": REPO_SCOUT_DESC,
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": true
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural-language repository question. For changes, include relevant user requirements and compatibility constraints; the scout does not automatically receive the parent conversation. Separate requested behavior from assumptions about existing code."
                },
                "repository": {
                    "type": "string",
                    "description": "Current target directory, including another checkout or worktree. Prefer an absolute path; relative paths resolve from the server startup directory. Omit to reuse the conversation's latest target or the startup directory. Related evidence may be investigated elsewhere."
                },
                "focus": {
                    "type": "string",
                    "description": "Optional starting file or directory, relative to the current target or absolute. Related paths outside the target are allowed. Without a repository or remembered conversation, an absolute focus in another Git checkout can select that checkout."
                },
                "investigation": {
                    "type": "object", "additionalProperties": false,
                    "description": "Optional JSON object of investigation hints, for example {\"intent\":\"diagnose\",\"reasoning_effort\":\"high\"}. Query alone can request any repository investigation.",
                    "properties": {
                        "reasoning_effort": {"type":"string", "enum":["low","medium","high","xhigh","max"], "description":"Native subscription effort for this investigation only. Medium suits straightforward lookups; high suits diagnosis, indirect relationships, or cross-component change impact. Omit to use configured effort. Supported levels depend on the selected provider and model."},
                        "intent": {"type":"string", "enum":["locate","explain","change_impact","diagnose","inventory"]},
                        "questions": {"type":"array", "maxItems":24, "items":{"type":"string"}},
                        "conversation_id": {"type":"string", "maxLength":128, "description":"Reuse a prior conversation when its context helps, including related assignments in another repository. Supply a new repository explicitly when changing target. Omit for independent work. Status reports resumed, fresh or unknown; include necessary context when history is unavailable."},
                        "known_context": {"type":"string", "description":"Context already known to the parent; unverified until checked."},
                        "target_paths": {"type":"array", "maxItems":32, "items":{"type":"string", "description":"Optional file or directory leads, relative to the current target or absolute for related locations. New relative paths may name proposed files. The investigator discovers other relevant paths itself."}}
                    }
                }
            },
            "required": ["query"]
        }
    })
}

fn repo_scout_prompt_def() -> Value {
    json!({
        "name": "repo_scout",
        "description": "Delegate a repository question or a failed targeted lookup to RepoTracer.",
        "arguments": [{
            "name": "query",
            "description": "Precise semantic repository question or flow to trace.",
            "required": true
        }]
    })
}

fn repo_scout_prompt(params: Value) -> Result<Value, Value> {
    if params.get("name").and_then(Value::as_str) != Some("repo_scout") {
        return Err(rpc_error(-32602, "Unknown prompt".into()));
    }
    let query = params
        .get("arguments")
        .and_then(|value| value.get("query"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if query.is_empty() {
        return Err(rpc_error(-32602, "`query` is required".into()));
    }
    Ok(json!({
        "description": "Explore the repository with RepoTracer, then use its explanation and embedded source context to inform the decision.",
        "messages": [{
            "role": "user",
            "content": {
                "type": "text",
                "text": format!("Call repo_scout with this question. Add context or an investigation intent when useful. Use its explanation and source context, and check any unresolved questions or limitations: {query}")
            }
        }]
    }))
}

fn tool_text(text: &str, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

fn rpc_error(code: i64, message: String) -> Value {
    json!({ "code": code, "message": message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn slow_git_selection_does_not_block_runtime() {
        use std::time::{Duration, Instant};
        const CHILD: &str = "REPOTRACER_SLOW_GIT_TEST";
        if std::env::var_os(CHILD).is_none() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let git = dir.path().join("git");
            std::fs::write(&git, "#!/bin/sh\nsleep 1\nprintf '%s\\n' \"$2\"\n").unwrap();
            std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::slow_git_selection_does_not_block_runtime",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        dir.path().display(),
                        std::env::var("PATH").unwrap_or_default()
                    ),
                )
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let startup = tempfile::tempdir().unwrap();
        let selected = tempfile::tempdir().unwrap();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let server = McpServer::new(
            Arc::new(CapturingScout {
                request: captured.clone(),
            }),
            startup.path().to_owned(),
        );
        let started = Instant::now();
        let (response, follow_up, elapsed) = tokio::join!(
            server.tools_call(json!({"name": "repo_scout", "arguments": {
                "query": "first", "focus": selected.path().canonicalize().unwrap(),
                "investigation": {"conversation_id": "same"}
            }})),
            server.tools_call(json!({"name": "repo_scout", "arguments": {
                "query": "follow-up", "repository": selected.path(),
                "investigation": {"conversation_id": "same"}
            }})),
            async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                started.elapsed()
            }
        );
        assert!(!response.unwrap()["isError"].as_bool().unwrap_or(false));
        assert!(!follow_up.unwrap()["isError"].as_bool().unwrap_or(false));
        assert_eq!(
            captured.lock().unwrap().as_ref().unwrap().query,
            "follow-up"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "Git blocked the runtime for {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn malformed_investigation_explains_retry_without_starting_scout() {
        let root = tempfile::tempdir().unwrap();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let server = McpServer::new(
            Arc::new(CapturingScout {
                request: captured.clone(),
            }),
            root.path().to_owned(),
        );
        let error = server
            .tools_call(json!({"name":"repo_scout", "arguments": {
                "query":"trace failures", "investigation":"<parameter name=\"intent\">diagnose"
            }}))
            .await
            .unwrap_err();
        assert_eq!(error["code"], -32602);
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("Supply a JSON object"));
        assert!(captured.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn selected_repository_and_returned_handle_follow_the_actual_checkout() {
        let startup = tempfile::tempdir().unwrap();
        let selected = tempfile::tempdir().unwrap();
        std::fs::write(startup.path().join("src.rs"), "WRONG CHECKOUT\n").unwrap();
        std::fs::write(selected.path().join("src.rs"), "SELECTED CHECKOUT\n").unwrap();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let server = McpServer::new(
            Arc::new(CapturingScout {
                request: captured.clone(),
            }),
            startup.path().to_owned(),
        );
        let first = server
            .tools_call(json!({"name":"repo_scout", "arguments": {
                "query":"trace selected", "repository":selected.path(), "focus":"src.rs"
            }}))
            .await
            .unwrap();
        let id = first["structuredContent"]["conversation"]["id"]
            .as_str()
            .unwrap();
        assert!(id.starts_with("rt-"));
        assert_eq!(
            first["structuredContent"]["repository"],
            selected
                .path()
                .canonicalize()
                .unwrap()
                .display()
                .to_string()
        );
        assert_eq!(
            first["structuredContent"]["conversation"]["status"],
            "unknown"
        );
        assert!(first["content"][0]["text"].as_str().unwrap().contains(id));
        let request = captured.lock().unwrap().clone().unwrap();
        assert_eq!(request.root, selected.path().canonicalize().unwrap());
        assert_eq!(request.investigation.conversation_id.as_deref(), Some(id));
        server
            .tools_call(json!({"name":"repo_scout", "arguments": {
                "query":"now trace its caller", "investigation":{"conversation_id":id}
            }}))
            .await
            .unwrap();
        assert_eq!(
            captured.lock().unwrap().as_ref().unwrap().root,
            request.root
        );
        let changed = server.tools_call(json!({"name":"repo_scout", "arguments": {
            "query":"different root", "repository":startup.path(), "investigation":{"conversation_id":id}
        }})).await.unwrap();
        assert_eq!(changed["isError"], false);
        assert_eq!(
            captured.lock().unwrap().as_ref().unwrap().root,
            startup.path().canonicalize().unwrap()
        );
        server
            .tools_call(json!({"name":"repo_scout", "arguments": {
                "query":"continue on the new target", "investigation":{"conversation_id":id}
            }}))
            .await
            .unwrap();
        assert_eq!(
            captured.lock().unwrap().as_ref().unwrap().root,
            startup.path().canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn explicit_root_evidence_is_read_from_selected_checkout_not_startup() {
        struct SourceScout;
        #[async_trait::async_trait]
        impl ScoutBackend for SourceScout {
            async fn scout(&self, _: ScoutRequest) -> anyhow::Result<ScoutResult> {
                let mut result = scout_result(0);
                result.citations.push(ValidatedCitation {
                    path: "src.rs".into(),
                    start_line: 1,
                    end_line: 1,
                    reason: None,
                });
                result.stats.thread_turn = 1;
                Ok(result)
            }
        }
        let startup = tempfile::tempdir().unwrap();
        let selected = tempfile::tempdir().unwrap();
        std::fs::write(startup.path().join("src.rs"), "WRONG\n").unwrap();
        std::fs::write(selected.path().join("src.rs"), "RIGHT\n").unwrap();
        let server = McpServer::new(Arc::new(SourceScout), startup.path().to_owned());
        let response = server
            .tools_call(json!({"name":"repo_scout","arguments":{
                "query":"read", "repository":selected.path()
            }}))
            .await
            .unwrap();
        assert_eq!(
            response["structuredContent"]["evidence"][0]["text"],
            "1: RIGHT"
        );
        assert_eq!(
            response["structuredContent"]["conversation"]["status"],
            "fresh"
        );
        let id = response["structuredContent"]["conversation"]["id"]
            .as_str()
            .unwrap();
        let moved = server
            .tools_call(json!({"name":"repo_scout","arguments":{
                "query":"read the other checkout", "repository":startup.path(),
                "investigation":{"conversation_id":id}
            }}))
            .await
            .unwrap();
        assert_eq!(
            moved["structuredContent"]["evidence"][0]["text"],
            "1: WRONG"
        );
        assert_eq!(moved["structuredContent"]["conversation"]["id"], id);
        assert_eq!(
            moved["structuredContent"]["repository"],
            startup
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        );
    }

    #[tokio::test]
    async fn unknown_generated_handle_requires_explicit_repository() {
        let root = tempfile::tempdir().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server = McpServer::new(
            Arc::new(CountingScout {
                calls: calls.clone(),
            }),
            root.path().to_owned(),
        );
        let response = server
            .tools_call(json!({"name":"repo_scout","arguments":{
                "query":"continue", "investigation":{"conversation_id":"rt-expired"}
            }}))
            .await
            .unwrap();
        assert_eq!(response["isError"], true);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn parent_resume_status_uses_first_attempt_not_adaptive_second_turn() {
        let mut stats = repotracer_core::ScoutStats {
            thread_turn: 2,
            attempts: vec![
                repotracer_core::ScoutAttemptStats {
                    thread_turn: 1,
                    ..Default::default()
                },
                repotracer_core::ScoutAttemptStats {
                    thread_turn: 2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        set_conversation(&mut stats, "test", Path::new("/repo"));
        assert_eq!(stats.conversation.as_ref().unwrap().status, "fresh");
        stats.attempts[0].thread_turn = 2;
        set_conversation(&mut stats, "test", Path::new("/repo"));
        assert_eq!(stats.conversation.as_ref().unwrap().status, "resumed");
    }

    fn embedded_text<'a>(_response: &Value, text: &'a Value) -> &'a str {
        text.as_str().unwrap()
    }

    #[test]
    fn standalone_structured_answer_preserves_unicode_and_repeated_source_text() {
        let root = tempfile::tempdir().unwrap();
        for path in ["α.rs", "β.rs"] {
            std::fs::write(root.path().join(path), "let 設定 = \"λ\";\n").unwrap();
        }
        let mut result = scout_result(0);
        result.summary = "Préface λ with repeated source: 1: let 設定 = \"λ\";".into();
        result.citations = ["α.rs", "β.rs"]
            .iter()
            .map(|path| ValidatedCitation {
                path: (*path).into(),
                start_line: 1,
                end_line: 1,
                reason: None,
            })
            .collect();
        let response = handoff_response(root.path(), result);
        let evidence = response["structuredContent"]["evidence"]
            .as_array()
            .unwrap();
        assert_eq!(evidence.len(), 2);
        for span in evidence {
            assert_eq!(
                embedded_text(&response, &span["text"]),
                "1: let 設定 = \"λ\";"
            );
        }
        assert_eq!(evidence[0]["text"], evidence[1]["text"]);
        assert!(
            embedded_text(&response, &response["structuredContent"]["report"])
                .contains("Préface λ")
        );
    }

    #[test]
    fn structured_and_text_reports_preserve_source_warnings_and_legacy_findings() {
        let root = tempfile::tempdir().unwrap();
        let mut result = scout_result(0);
        result.summary = "A reproduction confirms the empty value.".into();
        result.investigation.limitations =
            vec!["Source could not be attached: removed.rs:1-8.".into()];
        result.investigation.unresolved = vec!["Generated export behavior is unknown.".into()];
        let response = handoff_response(root.path(), result);
        let report = response["structuredContent"]["report"].as_str().unwrap();
        assert!(report.contains("A reproduction confirms"));
        assert!(report.contains("Source could not be attached: removed.rs:1-8."));
        assert!(report.contains("Generated export behavior is unknown."));
        assert!(response["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with(report));
        assert_eq!(response["isError"], false);
    }

    #[test]
    fn merged_ranges_retain_their_earliest_task_priority() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "one\ntwo\nthree\nfour\n").unwrap();
        let citations = [(3, 4), (1, 2)].map(|(start_line, end_line)| ValidatedCitation {
            path: "lib.rs".into(),
            start_line,
            end_line,
            reason: None,
        });
        let bundle = evidence_excerpts(root.path(), &citations);
        assert_eq!(bundle.spans.len(), 1);
        assert!(bundle.spans[0].text.contains("1: one"));
        assert!(bundle.spans[0].text.contains("4: four"));
    }

    #[test]
    fn repo_scout_prompt_requires_query_and_enforces_handoff() {
        let error =
            repo_scout_prompt(json!({ "name": "repo_scout", "arguments": {} })).unwrap_err();
        assert_eq!(error["code"], -32602);

        let result = repo_scout_prompt(json!({
            "name": "repo_scout",
            "arguments": { "query": "trace refresh-token rotation" }
        }))
        .unwrap();
        let text = result["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("trace refresh-token rotation"));
        assert!(text.contains("unresolved questions"));
        assert!(!text.contains("MUST answer immediately"));
        assert!(repo_scout_prompt_def()["description"]
            .as_str()
            .unwrap()
            .contains("repository question"));
        assert!(!repo_scout_prompt_def()["description"]
            .as_str()
            .unwrap()
            .contains("broad"));
        assert_eq!(repo_scout_prompt_def()["arguments"][0]["required"], true);
    }

    #[test]
    fn repo_scout_description_matches_routing_contract() {
        let tool = repo_scout_tool_def();
        let description = tool["description"].as_str().unwrap();
        assert!(description.contains("separately configured model"));
        assert!(description.contains("selected source"));
        assert!(description.contains("experimental evidence"));
        assert!(description.contains("Use either representation, not both"));
    }

    #[test]
    fn repo_scout_is_declared_read_only() {
        let annotations = &repo_scout_tool_def()["annotations"];
        assert_eq!(annotations["readOnlyHint"], false);
        assert_eq!(annotations["destructiveHint"], true);
        assert_eq!(annotations["openWorldHint"], true);
    }

    fn scout_result(citation_count: usize) -> ScoutResult {
        ScoutResult {
            investigation: Default::default(),
            summary: "Focused evidence".into(),
            citations: (0..citation_count)
                .map(|index| repotracer_core::ValidatedCitation {
                    path: format!("src/{index}.rs"),
                    start_line: 1,
                    end_line: 2,
                    reason: Some("relevant".into()),
                })
                .collect(),
            stats: repotracer_core::ScoutStats::default(),
            raw_final: None,
        }
    }

    struct CountingScout {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ScoutBackend for CountingScout {
        async fn scout(&self, _request: ScoutRequest) -> anyhow::Result<ScoutResult> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(scout_result(1))
        }
    }

    struct CapturingScout {
        request: Arc<std::sync::Mutex<Option<ScoutRequest>>>,
    }

    struct TerminalFailureScout;

    #[async_trait::async_trait]
    impl ScoutBackend for TerminalFailureScout {
        async fn scout(&self, _request: ScoutRequest) -> anyhow::Result<ScoutResult> {
            let stats = repotracer_core::ScoutStats {
                model: "fixture".into(),
                usage_status: repotracer_core::UsageStatus::Partial,
                reported_cost_usd: Some(1.25),
                usage: repotracer_core::UsageStats {
                    input_tokens: Some(100),
                    cached_input_tokens: Some(60),
                    cache_write_input_tokens: Some(10),
                    output_tokens: Some(19),
                    ..Default::default()
                },
                ..Default::default()
            };
            Err(anyhow::Error::new(ScoutBackendError::new(
                "Claude investigation failed: \"error_max_turns\" after 7 turns and 0 tool calls; reported usage (partial): {}",
                stats,
            )))
        }
    }

    struct OrdinaryFailureScout;

    #[async_trait::async_trait]
    impl ScoutBackend for OrdinaryFailureScout {
        async fn scout(&self, _request: ScoutRequest) -> anyhow::Result<ScoutResult> {
            Err(anyhow::anyhow!("ordinary backend failure"))
        }
    }

    fn valid_tool_call() -> Value {
        json!({
            "name": "repo_scout",
            "arguments": { "query": "trace" }
        })
    }

    #[tokio::test]
    async fn terminal_backend_failure_keeps_stats_in_mcp_tool_result() {
        let root = tempfile::tempdir().unwrap();
        let server = McpServer::new(Arc::new(TerminalFailureScout), root.path().to_path_buf());

        let response = server.tools_call(valid_tool_call()).await.unwrap();

        assert_eq!(response["isError"], true);
        assert_eq!(response["structuredContent"]["stats"]["model"], "fixture");
        assert_eq!(
            response["structuredContent"]["stats"]["reported_cost_usd"],
            1.25
        );
        assert_eq!(
            response["structuredContent"]["stats"]["usage_status"],
            "partial"
        );
        let usage = &response["structuredContent"]["stats"]["usage"];
        assert_eq!(usage["input_tokens"], 100);
        assert_eq!(usage["cached_input_tokens"], 60);
        assert_eq!(usage["cache_write_input_tokens"], 10);
        assert_eq!(usage["output_tokens"], 19);
        assert!(response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("error_max_turns"));
    }

    #[tokio::test]
    async fn ordinary_backend_failure_remains_json_rpc_error() {
        let root = tempfile::tempdir().unwrap();
        let server = McpServer::new(Arc::new(OrdinaryFailureScout), root.path().to_path_buf());

        let error = server.tools_call(valid_tool_call()).await.unwrap_err();

        assert_eq!(error["code"], -32000);
        assert_eq!(error["message"], "ordinary backend failure");
    }

    #[async_trait::async_trait]
    impl ScoutBackend for CapturingScout {
        async fn scout(&self, request: ScoutRequest) -> anyhow::Result<ScoutResult> {
            *self.request.lock().unwrap() = Some(request);
            Ok(scout_result(0))
        }
    }

    #[tokio::test]
    async fn mcp_normalizes_safe_absolute_paths_before_backend_validation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "fn start() {}\n").unwrap();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let server = McpServer::new(
            Arc::new(CapturingScout {
                request: captured.clone(),
            }),
            root.path().to_path_buf(),
        );

        server
            .tools_call(json!({
                "name": "repo_scout",
                "arguments": {
                    "query": "trace",
                    "focus": root.path().join("src").display().to_string(),
                    "investigation": {
                        "target_paths": [root.path().join("src/lib.rs").display().to_string()]
                    }
                }
            }))
            .await
            .unwrap();

        let request = captured.lock().unwrap().clone().unwrap();
        assert_eq!(request.focus, Some(PathBuf::from("src")));
        assert_eq!(request.investigation.target_paths, ["src/lib.rs"]);
    }

    #[tokio::test]
    async fn mcp_keeps_a_nonexistent_relative_focus_as_a_hint() {
        let root = tempfile::tempdir().unwrap();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let server = McpServer::new(
            Arc::new(CapturingScout {
                request: captured.clone(),
            }),
            root.path().to_path_buf(),
        );
        let focus = ".codex-worktrees/studio-terminal-preparation-20260906-e9cca8516/studio/backend/core/inference/windows_sandbox";

        server
            .tools_call(json!({
                "name": "repo_scout",
                "arguments": {"query": "trace sandbox setup", "focus": focus}
            }))
            .await
            .unwrap();

        let request = captured.lock().unwrap().clone().unwrap();
        assert_eq!(request.focus, Some(PathBuf::from(focus)));
    }

    #[tokio::test]
    async fn mcp_accepts_outside_absolute_paths_for_backend_context() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let outside = parent.path().join("secret.rs");
        std::fs::write(&outside, "secret\n").unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server = McpServer::new(
            Arc::new(CountingScout {
                calls: calls.clone(),
            }),
            root,
        );

        let response = server
            .tools_call(json!({
                "name": "repo_scout",
                "arguments": {
                    "query": "trace",
                    "investigation": {"target_paths": [outside.display().to_string()]}
                }
            }))
            .await
            .unwrap();

        assert_eq!(response["isError"], false);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn explicit_investigation_runs_even_in_a_tiny_repository() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.rs"), "fn main() {}\n").unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server = McpServer::new(
            Arc::new(CountingScout {
                calls: calls.clone(),
            }),
            root.path().to_path_buf(),
        );

        let response = server
            .tools_call(json!({
                "name": "repo_scout",
                "arguments": { "query": "Find the shared root of this narrow bug" }
            }))
            .await
            .unwrap();

        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(response["structuredContent"].get("investigation").is_none());
    }

    #[tokio::test]
    async fn larger_repository_still_starts_scout() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..=40 {
            std::fs::write(root.path().join(format!("{index}.rs")), "// source\n").unwrap();
        }
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server = McpServer::new(
            Arc::new(CountingScout {
                calls: calls.clone(),
            }),
            root.path().to_path_buf(),
        );

        let response = server
            .tools_call(json!({
                "name": "repo_scout",
                "arguments": { "query": "Trace a broad cross-component flow" }
            }))
            .await
            .unwrap();

        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(response["structuredContent"].get("next_action").is_none());
    }

    #[test]
    fn handoff_keeps_more_than_twelve_citations_without_intent_caps() {
        let root = tempfile::tempdir().unwrap();
        let citations = (0..13)
            .map(|index| {
                let path = format!("{index}.rs");
                std::fs::write(
                    root.path().join(&path),
                    format!("const VALUE_{index}: u8 = {index};\n"),
                )
                .unwrap();
                ValidatedCitation {
                    path,
                    start_line: 1,
                    end_line: 1,
                    reason: Some("direct implementation".into()),
                }
            })
            .collect::<Vec<_>>();
        let mut result = scout_result(0);
        result.investigation.status = repotracer_core::InvestigationStatus::Complete;
        result.citations = citations;
        let response = handoff_response(root.path(), result);
        assert_eq!(
            response["structuredContent"]["citations"]
                .as_array()
                .unwrap()
                .len(),
            13
        );
        assert_eq!(
            response["structuredContent"]["evidence"]
                .as_array()
                .unwrap()
                .len(),
            13
        );
    }

    #[test]
    fn full_large_source_span_is_not_capped_at_legacy_excerpt_size() {
        let root = tempfile::tempdir().unwrap();
        let source = (1..=100)
            .map(|line| format!("source line {line}\n"))
            .collect::<String>();
        std::fs::write(root.path().join("lib.rs"), &source).unwrap();
        let mut result = scout_result(0);
        result.investigation.status = repotracer_core::InvestigationStatus::Complete;
        result.citations = vec![ValidatedCitation {
            path: "lib.rs".into(),
            start_line: 1,
            end_line: 100,
            reason: Some("entire function context".into()),
        }];
        let response = handoff_response(root.path(), result);
        let evidence = &response["structuredContent"]["evidence"][0];
        let evidence_text = embedded_text(&response, &evidence["text"]);
        assert!(evidence_text.contains("1: source line 1"));
        assert!(evidence_text.contains("100: source line 100"));
        assert_eq!(evidence["truncated"], false);
        assert!(response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("100: source line 100"));
    }

    #[test]
    fn long_answer_and_large_selected_span_are_delivered_together() {
        let root = tempfile::tempdir().unwrap();
        let source = "source ".repeat(18_000);
        std::fs::write(root.path().join("large.rs"), &source).unwrap();
        let answer = "answer ".repeat(9_000);
        let mut result = scout_result(0);
        result.summary = answer.clone();
        result.citations = vec![ValidatedCitation {
            path: "large.rs".into(),
            start_line: 1,
            end_line: 1,
            reason: Some("selected implementation".into()),
        }];

        let response = handoff_response(root.path(), result);
        assert_eq!(response["structuredContent"]["report"], answer);
        assert_eq!(
            response["structuredContent"]["evidence"][0]["truncated"],
            false
        );
        assert!(
            response["structuredContent"]["evidence"][0]["text"]
                .as_str()
                .unwrap()
                .len()
                > 100_000
        );
        assert!(response["content"][0]["text"].as_str().unwrap().len() > 150_000);
    }

    #[test]
    fn handoff_reads_only_the_cited_part_of_a_large_file() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let mut file = std::fs::File::create(root.path().join("large.rs")).unwrap();
        file.write_all(b"fn answer() {}\n\xff").unwrap();
        file.set_len(64 * 1024 * 1024).unwrap();
        let bundle = evidence_excerpts(
            root.path(),
            &[ValidatedCitation {
                path: "large.rs".into(),
                start_line: 1,
                end_line: 1,
                reason: None,
            }],
        );
        assert_eq!(bundle.spans.len(), 1);
        assert_eq!(bundle.spans[0].text, "1: fn answer() {}");
        assert!(!bundle.spans[0].truncated);
    }

    #[test]
    fn evidence_loading_preserves_long_lines_before_handoff_serialization() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("long.rs"), "λ".repeat(140_000)).unwrap();
        let citations = [ValidatedCitation {
            path: "long.rs".into(),
            start_line: 1,
            end_line: 1,
            reason: None,
        }];
        let bundle = evidence_excerpts(root.path(), &citations);
        assert_eq!(bundle.spans.len(), 1);
        assert!(bundle.spans[0].text.len() > 140_000);
        assert!(!bundle.spans[0].truncated);
    }

    #[test]
    fn overlapping_and_contained_spans_are_one_source_context_block() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("lib.rs"),
            (1..=12)
                .map(|line| format!("line {line}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let mut result = scout_result(0);
        result.investigation.status = repotracer_core::InvestigationStatus::Complete;
        result.citations = vec![
            ValidatedCitation {
                path: "lib.rs".into(),
                start_line: 1,
                end_line: 5,
                reason: None,
            },
            ValidatedCitation {
                path: "lib.rs".into(),
                start_line: 3,
                end_line: 8,
                reason: None,
            },
            ValidatedCitation {
                path: "lib.rs".into(),
                start_line: 4,
                end_line: 4,
                reason: None,
            },
            ValidatedCitation {
                path: "lib.rs".into(),
                start_line: 9,
                end_line: 9,
                reason: None,
            },
        ];
        let response = handoff_response(root.path(), result);
        let evidence = response["structuredContent"]["evidence"]
            .as_array()
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0]["start_line"], 1);
        assert_eq!(evidence[0]["end_line"], 9);
        assert!(embedded_text(&response, &evidence[0]["text"]).contains("9: line 9"));
    }

    #[test]
    fn each_rendering_is_self_contained_and_has_one_copy_of_source() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn answer() { return 42; }\n").unwrap();
        let mut result = scout_result(0);
        result.summary = "The answer is returned by the leaf function.".into();
        result.investigation.confidence = repotracer_core::InvestigationConfidence {
            level: repotracer_core::ConfidenceLevel::High,
            basis: "Read the leaf function; no execution was needed for this location question."
                .into(),
        };
        result.investigation.status = repotracer_core::InvestigationStatus::Complete;
        result.investigation.findings = vec![repotracer_core::Finding {
            question: "where is the answer?".into(),
            answer: "The leaf function returns the value.".into(),
            citations: vec![ValidatedCitation {
                path: "lib.rs".into(),
                start_line: 1,
                end_line: 1,
                reason: Some("leaf".into()),
            }],
        }];
        result.citations = result.investigation.findings[0].citations.clone();
        let response = handoff_response(root.path(), result);
        let text = response["content"][0]["text"].as_str().unwrap();
        let structured = &response["structuredContent"];
        assert!(text.contains("The leaf function returns the value."));
        assert!(text.contains("Sources: lib.rs:1-1"));
        assert!(text.contains("1: fn answer() { return 42; }"));
        assert_eq!(structured["handoff_version"], 3);
        let report = embedded_text(&response, &structured["report"]);
        assert!(report.contains("The answer is returned"));
        assert!(report.contains("The leaf function returns the value."));
        assert!(embedded_text(&response, &structured["evidence"][0]["text"]).contains("return 42"));
        assert!(structured.get("summary").is_none());
        assert!(structured.get("investigation").is_none());
        assert!(structured.get("report_ref").is_none());
        assert!(structured["evidence"][0].get("text_ref").is_none());
        assert!(structured.to_string().contains("return 42"));
        assert_eq!(
            structured
                .to_string()
                .matches("The leaf function returns the value.")
                .count(),
            1
        );
    }

    #[test]
    fn malformed_or_outside_citations_never_embed_outside_source() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            parent.path().join("secret.rs"),
            "DO NOT EMBED THIS SECRET\n",
        )
        .unwrap();
        let mut result = scout_result(0);
        result.investigation.status = repotracer_core::InvestigationStatus::Complete;
        result.citations = vec![
            ValidatedCitation {
                path: "../secret.rs".into(),
                start_line: 1,
                end_line: 1,
                reason: None,
            },
            ValidatedCitation {
                path: "missing.rs".into(),
                start_line: 0,
                end_line: 1,
                reason: None,
            },
        ];
        let response = handoff_response(&root, result);
        assert!(!response.to_string().contains("DO NOT EMBED THIS SECRET"));
        assert_eq!(
            response["structuredContent"]["evidence"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            response["structuredContent"]["evidence_omissions"]["explicit"],
            true
        );
        assert!(response["structuredContent"]["citations"][0]
            .get("source_error")
            .is_some());
        assert!(
            response["structuredContent"]["evidence_omissions"]["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error["path"] == "../secret.rs")
        );
    }

    #[test]
    fn duplicate_locations_do_not_remove_distinct_evidence() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn main() {}\n").unwrap();
        let citation = ValidatedCitation {
            path: "lib.rs".into(),
            start_line: 1,
            end_line: 1,
            reason: None,
        };
        let mut result = scout_result(0);
        result.investigation.status = repotracer_core::InvestigationStatus::Complete;
        result.investigation.findings = vec![repotracer_core::Finding {
            question: "where?".into(),
            answer: "lib.rs".into(),
            citations: vec![citation.clone()],
        }];
        result.citations = vec![citation; 10];
        let response = handoff_response(root.path(), result);
        assert_eq!(
            response["structuredContent"]["citations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            response["structuredContent"]["citations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(response["structuredContent"].get("investigation").is_none());
    }

    #[test]
    fn empty_handoff_has_no_routing_prose() {
        let response = handoff_response(Path::new("."), scout_result(0));
        assert!(response["structuredContent"].get("next_action").is_none());
        assert!(!response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Fall back to normal repository exploration"));
    }
}
