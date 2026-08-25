//! Minimal MCP JSON-RPC server over stdio for repotracer.
//! NEVER write non-protocol text to stdout.

use repotracer_core::{ScoutBackend, ScoutRequest, ScoutResult, ValidatedCitation};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, error};

mod update_notice;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "repotracer";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_HANDOFF_CITATIONS: usize = 5;
const MAX_EVIDENCE_BYTES: usize = 6 * 1024;
const MAX_EXCERPT_BYTES: usize = 1200;
const MAX_EXCERPT_LINES: u32 = 40;
const SUCCESSFUL_HANDOFF: &str = "Broad repository exploration is complete. Use the summary and included evidence excerpts. For read-only explanation or planning, you MUST answer immediately and MUST NOT make another repository tool call. For edits, read one narrow cited range only when the handoff does not resolve a specific fact needed for the change. Do not run repository-wide file listings, broad Grep/Glob searches, or unrelated documentation reads. RepoTracer cannot inspect Git history; when the task describes a regression and current-source evidence does not establish what changed, run one targeted history lookup before selecting the fix.";
const EMPTY_HANDOFF: &str =
    "No validated evidence was returned. Fall back to normal repository exploration.";
const REPO_SCOUT_DESC: &str = "Use repo_scout only when it replaces broad repository exploration: the relevant implementation is unknown and the task needs unfamiliar cross-file understanding, or a targeted search failed. Skip when the prompt names the relevant files or symbols, supplies a precise change surface, the code is already in context, or one targeted lookup is likely enough. A successful result completes broad exploration and includes bounded source excerpts. For read-only explanation or planning, answer immediately without another repository tool call. For edits, read one narrow cited range only for a specific unresolved fact. Do not repeat repository-wide searches or unrelated documentation reads. RepoTracer cannot inspect Git history; after a regression handoff, use one targeted history lookup when current source does not establish what changed.";

pub struct McpServer {
    scout: Arc<dyn ScoutBackend>,
    root: PathBuf,
}

impl McpServer {
    pub fn new(scout: Arc<dyn ScoutBackend>, root: PathBuf) -> Self {
        Self { scout, root }
    }

    /// Serve MCP over stdin/stdout until EOF.
    pub async fn serve_stdio(&self) -> anyhow::Result<()> {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let mut stdout = std::io::stdout();

        loop {
            let msg = match read_message(&mut reader) {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(e) => {
                    error!("mcp read error: {e}");
                    break;
                }
            };

            debug!(?msg, "mcp message");
            if let Some(resp) = self.handle_message(msg).await {
                write_message(&mut stdout, &resp)?;
            }
        }
        Ok(())
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
            "tools/list" => Ok(json!({
                "tools": [repo_scout_tool_def()]
            })),
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
        if focus.as_deref().is_some_and(|path| !valid_focus(path)) {
            return Ok(tool_text(
                "Error: `focus` must be a repository-relative subdirectory.",
                true,
            ));
        }

        let result = self
            .scout
            .scout(ScoutRequest {
                query,
                root: self.root.clone(),
                focus,
                max_turns: None,
                timeout: None,
            })
            .await
            .map_err(|e| rpc_error(-32000, e.to_string()))?;

        Ok(handoff_response(&self.root, result))
    }
}

/// The newer release, if one is known. Wired to a real version check separately;
/// returning None keeps the notice inert until then.
fn pending_update() -> Option<(&'static str, &'static str)> {
    None
}

