use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use repotracer_core::{
    validate_citations, Citation, RepoTracerConfig, ScoutBackend, ScoutRequest, ScoutResult,
    ScoutStats,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
    Lines,
};
use tokio::process::Command;

const MAX_CAPTURE_BYTES: usize = 1_048_576;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const GPT_SCOUT_LABEL: &str = "GPT scout via Codex CLI";
const APP_SERVER_INSTRUCTIONS: &str = "RepoTracer repository scout. Never call MCP tools, apps, hooks, plugins, browser or computer-use tools, or delegate. Never edit files or use the network.";

pub fn is_subscription_backend(cfg: &RepoTracerConfig) -> bool {
    matches!(
        cfg.model.backend.to_ascii_lowercase().as_str(),
        "codex" | "codex-cli"
    )
}

pub struct CliScout {
    executable: PathBuf,
    model: Option<String>,
    reasoning_effort: String,
    timeout: Option<Duration>,
}

impl CliScout {
    pub fn from_config(cfg: &RepoTracerConfig) -> Result<Self> {
        if !is_subscription_backend(cfg) {
            bail!("unsupported GPT backend `{}`", cfg.model.backend);
        }
        let executable = cfg
            .model
            .executable
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("codex"));
        let model = match cfg.model.model.trim() {
            "" | "default" | "account-default" => None,
            model if model.starts_with("gpt-") => Some(model.to_string()),
            model => bail!("unsupported model `{model}`; RepoTracer currently supports GPT models"),
        };
        let reasoning_effort = match cfg.model.reasoning_effort.trim() {
            effort @ ("low" | "medium" | "high") => effort.to_string(),
            effort => {
                bail!("unsupported scout reasoning effort `{effort}`; use low, medium, or high")
            }
        };
        Ok(Self {
            executable,
            model,
            reasoning_effort,
            timeout: (cfg.model.timeout_ms > 0)
                .then(|| Duration::from_millis(cfg.model.timeout_ms)),
        })
    }

    pub fn label(&self) -> &'static str {
        GPT_SCOUT_LABEL
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub async fn probe(&self, root: &Path) -> Result<()> {
        let result = self
            .scout(ScoutRequest {
                query: "Cite the first line of one relevant source or manifest file.".into(),
                root: root.to_path_buf(),
                focus: None,
                max_turns: Some(2),
                timeout: Some(
                    self.timeout
                        .unwrap_or(Duration::from_secs(60))
                        .min(Duration::from_secs(60)),
                ),
            })
            .await?;
        if result.citations.is_empty() {
            bail!("{} returned no valid citation", self.label());
        }
        Ok(())
    }

    fn prompt(&self, request: &ScoutRequest) -> String {
        let focus = request
            .focus
            .as_ref()
            .map(|path| format!(" Prefer `{}` when relevant.", path.display()))
            .unwrap_or_default();
        format!(
            "Read-only repository scout. Search the repository; never edit, use the network, or delegate. Use at most 3 repository tool calls; batch independent searches and reads, and keep each tool result under 120 lines. Answer concisely, then cite the smallest direct evidence covering the question: normally 3-4 repository-relative ranges, at most 5, each ideally 40 lines or fewer. Every material claim needs a citation; cite leaf implementations rather than only dispatch callers. Put implementation and tests first; omit optional context.{}\n\nQuestion: {}",
            focus, request.query
        )
    }

    fn app_server_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = [
            "app-server",
            "--listen",
            "stdio://",
            "--disable",
            "apps",
            "--disable",
            "browser_use",
            "--disable",
            "computer_use",
            "--disable",
            "image_generation",
            "--disable",
            "hooks",
            "--disable",
            "multi_agent",
            "--disable",
            "plugins",
            "--config",
            "approval_policy=\"never\"",
            "--config",
            "default_permissions=\":read-only\"",
            "--config",
            "project_doc_max_bytes=0",
            "--config",
            "service_tier=\"default\"",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.extend([
            OsString::from("--config"),
            format!(
                "model_reasoning_effort={}",
                toml::Value::String(self.reasoning_effort.clone())
            )
            .into(),
        ]);
        #[cfg(windows)]
        args.extend([
            OsString::from("--config"),
            OsString::from("windows.sandbox=\"unelevated\""),
        ]);
        args
    }

    async fn run(&self, request: ScoutRequest) -> Result<ScoutResult> {
        if !request.root.is_dir() {
            bail!("repository root does not exist: {}", request.root.display());
        }
        let started = Instant::now();
        let response = self
            .run_app_server(
                &request.root,
                &self.prompt(&request),
                request.timeout.or(self.timeout),
            )
            .await?;
        let raw = response.raw;
        let structured: StructuredOutput =
            serde_json::from_str(&raw).context("Codex returned malformed structured output")?;
        let citations = validate_citations(&request.root, &structured.citations);
        if !structured.citations.is_empty() && citations.is_empty() {
            bail!("{} returned only invalid citations", self.label());
        }
        let metrics = response.metrics;
        Ok(ScoutResult {
            summary: truncate_utf8(structured.answer.trim(), 3_000),
            citations,
            stats: ScoutStats {
                turns: metrics.tool_calls.saturating_add(1),
                tool_calls: metrics.tool_calls,
                duration_ms: started.elapsed().as_millis() as u64,
                model: match &self.model {
                    Some(model) => format!("{} ({model})", self.label()),
                    None => self.label().into(),
                },
                prompt_tokens: metrics.usage.as_ref().and_then(|usage| usage.input_tokens),
                cached_prompt_tokens: metrics
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.cached_input_tokens),
                completion_tokens: metrics.usage.as_ref().and_then(|usage| usage.output_tokens),
                reasoning_output_tokens: metrics
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.reasoning_output_tokens),
            },
            raw_final: Some(raw),
        })
    }

    async fn run_app_server(
        &self,
        cwd: &Path,
        prompt: &str,
        timeout: Option<Duration>,
    ) -> Result<AppServerResult> {
        let codex_home = IsolatedCodexHome::create()?;
        let mut command = Command::new(&self.executable);
        command
            .args(self.app_server_args())
            .current_dir(cwd)
            .env("CODEX_HOME", codex_home.path())
            .env("REPOTRACER_SUBPROCESS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start `{}`", self.executable.display()))?;
        let mut stdin = child.stdin.take().context("missing provider stdin")?;
        let stdout = child.stdout.take().context("missing provider stdout")?;
        let stderr = child.stderr.take().context("missing provider stderr")?;
        let stderr_task = tokio::spawn(drain_limited(stderr, MAX_CAPTURE_BYTES));
        let session = app_server_session(
            &mut stdin,
            BufReader::new(stdout).lines(),
            cwd,
            prompt,
            self.model.as_deref(),
            &self.reasoning_effort,
        );
        let result = match timeout {
            Some(timeout) => match tokio::time::timeout(timeout, session).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!(
                    "{} timed out after {}s",
                    self.label(),
                    timeout.as_secs_f32()
                )),
            },
            None => session.await,
        };
        drop(stdin);
        if !matches!(
            tokio::time::timeout(Duration::from_millis(250), child.wait()).await,
            Ok(Ok(_))
        ) {
            kill_process_tree(&mut child).await;
        }
        let stderr = stderr_task.await.context("provider stderr task failed")??;
        match result {
            Ok(result) => Ok(result),
            Err(error) if stderr.is_empty() => Err(error),
            Err(error) => Err(anyhow::anyhow!(
                "{error:#}; {}",
                provider_error(self.label(), &stderr)
            )),
        }
    }
}

