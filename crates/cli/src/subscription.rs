use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use grephound_core::{
    validate_citations, Citation, GrephoundConfig, ScoutBackend, ScoutRequest, ScoutResult,
    ScoutStats,
};
use serde::Deserialize;
use serde_json::json;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const MAX_CAPTURE_BYTES: usize = 1_048_576;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliProvider {
    Codex,
    Claude,
}

impl CliProvider {
    fn from_backend(backend: &str) -> Option<Self> {
        match backend.to_ascii_lowercase().as_str() {
            "codex" | "codex-cli" => Some(Self::Codex),
            "claude" | "claude-cli" => Some(Self::Claude),
            _ => None,
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex subscription",
            Self::Claude => "Claude subscription",
        }
    }
}

pub fn is_subscription_backend(cfg: &GrephoundConfig) -> bool {
    CliProvider::from_backend(&cfg.model.backend).is_some()
}

pub struct CliScout {
    provider: CliProvider,
    executable: PathBuf,
    model: Option<String>,
    timeout: Duration,
    max_turns: u32,
}

impl CliScout {
    pub fn from_config(cfg: &GrephoundConfig) -> Result<Self> {
        let provider = CliProvider::from_backend(&cfg.model.backend)
            .with_context(|| format!("unsupported subscription backend `{}`", cfg.model.backend))?;
        let executable = cfg
            .model
            .executable
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(provider.command()));
        let model = match cfg.model.model.trim() {
            "" | "default" | "account-default" => None,
            model => Some(model.to_string()),
        };
        Ok(Self {
            provider,
            executable,
            model,
            timeout: Duration::from_millis(cfg.model.timeout_ms),
            max_turns: cfg.explorer.max_turns,
        })
    }

    pub fn provider(&self) -> CliProvider {
        self.provider
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub async fn probe(&self, root: &Path) -> Result<()> {
        let result = self
            .scout(ScoutRequest {
                query:
                    "Find one relevant source, manifest, or README file and cite its first line."
                        .into(),
                root: root.to_path_buf(),
                focus: None,
                max_turns: Some(2),
                timeout: Some(self.timeout.min(Duration::from_secs(60))),
            })
            .await?;
        if result.citations.is_empty() {
            bail!("{} returned no valid citation", self.provider.label());
        }
        Ok(())
    }

    fn prompt(&self, request: &ScoutRequest) -> String {
        let focus = request
            .focus
            .as_ref()
            .map(|path| {
                format!(
                    "\nPrefer evidence under `{}` when relevant.",
                    path.display()
                )
            })
            .unwrap_or_default();
        format!(
            "You are Grephound, a read-only repository scout. Autonomously search and read the repository to answer the question. Do not edit files, run network requests, or delegate to another agent. Return a concise answer and only repository-relative citations with exact line ranges.{}\n\nQuestion: {}",
            focus, request.query
        )
    }

    fn codex_args(&self, schema: &Path, output: &Path, prompt: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = [
            "exec",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--config",
            "approval_policy=\"never\"",
            "--config",
            "model_reasoning_effort=\"low\"",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
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

    fn claude_args(&self, turns: u32, prompt: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = [
            "-p",
            "--safe-mode",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--tools",
            "Read,Glob,Grep",
            "--allowedTools",
            "Read,Glob,Grep",
            "--permission-mode",
            "dontAsk",
            "--effort",
            "low",
            "--max-turns",
            &turns.to_string(),
            "--output-format",
            "json",
            "--json-schema",
            &output_schema().to_string(),
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        if let Some(model) = &self.model {
            args.extend([OsString::from("--model"), model.into()]);
        }
        args.push(prompt.into());
        args
    }

    async fn run(&self, request: ScoutRequest) -> Result<ScoutResult> {
        if !request.root.is_dir() {
            bail!("repository root does not exist: {}", request.root.display());
        }
        let started = Instant::now();
        let prompt = self.prompt(&request);
        let turns = request.max_turns.unwrap_or(self.max_turns).max(1);
        let timeout = request.timeout.unwrap_or(self.timeout);

        let (structured, usage, raw) = match self.provider {
            CliProvider::Codex => {
                let temp = TempRunDir::create()?;
                let schema = temp.path.join("schema.json");
                let output = temp.path.join("result.json");
                std::fs::write(&schema, output_schema().to_string())?;
                let process = self
                    .run_process(
                        self.codex_args(&schema, &output, &prompt),
                        &request.root,
                        timeout,
                    )
                    .await?;
                if !process.status.success() {
                    bail!("{} failed: {}", self.provider.label(), process.error_text());
                }
                let raw = std::fs::read_to_string(&output).with_context(|| {
                    format!("{} returned no structured result", self.provider.label())
                })?;
                let structured: StructuredOutput = serde_json::from_str(&raw)
                    .context("Codex returned malformed structured output")?;
                (structured, None, raw)
            }
            CliProvider::Claude => {
                let process = self
                    .run_process(self.claude_args(turns, &prompt), &request.root, timeout)
                    .await?;
                let envelope: ClaudeEnvelope = serde_json::from_slice(&process.stdout)
                    .context("Claude returned malformed JSON output")?;
                if !process.status.success() || envelope.is_error {
                    let message = envelope
                        .result
                        .clone()
                        .unwrap_or_else(|| process.error_text());
                    bail!("{} failed: {message}", self.provider.label());
                }
                let structured = envelope
                    .structured_output
                    .context("Claude returned no structured output")?;
                let raw = serde_json::to_string(&structured)?;
                (structured, envelope.usage, raw)
            }
        };

        let citations = validate_citations(&request.root, &structured.citations);
        if !structured.citations.is_empty() && citations.is_empty() {
            bail!("{} returned only invalid citations", self.provider.label());
        }
        Ok(ScoutResult {
            summary: structured.answer,
            citations,
            stats: ScoutStats {
                turns: 1,
                tool_calls: 0,
                duration_ms: started.elapsed().as_millis() as u64,
                model: match &self.model {
                    Some(model) => format!("{} ({model})", self.provider.label()),
                    None => self.provider.label().into(),
                },
                prompt_tokens: usage.as_ref().and_then(|usage| usage.input_tokens),
                completion_tokens: usage.as_ref().and_then(|usage| usage.output_tokens),
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
                    self.provider.label(),
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
struct ClaudeEnvelope {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    structured_output: Option<StructuredOutput>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
}

fn output_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "answer": { "type": "string" },
            "citations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "start_line": { "type": "integer", "minimum": 1 },
                        "end_line": { "type": "integer", "minimum": 1 },
                        "reason": { "type": "string" }
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
        let codex = CliScout::from_config(&config("codex-cli", Path::new("codex"))).unwrap();
        let codex_args = codex
            .codex_args(Path::new("schema"), Path::new("output"), "prompt")
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(codex_args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "read-only"]));
        assert!(codex_args.contains(&"--ignore-user-config".into()));
        assert!(codex_args.contains(&"--ignore-rules".into()));
        assert!(codex_args.contains(&"--ephemeral".into()));

        let claude = CliScout::from_config(&config("claude-cli", Path::new("claude"))).unwrap();
        let claude_args = claude
            .claude_args(4, "prompt")
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(claude_args.contains(&"--safe-mode".into()));
        assert_eq!(
            claude_args
                .iter()
                .position(|arg| arg == "--allowedTools")
                .map(|index| claude_args[index + 1].as_str()),
            Some("Read,Glob,Grep")
        );
        assert!(!claude_args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "Bash" | "Edit" | "Write" | "--dangerously-skip-permissions"
            )
        }));
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
