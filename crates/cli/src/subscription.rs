use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use grephound_core::{
    validate_citations, Citation, GrephoundConfig, ScoutBackend, ScoutRequest, ScoutResult,
    ScoutStats,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const MAX_CAPTURE_BYTES: usize = 1_048_576;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const GPT_SCOUT_LABEL: &str = "GPT scout via Codex CLI";

pub fn is_subscription_backend(cfg: &GrephoundConfig) -> bool {
    matches!(
        cfg.model.backend.to_ascii_lowercase().as_str(),
        "codex" | "codex-cli"
    )
}

pub struct CliScout {
    executable: PathBuf,
    model: Option<String>,
    reasoning_effort: String,
    timeout: Duration,
}

impl CliScout {
    pub fn from_config(cfg: &GrephoundConfig) -> Result<Self> {
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
            model => bail!("unsupported model `{model}`; Grephound currently supports GPT models"),
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
            timeout: Duration::from_millis(cfg.model.timeout_ms),
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
                timeout: Some(self.timeout.min(Duration::from_secs(60))),
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

    fn codex_args(&self, schema: &Path, output: &Path, prompt: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = [
            "exec",
            "--json",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "--disable",
            "apps",
            "--disable",
            "browser_use",
            "--disable",
            "computer_use",
            "--disable",
            "image_generation",
            "--disable",
            "multi_agent",
            "--disable",
            "plugins",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--config",
            "approval_policy=\"never\"",
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
        args.extend(codex_provider_overrides());
        if let Some(model) = &self.model {
            args.extend([OsString::from("--model"), model.into()]);
        }
        args.extend([
            OsString::from("--output-schema"),
            schema.as_os_str().to_owned(),
            OsString::from("--output-last-message"),
            output.as_os_str().to_owned(),
            OsString::from(prompt),
        ]);
        args
    }

    async fn run(&self, request: ScoutRequest) -> Result<ScoutResult> {
        if !request.root.is_dir() {
            bail!("repository root does not exist: {}", request.root.display());
        }
        let started = Instant::now();
        let temp = TempRunDir::create()?;
        let schema = temp.path.join("schema.json");
        let output = temp.path.join("result.json");
        std::fs::write(&schema, output_schema().to_string())?;
        let process = self
            .run_process(
                self.codex_args(&schema, &output, &self.prompt(&request)),
                &request.root,
                request.timeout.unwrap_or(self.timeout),
            )
            .await?;
        if !process.status.success() {
            bail!("{} failed: {}", self.label(), process.error_text());
        }
        let raw = std::fs::read_to_string(&output)
            .with_context(|| format!("{} returned no structured result", self.label()))?;
        let structured: StructuredOutput =
            serde_json::from_str(&raw).context("Codex returned malformed structured output")?;
        let citations = validate_citations(&request.root, &structured.citations);
        if !structured.citations.is_empty() && citations.is_empty() {
            bail!("{} returned only invalid citations", self.label());
        }
        let metrics = codex_metrics(&process.stdout);
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

    async fn run_process(
        &self,
        args: Vec<OsString>,
        cwd: &Path,
        timeout: Duration,
    ) -> Result<ProcessOutput> {
        let mut command = Command::new(&self.executable);
        command
            .args(args)
            .current_dir(cwd)
            .env("GREPHOUND_SUBPROCESS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start `{}`", self.executable.display()))?;
        let stdout = child.stdout.take().context("missing provider stdout")?;
        let stderr = child.stderr.take().context("missing provider stderr")?;
        let stdout_task = tokio::spawn(drain_limited(stdout, MAX_CAPTURE_BYTES));
        let stderr_task = tokio::spawn(drain_limited(stderr, MAX_CAPTURE_BYTES));

        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                kill_process_tree(&mut child).await;
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                bail!(
                    "{} timed out after {}s",
                    self.label(),
                    timeout.as_secs_f32()
                );
            }
        };
        let stdout = stdout_task.await.context("provider stdout task failed")??;
        let stderr = stderr_task.await.context("provider stderr task failed")??;
        Ok(ProcessOutput {
            status,
            stdout,
            stderr,
        })
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

fn codex_metrics(stdout: &[u8]) -> CodexMetrics {
    let mut metrics = CodexMetrics::default();
    for event in String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if event["type"] == "turn.completed" {
            metrics.usage = serde_json::from_value(event["usage"].clone()).ok();
        } else if event["type"] == "item.completed"
            && matches!(
                event["item"]["type"].as_str(),
                Some("command_execution" | "mcp_tool_call" | "web_search")
            )
        {
            metrics.tool_calls += 1;
        }
    }
    metrics
}

fn codex_provider_overrides() -> Vec<OsString> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    let Some(path) = home.map(|home| home.join("config.toml")) else {
        return Vec::new();
    };
    let Ok(config) = std::fs::read_to_string(path)
        .and_then(|text| toml::from_str::<toml::Value>(&text).map_err(std::io::Error::other))
    else {
        return Vec::new();
    };
    let Some(provider) = config.get("model_provider").and_then(toml::Value::as_str) else {
        return Vec::new();
    };
    let Some(settings) = config
        .get("model_providers")
        .and_then(|providers| providers.get(provider))
        .and_then(toml::Value::as_table)
    else {
        return Vec::new();
    };
    let mut overrides = vec![
        OsString::from("--config"),
        format!(
            "model_provider={}",
            toml::Value::String(provider.to_string())
        )
        .into(),
    ];
    let provider = if provider
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        provider.to_string()
    } else {
        serde_json::to_string(provider).unwrap_or_default()
    };
    for key in [
        "name",
        "base_url",
        "wire_api",
        "supports_websockets",
        "requires_openai_auth",
        "request_max_retries",
        "stream_max_retries",
        "stream_idle_timeout_ms",
    ] {
        if let Some(value) = settings.get(key) {
            overrides.extend([
                OsString::from("--config"),
                format!("model_providers.{provider}.{key}={value}").into(),
            ]);
        }
    }
    overrides
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

struct ProcessOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ProcessOutput {
    fn error_text(&self) -> String {
        let text = if self.stderr.is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let text = String::from_utf8_lossy(text);
        let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if compact.is_empty() {
            format!("process exited with {}", self.status)
        } else {
            compact.chars().take(500).collect()
        }
    }
}

struct TempRunDir {
    path: PathBuf,
}

impl TempRunDir {
    fn create() -> Result<Self> {
        for _ in 0..10 {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("grephound-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not create temporary provider directory")
    }
}

impl Drop for TempRunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grephound_core::ModelSettings;

    fn config(provider: &str, executable: &Path) -> GrephoundConfig {
        GrephoundConfig {
            model: ModelSettings {
                backend: provider.into(),
                executable: Some(executable.display().to_string()),
                model: "default".into(),
                timeout_ms: 2_000,
                ..ModelSettings::default()
            },
            ..GrephoundConfig::default()
        }
    }

    #[test]
    fn provider_args_are_read_only_and_isolated() {
        let mut codex_config = config("codex-cli", Path::new("codex"));
        codex_config.model.model = "gpt-5.6-luna".into();
        codex_config.model.reasoning_effort = "medium".into();
        let codex = CliScout::from_config(&codex_config).unwrap();
        let codex_args = codex
            .codex_args(Path::new("schema"), Path::new("output"), "prompt")
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "read-only"]));
        assert!(codex_args.contains(&"--ignore-rules".into()));
        assert!(codex_args.contains(&"--ignore-user-config".into()));
        assert!(codex_args.contains(&"--ephemeral".into()));
        assert!(codex_args.contains(&"--json".into()));
        for feature in [
            "apps",
            "browser_use",
            "computer_use",
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
            .any(|pair| pair == ["--config", "service_tier=\"default\""]));
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--model", "gpt-5.6-luna"]));
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
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output-last-message" ]; then shift; out="$1"; fi
  shift
done
printf '%s' '{"answer":"found","citations":[{"path":"source.rs","start_line":1,"end_line":1,"reason":"entry"}]}' > "$out"
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":20,"reasoning_output_tokens":5}}'
printf '%s\n' '{"type":"item.completed","item":{"type":"command_execution"}}'
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