#[async_trait]
impl ScoutBackend for CliScout {
    async fn scout(&self, request: ScoutRequest) -> Result<ScoutResult> {
        self.run(request).await
    }
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct StructuredOutput {
    answer: String,
    #[serde(default)]
    citations: Vec<Citation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    cached_input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
    #[serde(default)]
    reasoning_output_tokens: Option<u32>,
}

#[derive(Default)]
struct CodexMetrics {
    usage: Option<TokenUsage>,
    tool_calls: u32,
}

struct AppServerResult {
    raw: String,
    metrics: CodexMetrics,
}

async fn app_server_session<R, W>(
    stdin: &mut W,
    mut lines: Lines<R>,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
    reasoning_effort: &str,
) -> Result<AppServerResult>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    send_message(
        stdin,
        &json!({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "repotracer", "version": env!("CARGO_PKG_VERSION")}
        }}),
    )
    .await?;
    wait_for_response(stdin, &mut lines, 1).await?;
    send_message(stdin, &json!({"method": "initialized", "params": {}})).await?;

    send_message(
        stdin,
        &json!({"id": 2, "method": "thread/start", "params": {
            "cwd": cwd,
            "ephemeral": true,
            "approvalPolicy": "never",
            "developerInstructions": APP_SERVER_INSTRUCTIONS,
            "model": model,
            "serviceTier": "default",
            "config": {
                "approval_policy": "never",
                "default_permissions": ":read-only",
                "project_doc_max_bytes": 0
            }
        }}),
    )
    .await?;
    let started = wait_for_response(stdin, &mut lines, 2).await?;
    let thread_id = started["thread"]["id"]
        .as_str()
        .context("Codex app-server returned no thread id")?;

    send_message(
        stdin,
        &json!({"id": 3, "method": "turn/start", "params": {
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt}],
            "effort": reasoning_effort,
            "outputSchema": output_schema()
        }}),
    )
    .await?;
    wait_for_response(stdin, &mut lines, 3).await?;

    let mut raw = None;
    let mut metrics = CodexMetrics::default();
    loop {
        let message = next_message(&mut lines).await?;
        if message.get("id").is_some() && message.get("method").is_some() {
            reject_server_request(stdin, &message).await?;
            continue;
        }
        match message["method"].as_str() {
            Some("item/completed") => {
                let item = &message["params"]["item"];
                match item["type"].as_str() {
                    Some("agentMessage") => {
                        if let Some(text) = item["text"].as_str() {
                            raw = Some(text.to_string());
                        }
                    }
                    Some("commandExecution" | "mcpToolCall" | "webSearch") => {
                        metrics.tool_calls += 1;
                    }
                    _ => {}
                }
            }
            Some("thread/tokenUsage/updated") => {
                metrics.usage =
                    serde_json::from_value(message["params"]["tokenUsage"]["last"].clone()).ok();
            }
            Some("turn/completed") => {
                let turn = &message["params"]["turn"];
                if turn["status"] != "completed" {
                    let error = turn["error"]["message"]
                        .as_str()
                        .unwrap_or("Codex turn did not complete");
                    bail!("{error}");
                }
                if raw.is_none() {
                    raw = turn["items"]
                        .as_array()
                        .and_then(|items| {
                            items
                                .iter()
                                .rev()
                                .find(|item| item["type"] == "agentMessage")
                        })
                        .and_then(|item| item["text"].as_str())
                        .map(str::to_string);
                }
                return Ok(AppServerResult {
                    raw: raw.context("Codex app-server returned no structured result")?,
                    metrics,
                });
            }
            Some("error") if !message["params"]["willRetry"].as_bool().unwrap_or(false) => {
                bail!(
                    "{}",
                    message["params"]["error"]["message"]
                        .as_str()
                        .unwrap_or("Codex app-server turn failed")
                );
            }
            _ => {}
        }
    }
}

