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
use tokio::sync::mpsc;
use tokio::time::Instant as TokioInstant;

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
    service_tier: String,
    idle_timeout: Option<Duration>,
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
            effort @ ("low" | "medium" | "high" | "xhigh" | "max") => effort.to_string(),
            effort => bail!(
                "unsupported scout reasoning effort `{effort}`; use low, medium, high, xhigh, or max"
            ),
        };
        let service_tier = match cfg.model.service_tier.trim() {
            "default" => "default",
            "fast" | "priority" => "priority",
            tier => {
                bail!("unsupported scout service tier `{tier}`; use default, fast, or priority")
            }
        }
        .to_string();
        Ok(Self {
            executable,
            model,
            reasoning_effort,
            service_tier,
            idle_timeout: (cfg.model.timeout_ms > 0)
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
                    self.idle_timeout
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
            "Read-only repository scout. Search the repository; never edit, use the network, or delegate. Use the fewest repository tool calls that support every requested facet; batch independent searches and reads, keep each tool result under 120 lines, and stop when each material claim has direct code evidence. Do not repeat searches or browse unrelated files. Answer concisely, then cite the smallest direct evidence covering the question: normally 3-4 repository-relative ranges, at most 5, each ideally 40 lines or fewer. Every material claim needs a citation; cite leaf implementations rather than only dispatch callers. Put implementation and tests first; omit optional context.{}\n\nQuestion: {}",
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
            OsString::from("--config"),
            format!(
                "service_tier={}",
                toml::Value::String(self.service_tier.clone())
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
                request.timeout.or(self.idle_timeout),
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
        idle_timeout: Option<Duration>,
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
        let process_group = child.id();
        let mut stdin = child.stdin.take().context("missing provider stdin")?;
        let stdout = child.stdout.take().context("missing provider stdout")?;
        let stderr = child.stderr.take().context("missing provider stderr")?;
        let (activity_tx, mut activity_rx) = mpsc::channel(1);
        let stderr_task = tokio::spawn(drain_limited(
            stderr,
            MAX_CAPTURE_BYTES,
            activity_tx.clone(),
        ));
        let result = {
            let session = app_server_session(
                &mut stdin,
                BufReader::new(stdout).lines(),
                cwd,
                prompt,
                self,
                activity_tx,
            );
            tokio::pin!(session);
            if let Some(idle_timeout) = idle_timeout {
                let deadline = tokio::time::sleep_until(TokioInstant::now() + idle_timeout);
                tokio::pin!(deadline);
                let mut activity_open = true;
                loop {
                    tokio::select! {
                        biased;
                        result = &mut session => break result,
                        activity = activity_rx.recv(), if activity_open => match activity {
                            Some(()) => deadline.as_mut().reset(TokioInstant::now() + idle_timeout),
                            None => activity_open = false,
                        },
                        _ = &mut deadline => {
                            kill_process_tree(&mut child, process_group).await;
                            let _ = stderr_task.await;
                            bail!(
                                "{} produced no output for {}s",
                                self.label(),
                                idle_timeout.as_secs_f32()
                            );
                        }
                    }
                }
            } else {
                session.as_mut().await
            }
        };
        drop(stdin);
        let _ = tokio::time::timeout(Duration::from_millis(250), child.wait()).await;
        kill_process_tree(&mut child, process_group).await;
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
    scout: &CliScout,
    activity: mpsc::Sender<()>,
) -> Result<AppServerResult>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    send_message(
        stdin,
        &json!({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "repotracer", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {}
        }}),
    )
    .await?;
    wait_for_response(stdin, &mut lines, 1, &activity).await?;
    send_message(stdin, &json!({"method": "initialized", "params": {}})).await?;
    let thread_params = json!({
        "cwd": cwd,
        "ephemeral": true,
        "approvalPolicy": "never",
        "developerInstructions": APP_SERVER_INSTRUCTIONS,
        "model": scout.model.as_deref(),
        "serviceTier": scout.service_tier,
        "config": {
            "approval_policy": "never",
            "default_permissions": ":read-only",
            "project_doc_max_bytes": 0
        }
    });
    send_message(
        stdin,
        &json!({"id": 2, "method": "thread/start", "params": thread_params}),
    )
    .await?;
    let started = wait_for_response(stdin, &mut lines, 2, &activity).await?;
    let thread_id = started["thread"]["id"]
        .as_str()
        .context("Codex app-server returned no thread id")?;

    send_message(
        stdin,
        &json!({"id": 3, "method": "turn/start", "params": {
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt}],
            "effort": scout.reasoning_effort,
            "outputSchema": output_schema()
        }}),
    )
    .await?;
    wait_for_response(stdin, &mut lines, 3, &activity).await?;

    let mut raw = None;
    let mut metrics = CodexMetrics::default();
    loop {
        let message = next_message(&mut lines, &activity).await?;
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

async fn wait_for_response<R, W>(
    stdin: &mut W,
    lines: &mut Lines<R>,
    id: u64,
    activity: &mpsc::Sender<()>,
) -> Result<Value>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let message = next_message(lines, activity).await?;
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

async fn next_message<R: AsyncBufRead + Unpin>(
    lines: &mut Lines<R>,
    activity: &mpsc::Sender<()>,
) -> Result<Value> {
    let line = lines
        .next_line()
        .await?
        .context("Codex app-server closed its output")?;
    let message =
        serde_json::from_str(&line).context("Codex app-server returned malformed JSON")?;
    let _ = activity.try_send(());
    Ok(message)
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
                    let source_home = std::env::var_os("CODEX_HOME")
                        .map(PathBuf::from)
                        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
                    if let Some(source_home) = source_home {
                        let auth = source_home.join("auth.json");
                        if auth.is_file() {
                            link_auth(&auth, &path.join("auth.json"))?;
                        }
                        write_provider_config(
                            &source_home.join("config.toml"),
                            &path.join("config.toml"),
                        )?;
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

/// Keep the active Codex provider while excluding user MCPs, hooks, plugins,
/// and other session settings from the isolated scout home.
fn write_provider_config(source: &Path, target: &Path) -> Result<()> {
    let text = match std::fs::read_to_string(source) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("could not read Codex config {}", source.display()))
        }
    };
    let config: toml::Value = toml::from_str(&text)
        .with_context(|| format!("could not parse Codex config {}", source.display()))?;
    let config = config
        .as_table()
        .context("Codex config root must be a TOML table")?;
    let mut child = toml::map::Map::new();

    for key in [
        "model",
        "model_provider",
        "openai_base_url",
        "cli_auth_credentials_store",
    ] {
        if let Some(value) = config.get(key) {
            child.insert(key.into(), value.clone());
        }
    }

    if let Some(provider_id) = config.get("model_provider").and_then(toml::Value::as_str) {
        if let Some(provider) = config
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .and_then(|providers| providers.get(provider_id))
        {
            let mut providers = toml::map::Map::new();
            providers.insert(provider_id.into(), provider.clone());
            child.insert("model_providers".into(), toml::Value::Table(providers));
        }
    }

    if child.is_empty() {
        return Ok(());
    }
    std::fs::write(target, toml::to_string(&toml::Value::Table(child))?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
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
    activity: mpsc::Sender<()>,
) -> std::io::Result<Vec<u8>> {
    let mut kept = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(kept);
        }
        if buffer[..read]
            .iter()
            .any(|byte| !byte.is_ascii_whitespace())
        {
            let _ = activity.try_send(());
        }
        let remaining = limit.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

async fn kill_process_tree(child: &mut tokio::process::Child, process_group: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = process_group {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // The child starts a new process group, so a negative PID targets it and its descendants.
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
    #[cfg(windows)]
    if let Some(pid) = process_group {
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
            .any(|pair| pair == ["--config", "service_tier=\"priority\""]));
        #[cfg(windows)]
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--config", "windows.sandbox=\"unelevated\""]));
        assert!(CliScout::from_config(&config("claude-cli", Path::new("claude"))).is_err());
    }

    #[test]
    fn accepts_supported_and_rejects_unknown_reasoning_effort() {
        for effort in ["low", "medium", "high", "xhigh", "max"] {
            let mut cfg = config("codex-cli", Path::new("codex"));
            cfg.model.reasoning_effort = effort.into();
            assert!(CliScout::from_config(&cfg).is_ok());
        }
        let mut cfg = config("codex-cli", Path::new("codex"));
        cfg.model.reasoning_effort = "maximum".into();
        assert!(CliScout::from_config(&cfg)
            .err()
            .unwrap()
            .to_string()
            .contains("use low, medium, high, xhigh, or max"));
    }

    #[test]
    fn fast_service_tier_maps_to_priority() {
        let mut cfg = config("codex-cli", Path::new("codex"));
        cfg.model.service_tier = "fast".into();
        let scout = CliScout::from_config(&cfg).unwrap();
        assert_eq!(scout.service_tier, "priority");
        assert!(scout
            .app_server_args()
            .windows(2)
            .any(|pair| pair == ["--config", "service_tier=\"priority\""]));

        cfg.model.service_tier = "slow".into();
        assert!(CliScout::from_config(&cfg)
            .err()
            .unwrap()
            .to_string()
            .contains("use default, fast, or priority"));
    }

    #[tokio::test]
    async fn startup_reports_fatal_app_server_notifications() {
        let mut sink = tokio::io::sink();
        let mut lines = BufReader::new(
            &b"{\"method\":\"error\",\"params\":{\"willRetry\":false,\"error\":{\"message\":\"sandbox failed\"}}}\n"[..],
        )
        .lines();
        let (activity, _activity_rx) = mpsc::channel(1);
        let error = wait_for_response(&mut sink, &mut lines, 1, &activity)
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
    fn scout_prompt_requests_an_evidence_bounded_handoff() {
        let scout = CliScout::from_config(&config("codex-cli", Path::new("codex"))).unwrap();
        let root = tempfile::tempdir().unwrap();
        let prompt = scout.prompt(&ScoutRequest {
            query: "trace auth".into(),
            root: root.path().to_path_buf(),
            focus: None,
            max_turns: None,
            timeout: None,
        });
        assert!(prompt.contains("fewest repository tool calls"));
        assert!(prompt.contains("each material claim has direct code evidence"));
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
    async fn activity_extends_idle_deadline_and_silence_kills_tree() {
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
      sleep 0.1
      printf '%s\n' '{"method":"item/completed","params":{"item":{"type":"commandExecution"},"threadId":"thread-1","turnId":"turn-1","completedAtMs":1}}'
      sleep 0.1
      printf '%s\n' '{"method":"item/completed","params":{"item":{"type":"agentMessage","text":"{\"answer\":\"found\",\"citations\":[{\"path\":\"source.rs\",\"start_line\":1,\"end_line\":1,\"reason\":\"entry\"}]}"},"threadId":"thread-1","turnId":"turn-1","completedAtMs":2}}'
      sleep 0.1
      printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-1","turnId":"turn-1","tokenUsage":{"last":{"inputTokens":100,"cachedInputTokens":40,"outputTokens":20,"reasoningOutputTokens":5,"totalTokens":120}}}}'
      sleep 0.1
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed","items":[]}}}'
      ;;
  esac
done
"##,
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut active_cfg = config("codex-cli", &fake);
        active_cfg.model.timeout_ms = 300;
        let scout = CliScout::from_config(&active_cfg).unwrap();
        let started = Instant::now();
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
        assert!(started.elapsed() >= Duration::from_millis(350));
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

        std::fs::write(
            &fake,
            "#!/bin/sh\n(sleep 0.5; touch descendant-survived) &\nsleep 5\n",
        )
        .unwrap();
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
        assert!(error.to_string().contains("produced no output"));
        assert!(started.elapsed() < Duration::from_secs(2));
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!dir.path().join("descendant-survived").exists());

        std::fs::write(
            &fake,
            "#!/bin/sh\n(sleep 0.5; touch descendant-after-exit) >/dev/null 2>&1 &\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let scout = CliScout::from_config(&timeout_cfg).unwrap();
        assert!(scout
            .scout(ScoutRequest {
                query: "provider exit".into(),
                root: dir.path().to_path_buf(),
                focus: None,
                max_turns: None,
                timeout: None,
            })
            .await
            .is_err());
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!dir.path().join("descendant-after-exit").exists());
    }

    #[test]
    fn child_config_keeps_the_selected_provider_and_drops_user_tools() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("config.toml");
        let target = dir.path().join("child-config.toml");
        std::fs::write(
            &source,
            r#"
model = "gpt-5.6-luna"
model_provider = "codex-lb"
openai_base_url = "https://ignored-for-custom-provider.example"
cli_auth_credentials_store = "keyring"

[model_providers.codex-lb]
name = "Codex LB"
base_url = "https://codex-lb.example/v1"
wire_api = "responses"
requires_openai_auth = true

[mcp_servers.secret]
command = "do-not-copy"

[hooks.SessionStart]
hooks = []
"#,
        )
        .unwrap();

        write_provider_config(&source, &target).unwrap();
        let child: toml::Value = toml::from_str(&std::fs::read_to_string(target).unwrap()).unwrap();
        assert_eq!(child["model"].as_str(), Some("gpt-5.6-luna"));
        assert_eq!(child["model_provider"].as_str(), Some("codex-lb"));
        assert_eq!(
            child["cli_auth_credentials_store"].as_str(),
            Some("keyring")
        );
        assert_eq!(
            child["model_providers"]["codex-lb"]["base_url"].as_str(),
            Some("https://codex-lb.example/v1")
        );
        assert!(child.get("mcp_servers").is_none());
        assert!(child.get("hooks").is_none());
    }

    #[test]
    fn child_config_preserves_the_builtin_openai_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("config.toml");
        let target = dir.path().join("child-config.toml");
        std::fs::write(&source, "openai_base_url = \"https://proxy.example/v1\"\n").unwrap();

        write_provider_config(&source, &target).unwrap();
        let child: toml::Value = toml::from_str(&std::fs::read_to_string(target).unwrap()).unwrap();
        assert_eq!(
            child["openai_base_url"].as_str(),
            Some("https://proxy.example/v1")
        );
        assert!(child.get("model_providers").is_none());
    }
}