fn handoff_response(root: &Path, mut result: ScoutResult) -> Value {
    let omitted = result.citations.len().saturating_sub(MAX_HANDOFF_CITATIONS);
    result.citations.truncate(MAX_HANDOFF_CITATIONS);
    let next_action = if result.citations.is_empty() {
        EMPTY_HANDOFF
    } else {
        SUCCESSFUL_HANDOFF
    };
    let (evidence, evidence_text) = evidence_excerpts(root, &result.citations);
    let mut text = format!("Next action: {next_action}\n\n{}", result.compact_text());
    text.push_str(&evidence_text);
    if omitted > 0 {
        text.push_str(&format!(
            "\n\n{omitted} lower-priority citation{} omitted from the handoff.",
            if omitted == 1 { " was" } else { "s were" }
        ));
    }
    if let Some(line) = update_notice::notice(pending_update()) {
        text.push_str(&line);
    }
    let structured = json!({
        "summary": result.summary,
        "citations": result.citations,
        "evidence": evidence,
        "next_action": next_action,
        "stats": result.stats,
    });

    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

fn evidence_excerpts(root: &Path, citations: &[ValidatedCitation]) -> (Vec<Value>, String) {
    let mut remaining = MAX_EVIDENCE_BYTES;
    let mut evidence = Vec::new();
    let mut text = String::new();

    for citation in citations {
        if remaining < 256 {
            break;
        }
        let Ok(source) = std::fs::read_to_string(root.join(&citation.path)) else {
            continue;
        };
        let (excerpt, truncated) = render_excerpt(
            &source,
            citation.start_line,
            citation.end_line,
            remaining.min(MAX_EXCERPT_BYTES),
        );
        if excerpt.is_empty() {
            continue;
        }
        remaining -= excerpt.len();
        if text.is_empty() {
            text.push_str("\n\nEvidence excerpts (use these before reading repository files):");
        }
        text.push_str(&format!(
            "\n\n--- {}:{}-{} ---\n{}",
            citation.path, citation.start_line, citation.end_line, excerpt
        ));
        evidence.push(json!({
            "path": citation.path,
            "start_line": citation.start_line,
            "end_line": citation.end_line,
            "truncated": truncated,
        }));
    }

    (evidence, text)
}

fn render_excerpt(source: &str, start: u32, end: u32, max_bytes: usize) -> (String, bool) {
    if end < start || max_bytes == 0 {
        return (String::new(), true);
    }
    let span = end - start + 1;
    let mut excerpt = String::new();
    let rendered = if span <= MAX_EXCERPT_LINES {
        append_lines(&mut excerpt, source, start, end, max_bytes)
    } else {
        let half = MAX_EXCERPT_LINES / 2;
        let mut count = append_lines(&mut excerpt, source, start, start + half - 1, max_bytes / 2);
        let marker = format!("... {} lines omitted ...\n", span - MAX_EXCERPT_LINES);
        if excerpt.len() + marker.len() <= max_bytes {
            excerpt.push_str(&marker);
        }
        count += append_lines(&mut excerpt, source, end - half + 1, end, max_bytes);
        count
    };
    while excerpt.ends_with('\n') {
        excerpt.pop();
    }
    (excerpt, rendered < span)
}

fn append_lines(out: &mut String, source: &str, start: u32, end: u32, max_bytes: usize) -> u32 {
    let mut count = 0;
    for (index, line) in source
        .lines()
        .enumerate()
        .skip(start.saturating_sub(1) as usize)
        .take((end - start + 1) as usize)
    {
        let rendered = format!("{}: {}\n", index + 1, line);
        if out.len() + rendered.len() > max_bytes {
            break;
        }
        out.push_str(&rendered);
        count += 1;
    }
    count
}
fn valid_focus(path: &std::path::Path) -> bool {
    !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
}

fn repo_scout_tool_def() -> Value {
    json!({
        "name": "repo_scout",
        "description": REPO_SCOUT_DESC,
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural-language repository question, e.g. 'Trace how refresh tokens are created, validated, and revoked.'"
                },
                "focus": {
                    "type": "string",
                    "description": "Optional subdirectory to bias exploration."
                }
            },
            "required": ["query"]
        }
    })
}