async fn wait_for_response<R, W>(stdin: &mut W, lines: &mut Lines<R>, id: u64) -> Result<Value>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let message = next_message(lines).await?;
        if message["id"].as_u64() == Some(id) {
            if let Some(error) = message.get("error") {
                bail!(
                    "Codex app-server request failed: {}",
                    error["message"].as_str().unwrap_or("unknown error")
                );
            }
            return message
                .get("result")
                .cloned()
                .context("Codex app-server response had no result");
        }
        if message["method"] == "error"
            && !message["params"]["willRetry"].as_bool().unwrap_or(false)
        {
            bail!(
                "{}",
                message["params"]["error"]["message"]
                    .as_str()
                    .unwrap_or("Codex app-server request failed")
            );
        }
        if message.get("id").is_some() && message.get("method").is_some() {
            reject_server_request(stdin, &message).await?;
        }
    }
}

async fn next_message<R: AsyncBufRead + Unpin>(lines: &mut Lines<R>) -> Result<Value> {
    let line = lines
        .next_line()
        .await?
        .context("Codex app-server closed its output")?;
    serde_json::from_str(&line).context("Codex app-server returned malformed JSON")
}

async fn send_message<W: AsyncWrite + Unpin>(stdin: &mut W, message: &Value) -> Result<()> {
    stdin.write_all(message.to_string().as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

async fn reject_server_request<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    request: &Value,
) -> Result<()> {
    send_message(
        stdin,
        &json!({
            "id": request["id"],
            "error": {"code": -32601, "message": "RepoTracer does not accept server requests"}
        }),
    )
    .await
}

fn provider_error(label: &str, stderr: &[u8]) -> String {
    let compact = String::from_utf8_lossy(stderr)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if compact.is_empty() {
        format!("{label} failed")
    } else {
        format!(
            "{label} failed: {}",
            compact.chars().take(500).collect::<String>()
        )
    }
}

struct IsolatedCodexHome {
    path: PathBuf,
}

impl IsolatedCodexHome {
    fn create() -> Result<Self> {
        for _ in 0..10 {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("repotracer-codex-home-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
                    }
                    let source = std::env::var_os("CODEX_HOME")
                        .map(PathBuf::from)
                        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
                        .map(|home| home.join("auth.json"));
                    if let Some(source) = source.filter(|source| source.is_file()) {
                        link_auth(&source, &path.join("auth.json"))?;
                    }
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not create isolated Codex home")
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IsolatedCodexHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn link_auth(source: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    if std::os::unix::fs::symlink(source, target).is_ok() {
        return Ok(());
    }
    if std::fs::hard_link(source, target).is_ok() {
        return Ok(());
    }
    std::fs::copy(source, target)
        .map(|_| ())
        .with_context(|| "could not make Codex authentication available to isolated scout")
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_string()
}

fn output_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "answer": { "type": "string", "maxLength": 3000 },
            "citations": {
                "type": "array",
                "description": "Smallest sufficient direct evidence map; normally 3-4 citations.",
                "maxItems": 5,
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Repository-relative evidence file." },
                        "start_line": { "type": "integer", "minimum": 1, "description": "First direct-evidence line." },
                        "end_line": { "type": "integer", "minimum": 1, "description": "Last direct-evidence line; ideally no more than 40 lines after start_line." },
                        "reason": { "type": "string", "maxLength": 200, "description": "Why this range is needed." }
                    },
                    "required": ["path", "start_line", "end_line", "reason"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["answer", "citations"],
        "additionalProperties": false
    })
}

async fn drain_limited<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut kept = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(kept);
        }
        let remaining = limit.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

async fn kill_process_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // The child starts a new process group, so a negative PID targets it and its descendants.
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use repotracer_core::ModelSettings;

    fn config(provider: &str, executable: &Path) -> RepoTracerConfig {
        RepoTracerConfig {
            model: ModelSettings {
                backend: provider.into(),
                executable: Some(executable.display().to_string()),
                model: "default".into(),
                timeout_ms: 2_000,
                ..ModelSettings::default()
            },
            ..RepoTracerConfig::default()
        }
    }

    #[test]
    fn provider_args_are_read_only_and_isolated() {
        let mut codex_config = config("codex-cli", Path::new("codex"));
        codex_config.model.model = "gpt-5.6-luna".into();
        codex_config.model.reasoning_effort = "medium".into();
        let codex = CliScout::from_config(&codex_config).unwrap();
        let codex_args = codex
            .app_server_args()
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(&codex_args[..3], ["app-server", "--listen", "stdio://"]);
        for feature in [
            "apps",
            "browser_use",
            "computer_use",
            "hooks",
            "image_generation",
            "multi_agent",
            "plugins",
        ] {
            assert!(codex_args
                .windows(2)
                .any(|pair| pair == ["--disable", feature]));
        }
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--config", "model_reasoning_effort=\"medium\""]));
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--config", "project_doc_max_bytes=0"]));
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--config", "default_permissions=\":read-only\""]));
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--config", "service_tier=\"default\""]));
        #[cfg(windows)]
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--config", "windows.sandbox=\"unelevated\""]));
        assert!(CliScout::from_config(&config("claude-cli", Path::new("claude"))).is_err());
    }

    #[test]
    fn rejects_unknown_reasoning_effort() {
        let mut cfg = config("codex-cli", Path::new("codex"));
        cfg.model.reasoning_effort = "maximum".into();
        assert!(CliScout::from_config(&cfg)
            .err()
            .unwrap()
            .to_string()
            .contains("use low, medium, or high"));
    }

    #[tokio::test]
    async fn startup_reports_fatal_app_server_notifications() {
        let mut sink = tokio::io::sink();
        let mut lines = BufReader::new(
            &b"{\"method\":\"error\",\"params\":{\"willRetry\":false,\"error\":{\"message\":\"sandbox failed\"}}}\n"[..],
        )
        .lines();
        let error = wait_for_response(&mut sink, &mut lines, 1)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("sandbox failed"));
    }

    #[cfg(unix)]
    #[test]
    fn isolated_codex_home_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let home = IsolatedCodexHome::create().unwrap();
        let mode = std::fs::metadata(home.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn scout_prompt_requests_a_ranked_bounded_handoff() {
        let scout = CliScout::from_config(&config("codex-cli", Path::new("codex"))).unwrap();
        let root = tempfile::tempdir().unwrap();
        let prompt = scout.prompt(&ScoutRequest {
            query: "trace auth".into(),
            root: root.path().to_path_buf(),
            focus: None,
            max_turns: None,
            timeout: None,
        });
        assert!(prompt.contains("at most 3 repository tool calls"));
        assert!(prompt.contains("under 120 lines"));
        assert!(prompt.contains("normally 3-4"));
        assert!(prompt.contains("at most 5"));
        assert!(prompt.contains("40 lines or fewer"));
        assert!(prompt.contains("implementation and tests first"));
        assert_eq!(output_schema()["properties"]["citations"]["maxItems"], 5);
        assert_eq!(output_schema()["properties"]["answer"]["maxLength"], 3000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_codex_result_is_validated_and_timeout_kills_child() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source.rs"), "fn main() {}\n").unwrap();
        let fake = dir.path().join("fake-codex");
        std::fs::write(
            &fake,
            r##"#!/bin/sh
printf '%s\n' "$@" > app-server-args
printf '%s' "$CODEX_HOME" > child-codex-home
i=0
while IFS= read -r line; do
  i=$((i + 1))
  case "$i" in
    1) printf '%s\n' '{"id":1,"result":{"userAgent":"fake","codexHome":"/tmp","platformFamily":"unix","platformOs":"linux"}}' ;;
    2) ;;
    3) printf '%s\n' '{"id":2,"result":{"thread":{"id":"thread-1"}}}' ;;
    4)
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn-1","status":"inProgress","items":[]}}}'
      printf '%s\n' '{"method":"item/completed","params":{"item":{"type":"commandExecution"},"threadId":"thread-1","turnId":"turn-1","completedAtMs":1}}'
      printf '%s\n' '{"method":"item/completed","params":{"item":{"type":"agentMessage","text":"{\"answer\":\"found\",\"citations\":[{\"path\":\"source.rs\",\"start_line\":1,\"end_line\":1,\"reason\":\"entry\"}]}"},"threadId":"thread-1","turnId":"turn-1","completedAtMs":2}}'
      printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-1","turnId":"turn-1","tokenUsage":{"last":{"inputTokens":100,"cachedInputTokens":40,"outputTokens":20,"reasoningOutputTokens":5,"totalTokens":120}}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed","items":[]}}}'
      ;;
  esac
