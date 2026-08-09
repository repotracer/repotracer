//! Minimal MCP JSON-RPC server over stdio for grephound.
//! NEVER write non-protocol text to stdout.

use grephound_core::{ScoutEngine, ScoutRequest};
use grephound_model::ModelBackend;
use grephound_repo_tools::RepoTools;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "grephound";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

const REPO_SCOUT_DESC: &str = "Use repo_scout when you need to locate or understand behavior across an unfamiliar repository, trace multi-file flows, identify implementation and related tests, or discover where a feature lives. It autonomously explores the repository with a dedicated read-only model and returns focused file/line citations. Skip it for trivial known-file edits or when the exact relevant code is already in context.";

pub struct McpServer {
    engine: Arc<ScoutEngine>,
    root: PathBuf,
}

impl McpServer {
    pub fn new(engine: ScoutEngine, root: PathBuf) -> Self {
        Self {
            engine: Arc::new(engine),
            root,
        }
    }

    pub fn from_backend(
        model: Arc<dyn ModelBackend>,
        root: PathBuf,
        budget: grephound_core::ExplorerBudget,
    ) -> Self {
        let tools = RepoTools::new(root.clone());
        let engine = ScoutEngine::new(model, tools, budget);
        Self::new(engine, root)
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
                    "tools": {}
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
            "prompts/list" => Ok(json!({ "prompts": [] })),
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

        let result = self
            .engine
            .scout(ScoutRequest {
                query,
                root: self.root.clone(),
                focus,
                max_turns: None,
                timeout: None,
            })
            .await
            .map_err(|e| rpc_error(-32000, e.to_string()))?;

        // Compact structured content for the frontier model.
        let text = result.compact_text();
        let structured = json!({
            "summary": result.summary,
            "citations": result.citations,
            "stats": result.stats,
        });

        Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": text
                }
            ],
            "structuredContent": structured,
            "isError": false
        }))
    }
}

fn repo_scout_tool_def() -> Value {
    json!({
        "name": "repo_scout",
        "description": REPO_SCOUT_DESC,
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
    let body = serde_json::to_vec(msg)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}