fn repo_scout_prompt_def() -> Value {
    json!({
        "name": "repo_scout",
        "description": "Delegate unknown-location or broad repository exploration to RepoTracer.",
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
        "description": "Explore the repository with RepoTracer, then treat its validated citations as the completed exploration handoff.",
        "messages": [{
            "role": "user",
            "content": {
                "type": "text",
                "text": format!("Call the `repo_scout` tool with this repository question. If it returns validated citations, broad exploration is complete. For read-only explanation or planning, you MUST answer immediately from its summary and included evidence excerpts and MUST NOT make another repository tool call. For edits, read one narrow cited range only when the handoff does not resolve a specific fact needed for the change. Do not run repository-wide searches, history scans, or unrelated documentation reads: {query}")
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

/// Content-Length framed or newline-delimited JSON.
fn read_message<R: BufRead>(reader: &mut R) -> anyhow::Result<Option<Value>> {
    let mut first = String::new();
    let n = reader.read_line(&mut first)?;
    if n == 0 {
        return Ok(None);
    }
    let first_trim = first.trim_end_matches(['\r', '\n']);
    if first_trim.is_empty() {
        return read_message(reader);
    }

    // Content-Length framing
    if first_trim
        .to_ascii_lowercase()
        .starts_with("content-length:")
    {
        let mut headers = first.clone();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                return Ok(None);
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let len = headers
            .lines()
            .find_map(|l| {
                let l = l.trim();
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .ok_or_else(|| anyhow::anyhow!("missing Content-Length"))?;

        let mut buf = vec![0u8; len];
        std::io::Read::read_exact(&mut *reader, &mut buf)?;
        let v: Value = serde_json::from_slice(&buf)?;
        return Ok(Some(v));
    }

    // Newline-delimited JSON (single line).
    let v: Value = serde_json::from_str(first_trim)?;
    Ok(Some(v))
}

fn write_message<W: Write>(writer: &mut W, msg: &Value) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *writer, msg)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(text.contains("broad exploration is complete"));
        assert!(text.contains("included evidence excerpts"));
        assert!(text.contains("MUST answer immediately"));
        assert_eq!(repo_scout_prompt_def()["arguments"][0]["required"], true);
    }

    #[test]
    fn repo_scout_description_matches_routing_contract() {
        let tool = repo_scout_tool_def();
        let description = tool["description"].as_str().unwrap();
        assert!(description.contains("replaces broad repository exploration"));
        assert!(description.contains("prompt names the relevant files or symbols"));
        assert!(description.contains("includes bounded source excerpts"));
        assert!(description.contains("Do not repeat repository-wide searches"));
        assert!(description.contains("answer immediately without another repository tool call"));
        assert!(description.contains("one targeted history lookup"));
    }

    #[test]
    fn focus_must_stay_repository_relative() {
        assert!(valid_focus(std::path::Path::new("src/auth")));
        assert!(!valid_focus(std::path::Path::new("../secrets")));
        assert!(!valid_focus(std::path::Path::new("/tmp/secrets")));
    }

    #[test]
    fn repo_scout_is_declared_read_only() {
        let annotations = &repo_scout_tool_def()["annotations"];
        assert_eq!(annotations["readOnlyHint"], true);
        assert_eq!(annotations["destructiveHint"], false);
        assert_eq!(annotations["openWorldHint"], false);
    }

    fn scout_result(citation_count: usize) -> ScoutResult {
        ScoutResult {
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

    #[test]
    fn successful_handoff_caps_evidence_and_stops_broad_search() {
        let response = handoff_response(Path::new("."), scout_result(10));
        let structured = &response["structuredContent"];
        assert_eq!(structured["citations"].as_array().unwrap().len(), 5);
        assert!(structured["next_action"]
            .as_str()
            .unwrap()
            .contains("Broad repository exploration is complete"));
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Do not run repository-wide"));
        assert!(text.contains("5 lower-priority citations were omitted"));
        assert!(!text.contains("src/6.rs"));
    }

    #[test]
    fn handoff_embeds_source_evidence_once() {
        let mut result = scout_result(1);
        result.citations[0].path = "Cargo.toml".into();
        result.citations[0].start_line = 1;
        result.citations[0].end_line = 17;
        let response = handoff_response(Path::new(env!("CARGO_MANIFEST_DIR")), result);
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Evidence excerpts"));
        assert!(text.contains("1: [package]"));
        assert_eq!(
            response["structuredContent"]["evidence"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(response["structuredContent"]["evidence"][0]
            .get("text")
            .is_none());
    }

    #[test]
    fn wide_excerpt_keeps_both_ends_within_budget() {
        let source = (1..=200)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let (excerpt, truncated) = render_excerpt(&source, 1, 200, MAX_EXCERPT_BYTES);
        assert!(truncated);
        assert!(excerpt.contains("1: line 1"));
        assert!(excerpt.contains("200: line 200"));
        assert!(excerpt.contains("160 lines omitted"));
        assert!(!excerpt.contains("100: line 100"));
        assert!(excerpt.len() <= MAX_EXCERPT_BYTES);
    }

    #[test]
    fn empty_handoff_explicitly_allows_normal_exploration() {
        let response = handoff_response(Path::new("."), scout_result(0));
        assert_eq!(response["structuredContent"]["next_action"], EMPTY_HANDOFF);
        assert!(response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No validated citations"));
    }

    #[test]
    fn responses_use_mcp_newline_framing() {
        let mut output = Vec::new();
        write_message(
            &mut output,
            &json!({ "jsonrpc": "2.0", "id": 1, "result": {} }),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "{\"id\":1,\"jsonrpc\":\"2.0\",\"result\":{}}\n"
        );
    }
}