done
"##,
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let scout = CliScout::from_config(&config("codex-cli", &fake)).unwrap();
        let result = scout
            .scout(ScoutRequest {
                query: "find entry".into(),
                root: dir.path().to_path_buf(),
                focus: None,
                max_turns: None,
                timeout: None,
            })
            .await
            .unwrap();
        assert_eq!(result.citations[0].path, "source.rs");
        assert_eq!(result.stats.turns, 2);
        assert_eq!(result.stats.tool_calls, 1);
        assert_eq!(result.stats.prompt_tokens, Some(100));
        assert_eq!(result.stats.cached_prompt_tokens, Some(40));
        assert_eq!(result.stats.completion_tokens, Some(20));
        assert_eq!(result.stats.reasoning_output_tokens, Some(5));
        assert!(std::fs::read_to_string(dir.path().join("app-server-args"))
            .unwrap()
            .starts_with("app-server\n--listen\nstdio://\n"));
        assert_ne!(
            std::fs::read_to_string(dir.path().join("child-codex-home")).unwrap(),
            std::env::var("CODEX_HOME").unwrap_or_default()
        );

        std::fs::write(&fake, "#!/bin/sh\nsleep 5\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut timeout_cfg = config("codex-cli", &fake);
        timeout_cfg.model.timeout_ms = 50;
        let scout = CliScout::from_config(&timeout_cfg).unwrap();
        let started = Instant::now();
        let error = scout
            .scout(ScoutRequest {
                query: "timeout".into(),
                root: dir.path().to_path_buf(),
                focus: None,
                max_turns: None,
                timeout: None,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
