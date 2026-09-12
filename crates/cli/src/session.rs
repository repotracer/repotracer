//! Warm scout sessions.
//!
//! A scout request needs a `codex app-server` subprocess, a JSON-RPC handshake,
//! and a conversation thread. Creating all three per request throws away
//! the two things that make a second question
//! about the same repository cheap: a running process and a conversation the
//! provider's prompt cache has already seen.
//!
//! [`SessionPool`] keeps finished sessions alive, keyed by provider identity
//! and parent conversation ID, and hands them back for the next matching
//! request. Reuse is bounded on every axis that can grow
//! without limit: idle time, warm process count, turns per thread, and
//! accumulated input tokens. An unbounded warm session is a memory leak and an
//! unbounded thread is a token leak.
//!
//! Threads stay `ephemeral`, and a session that errors is destroyed rather
//! than reused. The native process inherits the user's configured Codex home
//! so its normal authentication, tools, skills, plugins and caching remain
//! available.

use anyhow::{bail, Context, Result};
use repotracer_core::{IndexUsage, ScoutBackendError, SessionSettings, UsageStats, UsageStatus};
use repotracer_repo_tools::RepositoryIndex;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
    Lines,
};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant as TokioInstant;

const MAX_CAPTURE_BYTES: usize = 1_048_576;
static SCRATCH_BASE: OnceLock<PathBuf> = OnceLock::new();

/// Report received bytes, not only complete JSON lines. Large native events
/// can arrive in pieces while a turn is still making progress.
struct ActivityReader<R> {
    inner: R,
    activity: mpsc::Sender<()>,
}

impl<R: AsyncRead + Unpin> AsyncRead for ActivityReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            let _ = self.activity.try_send(());
        }
        result
    }
}

/// How to start a provider process. Owned by the caller so the pool stays
/// independent of the backend's configuration types.
#[derive(Clone)]
pub struct SessionSpec {
    pub conversation_id: Option<String>,
    pub provider_identity: u64,
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub root: PathBuf,
    /// Conversation-scoped temporary work. This directory is deliberately
    /// retained after a turn and is exposed to native tools as an additional
    /// workspace root. It is never removed by session idle reaping.
    pub scratch_dir: Option<PathBuf>,
    /// Thread parameters minus `cwd`, which the pool fills from `root`.
    pub thread_params: Value,
    pub developer_instructions: String,
    /// Bound on the handshake and `thread/start` round trips. These are not
    /// model work, so unlike a turn they get a flat deadline rather than one
    /// that resets on activity. Without it a provider that accepts stdin and
    /// never answers hangs the request forever.
    pub startup_timeout: Option<Duration>,
}

/// A single turn's result plus what it cost.
pub struct TurnOutput {
    pub raw: String,
    pub metrics: TurnMetrics,
}

impl SessionSpec {
    fn identity(&self) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        (
            &self.executable,
            &self.args,
            self.provider_identity,
            self.thread_params.to_string(),
        )
            .hash(&mut hash);
        hash.finish()
    }
}

fn canonical_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

/// Allocate a private scratch base with a collision-resistant name and owner-only mode.
fn new_scratch_base() -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("repotracer-investigation-scratch-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir()
        .context("could not create retained investigation scratch base")
}

/// Create (or return) the private process-scoped base for retained scratch.
/// The winning `TempDir` is explicitly kept so conversation artifacts survive
/// session eviction and future replies. A concurrent loser remains temporary
/// and is dropped while still empty.
fn retained_scratch_base() -> Result<PathBuf> {
    if let Some(path) = SCRATCH_BASE.get() {
        return Ok(path.clone());
    }
    let candidate = new_scratch_base()?;
    let path = candidate.path().to_path_buf();
    if SCRATCH_BASE.set(path.clone()).is_ok() {
        return Ok(candidate.keep());
    }
    Ok(SCRATCH_BASE
        .get()
        .expect("scratch base set by competing initializer")
        .clone())
}

/// Return the retained scratch directory for one parent-visible conversation.
///
/// The path is process-scoped and derived from the opaque handle rather than
/// embedding that handle in a filesystem name. It is intentionally not a
/// `TempDir`: scripts and generated evidence must survive a warm-session
/// eviction, a native process restart, and later replies. Cleanup is an
/// explicit lifecycle operation outside the turn/session path.
pub fn conversation_scratch(conversation_id: Option<&str>) -> Result<Option<PathBuf>> {
    let Some(conversation_id) = conversation_id else {
        return Ok(None);
    };
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    conversation_id.hash(&mut hash);
    let base = retained_scratch_base()?;
    let path = base.join(format!("{:016x}", hash.finish()));
    match std::fs::create_dir(&path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(error) =
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                {
                    let _ = std::fs::remove_dir(&path);
                    return Err(error.into());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(&path).with_context(|| {
                format!("could not inspect investigation scratch {}", path.display())
            })?;
            anyhow::ensure!(
                metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
                "investigation scratch path is not a directory: {}",
                path.display()
            );
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("could not create investigation scratch {}", path.display())
            })
        }
    }
    Ok(Some(path))
}

/// A retained process owns one parent conversation slot. Keeping the
/// conversation ID in this key prevents a request for B from borrowing A's
/// thread and silently losing A's history.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionKey {
    process_identity: u64,
    conversation_id: Option<String>,
}

impl SessionKey {
    fn from_spec(spec: &SessionSpec) -> Self {
        Self {
            process_identity: spec.identity(),
            conversation_id: spec.conversation_id.clone(),
        }
    }
}

/// Detect source account/configuration changes without logging or retaining their contents.
pub fn provider_identity() -> Result<u64> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    home.hash(&mut hash);
    if let Some(home) = home {
        for name in ["config.toml", "auth.json"] {
            match std::fs::read(home.join(name)) {
                Ok(bytes) => bytes.hash(&mut hash),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => name.hash(&mut hash),
                Err(error) => {
                    return Err(error).context("could not check Codex source configuration")
                }
            }
        }
    }
    Ok(hash.finish())
}

#[derive(Debug, Clone, Default)]
pub struct TurnMetrics {
    pub usage: Option<TokenUsage>,
    /// Latest cumulative thread usage, retained so a follow-up can subtract
    /// the prior turn rather than charging the whole conversation again.
    pub cumulative_usage: Option<TokenUsage>,
    pub usage_status: UsageStatus,
    pub tool_calls: u32,
    /// 1 on a fresh thread, 2+ when continuing one.
    pub thread_turn: u32,
    /// Whether the provider process was already running when this turn started.
    pub warm_process: bool,
    /// Tree-sitter Symbols work performed during this turn. `None` means the
    /// provider did not expose an index-capable turn (for example, startup
    /// failed before a turn began).
    pub index_usage: Option<IndexUsage>,
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    #[serde(default, alias = "input_tokens")]
    pub input_tokens: Option<u32>,
    #[serde(default, alias = "cached_input_tokens")]
    pub cached_input_tokens: Option<u32>,
    #[serde(default, alias = "output_tokens")]
    pub output_tokens: Option<u32>,
    #[serde(default, alias = "reasoning_output_tokens")]
    pub reasoning_output_tokens: Option<u32>,
    #[serde(
        default,
        alias = "cache_write_input_tokens",
        alias = "cacheCreationInputTokens",
        alias = "cache_creation_input_tokens"
    )]
    pub cache_write_input_tokens: Option<u32>,
    #[serde(default, alias = "total_tokens")]
    pub total_tokens: Option<u32>,
}

impl TokenUsage {
    pub(crate) fn to_usage_stats(&self) -> UsageStats {
        UsageStats {
            input_tokens: self.input_tokens,
            cached_input_tokens: self.cached_input_tokens,
            cache_write_input_tokens: self.cache_write_input_tokens,
            output_tokens: self.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens,
            total_tokens: self.total_tokens,
        }
    }

    fn observed(&self) -> bool {
        self.input_tokens.is_some()
            || self.cached_input_tokens.is_some()
            || self.cache_write_input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.reasoning_output_tokens.is_some()
            || self.total_tokens.is_some()
    }
}

/// Usage snapshots are sent repeatedly while a turn runs. The `total`
/// snapshot is cumulative for the app-server thread, so keep a per-dimension
/// maximum and never sum notifications. A lower value is treated as a reset
/// or out-of-order update and makes the result partial.
#[derive(Default, Clone)]
struct UsageTracker {
    baseline: Option<TokenUsage>,
    latest_last: Option<TokenUsage>,
    cumulative: Option<TokenUsage>,
    regression: bool,
    // Keep tool work outside the driver future so watchdog cancellation retains it.
    tool_calls: u32,
}

impl UsageTracker {
    fn new(baseline: Option<TokenUsage>) -> Self {
        Self {
            baseline,
            ..Default::default()
        }
    }

    fn observe_last(&mut self, usage: TokenUsage) {
        if usage.observed() {
            self.latest_last = Some(usage);
        }
    }

    fn observe_total(&mut self, usage: TokenUsage) {
        if !usage.observed() {
            return;
        }
        if let Some(previous) = &self.cumulative {
            self.regression |= dimensions_regressed(previous, &usage);
            self.cumulative = Some(max_dimensions(previous, &usage));
        } else {
            if let Some(baseline) = &self.baseline {
                self.regression = dimensions_regressed(baseline, &usage);
            }
            self.cumulative = Some(usage);
        }
    }

    fn finish(self, failed: bool) -> TurnMetrics {
        // A successful turn with no cumulative snapshot still advances the
        // conversation. Preserve an explicit empty baseline so a later
        // cumulative total cannot be mistaken for this turn's delta.
        let cumulative_usage = self
            .cumulative
            .clone()
            .or_else(|| Some(TokenUsage::default()));
        let (usage, status) = self.usage_snapshot();
        TurnMetrics {
            usage,
            cumulative_usage,
            tool_calls: self.tool_calls,
            usage_status: if failed {
                match status {
                    UsageStatus::Unknown => UsageStatus::Unknown,
                    _ => UsageStatus::Partial,
                }
            } else {
                status
            },
            ..Default::default()
        }
    }

    /// Read a safe copy while the turn may still be running or may have been
    /// canceled by the idle watchdog. The tracker itself remains available
    /// to the caller for the final cumulative baseline.
    fn snapshot(&self, failed: bool) -> TurnMetrics {
        self.clone().finish(failed)
    }

    fn usage_snapshot(&self) -> (Option<TokenUsage>, UsageStatus) {
        let Some(cumulative) = &self.cumulative else {
            let Some(last) = &self.latest_last else {
                return (None, UsageStatus::Unknown);
            };
            return (Some(last.clone()), UsageStatus::Partial);
        };
        let mut usage = subtract_snapshot(cumulative, self.baseline.as_ref());
        // Older app-server versions omitted detail fields from `total` while
        // still reporting them in `last`. Preserve those provider values for
        // the current turn without ever treating an omission as zero.
        // "last" is a per-turn snapshot, not a cumulative thread total. It is
        // safe as a partial fallback for omitted fields, but those fields did
        // not come from the cumulative accounting path, so the result cannot
        // be Complete.
        let filled_from_last = self
            .latest_last
            .as_ref()
            .is_some_and(|last| fill_missing(&mut usage, last));
        let status = if self.regression || filled_from_last || !all_dimensions_present(&usage) {
            UsageStatus::Partial
        } else {
            UsageStatus::Complete
        };
        (Some(usage), status)
    }
}

fn fill_missing(target: &mut TokenUsage, fallback: &TokenUsage) -> bool {
    let mut filled = false;
    if target.input_tokens.is_none() {
        target.input_tokens = fallback.input_tokens;
        filled |= target.input_tokens.is_some();
    }
    if target.cached_input_tokens.is_none() {
        target.cached_input_tokens = fallback.cached_input_tokens;
        filled |= target.cached_input_tokens.is_some();
    }
    if target.cache_write_input_tokens.is_none() {
        target.cache_write_input_tokens = fallback.cache_write_input_tokens;
        filled |= target.cache_write_input_tokens.is_some();
    }
    if target.output_tokens.is_none() {
        target.output_tokens = fallback.output_tokens;
        filled |= target.output_tokens.is_some();
    }
    if target.reasoning_output_tokens.is_none() {
        target.reasoning_output_tokens = fallback.reasoning_output_tokens;
        filled |= target.reasoning_output_tokens.is_some();
    }
    if target.total_tokens.is_none() {
        target.total_tokens = fallback.total_tokens;
        filled |= target.total_tokens.is_some();
    }
    filled
}

fn all_dimensions_present(usage: &TokenUsage) -> bool {
    usage.input_tokens.is_some()
        && usage.cached_input_tokens.is_some()
        && usage.cache_write_input_tokens.is_some()
        && usage.output_tokens.is_some()
        && usage.reasoning_output_tokens.is_some()
        && usage.total_tokens.is_some()
}

fn dimensions_regressed(previous: &TokenUsage, current: &TokenUsage) -> bool {
    dimension_regressed(previous.input_tokens, current.input_tokens)
        || dimension_regressed(previous.cached_input_tokens, current.cached_input_tokens)
        || dimension_regressed(
            previous.cache_write_input_tokens,
            current.cache_write_input_tokens,
        )
        || dimension_regressed(previous.output_tokens, current.output_tokens)
        || dimension_regressed(
            previous.reasoning_output_tokens,
            current.reasoning_output_tokens,
        )
        || dimension_regressed(previous.total_tokens, current.total_tokens)
}

fn dimension_regressed(previous: Option<u32>, current: Option<u32>) -> bool {
    matches!((previous, current), (Some(previous), Some(current)) if current < previous)
}

fn max_dimensions(previous: &TokenUsage, current: &TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: max_dimension(previous.input_tokens, current.input_tokens),
        cached_input_tokens: max_dimension(
            previous.cached_input_tokens,
            current.cached_input_tokens,
        ),
        cache_write_input_tokens: max_dimension(
            previous.cache_write_input_tokens,
            current.cache_write_input_tokens,
        ),
        output_tokens: max_dimension(previous.output_tokens, current.output_tokens),
        reasoning_output_tokens: max_dimension(
            previous.reasoning_output_tokens,
            current.reasoning_output_tokens,
        ),
        total_tokens: max_dimension(previous.total_tokens, current.total_tokens),
    }
}

fn max_dimension(previous: Option<u32>, current: Option<u32>) -> Option<u32> {
    match (previous, current) {
        (Some(previous), Some(current)) => Some(previous.max(current)),
        (Some(previous), None) => Some(previous),
        (None, current) => current,
    }
}

fn subtract_snapshot(current: &TokenUsage, baseline: Option<&TokenUsage>) -> TokenUsage {
    let has_baseline = baseline.is_some();
    TokenUsage {
        input_tokens: subtract_dimension(
            current.input_tokens,
            baseline.and_then(|v| v.input_tokens),
            has_baseline,
        ),
        cached_input_tokens: subtract_dimension(
            current.cached_input_tokens,
            baseline.and_then(|v| v.cached_input_tokens),
            has_baseline,
        ),
        cache_write_input_tokens: subtract_dimension(
            current.cache_write_input_tokens,
            baseline.and_then(|v| v.cache_write_input_tokens),
            has_baseline,
        ),
        output_tokens: subtract_dimension(
            current.output_tokens,
            baseline.and_then(|v| v.output_tokens),
            has_baseline,
        ),
        reasoning_output_tokens: subtract_dimension(
            current.reasoning_output_tokens,
            baseline.and_then(|v| v.reasoning_output_tokens),
            has_baseline,
        ),
        total_tokens: subtract_dimension(
            current.total_tokens,
            baseline.and_then(|v| v.total_tokens),
            has_baseline,
        ),
    }
}

fn subtract_dimension(
    current: Option<u32>,
    baseline: Option<u32>,
    has_baseline: bool,
) -> Option<u32> {
    match (current, baseline) {
        (Some(current), Some(baseline)) => Some(current.saturating_sub(baseline)),
        (Some(current), None) if !has_baseline => Some(current),
        (Some(_), None) => None,
        (None, _) => None,
    }
}

fn usage_diagnostic(usage: Option<&TokenUsage>, status: UsageStatus) -> Value {
    let usage = usage.map(TokenUsage::to_usage_stats).unwrap_or_default();
    json!({
        "status": status,
        "input_tokens": usage.input_tokens,
        "cached_input_tokens": usage.cached_input_tokens,
        "cache_write_input_tokens": usage.cache_write_input_tokens,
        "output_tokens": usage.output_tokens,
        "reasoning_output_tokens": usage.reasoning_output_tokens,
        "total_tokens": usage.total_tokens,
    })
}

#[derive(Debug)]
pub(crate) struct TurnFailure {
    pub(crate) message: String,
    pub(crate) metrics: TurnMetrics,
}

impl std::fmt::Display for TurnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let diagnostic = usage_diagnostic(self.metrics.usage.as_ref(), self.metrics.usage_status);
        write!(
            formatter,
            "{}; scout usage diagnostic: {}",
            self.message, diagnostic
        )
    }
}

impl std::error::Error for TurnFailure {}

/// Copy metrics out of a failed turn before transport diagnostics are added.
/// The helper keeps `TurnFailure` private to the session implementation while
/// allowing the backend boundary to preserve paid/index work in
/// `ScoutBackendError`.
pub(crate) fn failure_metrics(error: &anyhow::Error) -> Option<TurnMetrics> {
    error
        .downcast_ref::<TurnFailure>()
        .map(|failure| failure.metrics.clone())
}

/// Whether a turn finished, or the provider went silent for too long.
enum TurnOutcome {
    Finished(Result<TurnOutput>),
    IdleTimeout(Duration),
}

struct ActiveThread {
    conversation_id: Option<String>,
    id: String,
    /// Native tools use this as the current target until the next turn
    /// overrides it. It is retained separately from the process key so a
    /// related conversation may move between checkouts.
    root: PathBuf,
    turns: u32,
    last_input_tokens: Option<u32>,
    cumulative_usage: Option<TokenUsage>,
}

fn request_baseline(thread: &ActiveThread) -> Option<TokenUsage> {
    // None means a genuinely fresh thread. After an unreported turn, an empty
    // snapshot means the baseline is unknown, not zero. Do not charge an older
    // turn's tokens to the next request or reuse stale per-dimension baselines.
    if thread.turns == 0 {
        None
    } else {
        Some(thread.cumulative_usage.clone().unwrap_or_default())
    }
}

/// A live provider process that has completed its handshake.
pub struct WarmSession {
    threads_created: u32,
    key: SessionKey,
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ActivityReader<ChildStdout>>>,
    stderr: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    activity_tx: mpsc::Sender<()>,
    activity_rx: mpsc::Receiver<()>,
    process_group: Option<u32>,
    /// Retained across replies for the parent conversation. This directory is
    /// deliberately independent of the provider home and is never removed by
    /// session eviction.
    scratch_dir: Option<PathBuf>,
    next_request_id: u64,
    thread: Option<ActiveThread>,
    /// False only for the request that spawned this process.
    was_warm: bool,
    last_used: Instant,
}

impl WarmSession {
    /// Spawn a provider process and complete the `initialize` handshake.
    async fn spawn(spec: &SessionSpec) -> Result<Self> {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .current_dir(&spec.root)
            .env("REPOTRACER_SUBPROCESS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start `{}`", spec.executable.display()))?;
        let process_group = child.id();
        let stdin = child.stdin.take().context("missing provider stdin")?;
        let stdout = child.stdout.take().context("missing provider stdout")?;
        let stderr = child.stderr.take().context("missing provider stderr")?;
        let (activity_tx, activity_rx) = mpsc::channel(1);
        let stderr = tokio::spawn(drain_limited(
            stderr,
            MAX_CAPTURE_BYTES,
            activity_tx.clone(),
        ));

        let mut session = Self {
            threads_created: 0,
            key: SessionKey::from_spec(spec),
            child,
            stdin,
            lines: BufReader::new(ActivityReader {
                inner: stdout,
                activity: activity_tx.clone(),
            })
            .lines(),
            stderr: Some(stderr),
            activity_tx,
            activity_rx,
            process_group,
            scratch_dir: spec.scratch_dir.clone(),
            next_request_id: 1,
            thread: None,
            was_warm: false,
            last_used: Instant::now(),
        };
        match bounded(spec.startup_timeout, session.handshake()).await {
            Ok(Ok(())) => Ok(session),
            Ok(Err(error)) => Err(attach_stderr(error, session.shutdown().await)),
            Err(limit) => Err(attach_stderr(
                startup_timeout_error(limit, "handshake"),
                session.shutdown().await,
            )),
        }
    }

    async fn handshake(&mut self) -> Result<()> {
        let id = self.take_request_id();
        send_message(
            &mut self.stdin,
            &json!({"id": id, "method": "initialize", "params": {
                "clientInfo": {"name": "repotracer", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true}
            }}),
        )
        .await?;
        wait_for_response(&mut self.stdin, &mut self.lines, id, &self.activity_tx).await?;
        send_message(
            &mut self.stdin,
            &json!({"method": "initialized", "params": {}}),
        )
        .await
    }

    /// Turns already completed on the current thread. Zero means the next turn
    /// is the first, so the caller still owes the full instructions.
    pub fn thread_turns(&self) -> u32 {
        self.thread.as_ref().map_or(0, |thread| thread.turns)
    }

    /// Current native target for the active thread, if one has started.
    pub fn thread_target(&self) -> Option<&Path> {
        self.thread.as_ref().map(|thread| thread.root.as_path())
    }

    fn workspace_roots(&self, target: &Path) -> Value {
        let mut roots = vec![json!(target)];
        if let Some(scratch) = &self.scratch_dir {
            if scratch != target {
                roots.push(json!(scratch));
            }
        }
        Value::Array(roots)
    }

    fn take_request_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    /// Whether the existing thread may take another turn under `settings`.
    fn thread_is_reusable(&self, settings: &SessionSettings) -> bool {
        self.thread.as_ref().is_some_and(|thread| {
            settings.thread_has_turns_left(thread.turns)
                && settings.thread_within_input_budget(thread.last_input_tokens)
        })
    }

    /// Start a thread unless a reusable one is already open.
    async fn ensure_thread(
        &mut self,
        spec: &SessionSpec,
        settings: &SessionSettings,
    ) -> Result<()> {
        anyhow::ensure!(
            self.child.try_wait()?.is_none(),
            "idle provider process exited"
        );
        if self.thread_is_reusable(settings)
            && spec.conversation_id.is_some()
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| thread.conversation_id == spec.conversation_id)
        {
            return Ok(());
        }
        // A thread that ran out of budget is abandoned, not resumed. The
        // process stays warm, so this costs one round trip rather than a spawn.
        anyhow::ensure!(
            self.threads_created < settings.max_process_threads.max(1),
            "provider process reached its thread limit"
        );
        self.thread = None;

        let mut params = spec.thread_params.clone();
        let table = params
            .as_object_mut()
            .context("thread parameters must be a JSON object")?;
        table.insert("cwd".into(), json!(spec.root));
        table.insert(
            "runtimeWorkspaceRoots".into(),
            self.workspace_roots(&spec.root),
        );
        // Use the native full-access mode requested for investigations. The
        // mandate, rather than a RepoTracer tool allowlist or a second
        // sandbox, keeps source edits focused on the supplied target/scratch
        // locations.
        table.insert("sandbox".into(), json!("danger-full-access"));
        table.insert(
            "developerInstructions".into(),
            json!(spec.developer_instructions),
        );

        let id = self.take_request_id();
        send_message(
            &mut self.stdin,
            &json!({"id": id, "method": "thread/start", "params": params}),
        )
        .await?;
        let started =
            wait_for_response(&mut self.stdin, &mut self.lines, id, &self.activity_tx).await?;
        let thread_id = started["thread"]["id"]
            .as_str()
            .context("Codex app-server returned no thread id")?;
        self.thread = Some(ActiveThread {
            conversation_id: spec.conversation_id.clone(),
            id: thread_id.to_string(),
            root: canonical_root(&spec.root),
            turns: 0,
            last_input_tokens: None,
            cumulative_usage: None,
        });
        self.threads_created += 1;
        Ok(())
    }

    /// Run one turn. `idle_timeout` bounds silence, not total duration: any
    /// provider output resets it, so a slow-but-working scout is not killed.
    pub async fn turn(
        &mut self,
        prompt: &str,
        effort: &str,
        output_schema: Value,
        target: &Path,
        idle_timeout: Option<Duration>,
        index: &RepositoryIndex,
    ) -> Result<TurnOutput> {
        let thread = self.thread.as_ref().context("no active thread")?;
        let thread_id = thread.id.clone();
        let thread_turn = thread.turns + 1;
        let warm_process = self.was_warm;
        let mut usage = UsageTracker::new(request_baseline(thread));
        let mut index_usage = IndexUsage {
            available: true,
            ..Default::default()
        };

        let id = self.take_request_id();
        let start_message = json!({"id": id, "method": "turn/start", "params": {
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt}],
            "effort": effort,
            "cwd": target,
            "runtimeWorkspaceRoots": self.workspace_roots(target),
            "outputSchema": output_schema
        }});

        // Field-wise borrows so the idle watchdog can read `activity_rx` while
        // the turn holds `stdin` and `lines`.
        let Self {
            stdin,
            lines,
            activity_tx,
            activity_rx,
            ..
        } = self;

        let outcome = {
            let driver = async {
                // A native process that stops reading stdin must not bypass
                // the same inactivity watchdog used while awaiting output.
                send_message(stdin, &start_message).await?;
                drive_turn(
                    stdin,
                    lines,
                    id,
                    activity_tx,
                    index,
                    &mut usage,
                    &mut index_usage,
                )
                .await
            };
            tokio::pin!(driver);
            match idle_timeout {
                None => TurnOutcome::Finished(driver.as_mut().await),
                Some(limit) => {
                    let deadline = tokio::time::sleep_until(TokioInstant::now() + limit);
                    tokio::pin!(deadline);
                    let mut activity_open = true;
                    loop {
                        tokio::select! {
                            biased;
                            result = &mut driver => break TurnOutcome::Finished(result),
                            activity = activity_rx.recv(), if activity_open => match activity {
                                Some(()) => deadline.as_mut().reset(TokioInstant::now() + limit),
                                None => activity_open = false,
                            },
                            _ = &mut deadline => break TurnOutcome::IdleTimeout(limit),
                        }
                    }
                }
            }
        };

        let mut output = match outcome {
            TurnOutcome::Finished(Ok(output)) => output,
            TurnOutcome::Finished(Err(error)) => {
                if let Some(failure) = error.downcast_ref::<TurnFailure>() {
                    let mut metrics = failure.metrics.clone();
                    metrics.thread_turn = thread_turn;
                    metrics.warm_process = warm_process;
                    return Err(TurnFailure {
                        message: failure.message.clone(),
                        metrics,
                    }
                    .into());
                }
                return Err(error);
            }
            TurnOutcome::IdleTimeout(limit) => {
                let mut metrics = usage.snapshot(true);
                metrics.index_usage = Some(index_usage.clone());
                metrics.thread_turn = thread_turn;
                metrics.warm_process = warm_process;
                return Err(TurnFailure {
                    message: format!("scout produced no output for {}s", limit.as_secs_f32()),
                    metrics,
                }
                .into());
            }
        };

        if let Some(thread) = self.thread.as_mut() {
            thread.root = canonical_root(target);
            thread.turns = thread_turn;
            thread.last_input_tokens = output
                .metrics
                .cumulative_usage
                .as_ref()
                .and_then(|usage| usage.input_tokens);
            thread.cumulative_usage = output.metrics.cumulative_usage.clone();
        }
        output.metrics.thread_turn = thread_turn;
        output.metrics.warm_process = warm_process;
        self.last_used = Instant::now();
        Ok(output)
    }

    /// Kill the process tree and return whatever the provider wrote to stderr.
    pub async fn shutdown(mut self) -> Vec<u8> {
        let stderr = self.stderr.take();
        // Signal EOF before waiting. `WarmSession` implements `Drop` so the
        // field cannot be moved out here; an orderly shutdown is equivalent
        // and the Drop path still force-kills the process group if needed.
        let _ = self.stdin.shutdown().await;
        let _ = tokio::time::timeout(Duration::from_millis(250), self.child.wait()).await;
        kill_process_tree(&mut self.child, self.process_group).await;
        // Explicit cleanup already handled this group. Do not send another
        // signal from Drop after the process ID is available for reuse.
        self.process_group = None;
        match stderr {
            Some(task) => task.await.ok().and_then(Result::ok).unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

impl Drop for WarmSession {
    fn drop(&mut self) {
        // MCP cancellation drops the handler future while it is awaiting a
        // native turn. In that path `CliScout::run` cannot reach
        // `shutdown().await`; kill the process group synchronously so shell
        // wrappers and other descendants do not outlive the cancelled call.
        if let Some(process_group) = self.process_group {
            kill_process_group(process_group);
        }
        // `kill_on_drop` covers the direct child on supported Tokio targets;
        // start_kill is also useful when a child has been detached from its
        // Tokio bookkeeping but still has a live handle here.
        let _ = self.child.start_kill();
    }
}

/// Read provider messages until the turn completes.
async fn drive_turn<R, W>(
    stdin: &mut W,
    lines: &mut Lines<R>,
    turn_request_id: u64,
    activity: &mpsc::Sender<()>,
    index: &RepositoryIndex,
    usage: &mut UsageTracker,
    index_usage: &mut IndexUsage,
) -> Result<TurnOutput>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    match drive_turn_inner(
        stdin,
        lines,
        turn_request_id,
        activity,
        index,
        usage,
        index_usage,
    )
    .await
    {
        Ok(output) => Ok(output),
        Err(error) if error.downcast_ref::<TurnFailure>().is_some() => Err(error),
        Err(error) => {
            let mut metrics = usage.snapshot(true);
            metrics.index_usage = Some(index_usage.clone());
            Err(TurnFailure {
                message: format!("{error:#}"),
                metrics,
            }
            .into())
        }
    }
}

async fn drive_turn_inner<R, W>(
    stdin: &mut W,
    lines: &mut Lines<R>,
    turn_request_id: u64,
    activity: &mpsc::Sender<()>,
    index: &RepositoryIndex,
    usage: &mut UsageTracker,
    index_usage: &mut IndexUsage,
) -> Result<TurnOutput>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    wait_for_response(stdin, lines, turn_request_id, activity).await?;

    let mut raw = None;
    let mut metrics = TurnMetrics {
        index_usage: Some(index_usage.clone()),
        ..Default::default()
    };
    loop {
        let message = next_message(lines, activity).await?;
        if message.get("id").is_some() && message.get("method").is_some() {
            if message["method"] == "item/tool/call" && message["params"]["tool"] == "Symbols" {
                usage.tool_calls = usage.tool_calls.saturating_add(1);
                index_usage.calls = index_usage.calls.saturating_add(1);
                let result = index
                    .call_with_metrics(&message["params"]["arguments"].to_string())
                    .await;
                let success = result.is_ok();
                let output = match result {
                    Ok((
                        output,
                        parsed_files,
                        reused_files,
                        incomplete,
                        duration_ms,
                        output_bytes,
                    )) => {
                        add_index_call_metrics(
                            index_usage,
                            parsed_files,
                            reused_files,
                            incomplete,
                            duration_ms,
                            output_bytes,
                        );
                        output
                    }
                    Err(error) => {
                        index_usage.failed_calls = index_usage.failed_calls.saturating_add(1);
                        format!("Symbols failed: {error}. Use bounded text search instead.")
                    }
                };
                send_message(
                    stdin,
                    &json!({"id":message["id"], "result":{
                        "success":success, "contentItems":[{"type":"inputText", "text":output}]
                    }}),
                )
                .await?;
            } else {
                reject_server_request(stdin, &message).await?;
            }
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
                        usage.tool_calls = usage.tool_calls.saturating_add(1);
                    }
                    _ => {}
                }
            }
            Some("thread/tokenUsage/updated") => {
                let token_usage = &message["params"]["tokenUsage"];
                if let Some(last) = token_usage
                    .get("last")
                    .and_then(|value| serde_json::from_value(value.clone()).ok())
                {
                    usage.observe_last(last);
                }
                if let Some(total) = token_usage
                    .get("total")
                    .and_then(|value| serde_json::from_value(value.clone()).ok())
                {
                    usage.observe_total(total);
                }
            }
            Some("turn/completed") => {
                let turn = &message["params"]["turn"];
                if turn["status"] != "completed" {
                    let error = turn["error"]["message"]
                        .as_str()
                        .unwrap_or("Codex turn did not complete");
                    let usage_metrics = usage.snapshot(true);
                    let mut failure_metrics = usage_metrics;
                    failure_metrics.index_usage = Some(index_usage.clone());
                    return Err(TurnFailure {
                        message: error.to_string(),
                        metrics: failure_metrics,
                    }
                    .into());
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
                let usage_metrics = usage.snapshot(false);
                metrics.tool_calls = usage_metrics.tool_calls;
                metrics.usage = usage_metrics.usage;
                metrics.cumulative_usage = usage_metrics.cumulative_usage;
                metrics.usage_status = usage_metrics.usage_status;
                metrics.index_usage = Some(index_usage.clone());
                return Ok(TurnOutput {
                    raw: raw.context("Codex app-server returned no structured result")?,
                    metrics,
                });
            }
            Some("error") if !message["params"]["willRetry"].as_bool().unwrap_or(false) => {
                let usage_metrics = usage.snapshot(true);
                let mut failure_metrics = usage_metrics;
                failure_metrics.index_usage = Some(index_usage.clone());
                return Err(TurnFailure {
                    message: message["params"]["error"]["message"]
                        .as_str()
                        .unwrap_or("Codex app-server turn failed")
                        .to_string(),
                    metrics: failure_metrics,
                }
                .into());
            }
            _ => {}
        }
    }
}

fn add_index_call_metrics(
    usage: &mut IndexUsage,
    parsed_files: u64,
    reused_files: u64,
    incomplete: bool,
    duration_ms: u64,
    output_bytes: u64,
) {
    usage.parsed_files = usage.parsed_files.saturating_add(parsed_files);
    usage.reused_files = usage.reused_files.saturating_add(reused_files);
    usage.duration_ms = usage.duration_ms.saturating_add(duration_ms);
    usage.output_bytes = usage.output_bytes.saturating_add(output_bytes);
    if incomplete {
        usage.incomplete_calls = usage.incomplete_calls.saturating_add(1);
    }
}

/// Bounded idle provider processes. Active requests own their sessions exclusively.
pub struct SessionPool {
    settings: SessionSettings,
    idle: Mutex<Vec<WarmSession>>,
}

impl SessionPool {
    pub fn new(settings: SessionSettings) -> Arc<Self> {
        Arc::new(Self {
            settings,
            idle: Mutex::new(Vec::new()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<WarmSession>> {
        // A panic inside a pool operation must not disable reuse for the rest
        // of the process; the vector itself is always structurally valid.
        self.idle.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take a session for `spec`, reusing a warm one when possible.
    ///
    /// The returned session is owned by the caller and is *not* in the pool;
    /// it must be passed to [`SessionPool::release`] to be reused, or to
    /// [`WarmSession::shutdown`] to be destroyed. Errors must never be
    /// released — a session whose turn failed may have unread protocol frames
    /// queued, so the next turn on it would desynchronize.
    pub async fn acquire(self: &Arc<Self>, spec: &SessionSpec) -> Result<WarmSession> {
        self.retire_expired();
        if let Some(mut warm) = self.reuse(spec) {
            warm.was_warm = true;
            match self.start_thread(&mut warm, spec).await {
                Ok(()) => return Ok(warm),
                // A process that sat idle may have been killed by the OS or
                // exited on its own. Discovering that must cost this request a
                // respawn, not a failure — otherwise one dead warm process
                // breaks every request until the idle timeout clears it.
                Err(_) => {
                    warm.shutdown().await;
                }
            }
        }
        let mut fresh = WarmSession::spawn(spec).await?;
        match self.start_thread(&mut fresh, spec).await {
            Ok(()) => Ok(fresh),
            Err(error) => Err(attach_stderr(error, fresh.shutdown().await)),
        }
    }

    async fn start_thread(&self, session: &mut WarmSession, spec: &SessionSpec) -> Result<()> {
        match bounded(
            spec.startup_timeout,
            session.ensure_thread(spec, &self.settings),
        )
        .await
        {
            Ok(result) => result,
            Err(limit) => Err(startup_timeout_error(limit, "thread start")),
        }
    }

    fn reuse(&self, spec: &SessionSpec) -> Option<WarmSession> {
        if !self.settings.reuses_process() {
            return None;
        }
        let mut idle = self.lock();
        let key = SessionKey::from_spec(spec);
        // Keep the existing configuration-fingerprint behavior: a changed
        // provider identity retires every old process before it can serve a
        // request. The target root is intentionally absent from this key;
        // native turns can override cwd and workspace roots per request.
        let mut obsolete = Vec::new();
        let mut index = 0;
        while index < idle.len() {
            if idle[index].key.process_identity != key.process_identity {
                obsolete.push(idle.swap_remove(index));
            } else {
                index += 1;
            }
        }
        let reused = idle
            .iter()
            .position(|session| session.key == key)
            .map(|index| idle.swap_remove(index));
        drop(idle);
        for session in obsolete {
            retire(session);
        }
        reused
    }

    /// Return a healthy session to the pool, or destroy it if reuse is off or
    /// the pool is full.
    pub fn release(self: &Arc<Self>, session: WarmSession) {
        if !self.settings.reuses_process() {
            retire(session);
            return;
        }
        let evicted = {
            let mut idle = self.lock();
            // Keep one retained process per provider identity and parent
            // conversation. Native turns may retarget that process to another
            // checkout, while a different named conversation must never
            // replace this one.
            let key = session.key.clone();
            let mut evicted: Vec<WarmSession> = Vec::new();
            while let Some(index) = idle.iter().position(|other| other.key == key) {
                evicted.push(idle.swap_remove(index));
            }
            idle.push(session);
            while idle.len() > self.settings.max_warm {
                // Oldest first, so recently used conversation slots survive.
                let oldest = idle
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, session)| session.last_used)
                    .map(|(index, _)| index);
                match oldest {
                    Some(index) => evicted.push(idle.swap_remove(index)),
                    None => break,
                }
            }
            evicted
        };
        for session in evicted {
            retire(session);
        }
    }

    fn retire_expired(&self) {
        let idle_timeout = self.settings.idle_timeout();
        let expired = {
            let mut idle = self.lock();
            let mut expired = Vec::new();
            let mut index = 0;
            while index < idle.len() {
                if idle[index].last_used.elapsed() >= idle_timeout {
                    expired.push(idle.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            expired
        };
        for session in expired {
            retire(session);
        }
    }

    /// Reap idle sessions in the background so a long-lived MCP server does not
    /// hold a provider process for hours after the last question. Exits once
    /// the pool's last strong reference is dropped.
    pub fn spawn_reaper(pool: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let idle_timeout = pool.settings.idle_timeout();
        if !pool.settings.reuses_process() {
            return;
        }
        let weak = Arc::downgrade(pool);
        tokio::spawn(async move {
            let period = idle_timeout
                .min(Duration::from_secs(30))
                .max(Duration::from_secs(1));
            let mut ticker = tokio::time::interval(period);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                match Weak::upgrade(&weak) {
                    Some(pool) => pool.retire_expired(),
                    None => return,
                }
            }
        });
    }
}

impl Drop for SessionPool {
    fn drop(&mut self) {
        // `kill_on_drop` handles the direct children; descendants are only
        // reachable through the process group, which needs a runtime.
        let sessions: Vec<WarmSession> = self.lock().drain(..).collect();
        for session in sessions {
            retire(session);
        }
    }
}

/// Destroy a session without blocking the caller.
fn retire(session: WarmSession) {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(async move {
            session.shutdown().await;
        });
    }
    // Without a runtime, dropping the session relies on `kill_on_drop`.
}

/// Run `future` under an optional deadline. `Err(limit)` means it expired.
async fn bounded<T>(
    limit: Option<Duration>,
    future: impl std::future::Future<Output = T>,
) -> std::result::Result<T, Duration> {
    match limit {
        None => Ok(future.await),
        Some(limit) => tokio::time::timeout(limit, future).await.map_err(|_| limit),
    }
}

fn startup_timeout_error(limit: Duration, stage: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "scout produced no output for {}s during {stage}",
        limit.as_secs_f32()
    )
}

/// Fold whatever the provider wrote to stderr into `error`, so a failure
/// reports the provider's own diagnosis and not just our view of it.
pub fn attach_stderr(error: anyhow::Error, stderr: Vec<u8>) -> anyhow::Error {
    let compact = String::from_utf8_lossy(&stderr)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if compact.is_empty() {
        return error;
    }
    let stderr = compact.chars().take(500).collect::<String>();
    if let Some(failure) = error.downcast_ref::<TurnFailure>() {
        return TurnFailure {
            message: format!("{}; provider stderr: {stderr}", failure.message),
            metrics: failure.metrics.clone(),
        }
        .into();
    }
    if let Some(failure) = error.downcast_ref::<ScoutBackendError>() {
        return ScoutBackendError::new(
            format!("{}; provider stderr: {stderr}", failure.message),
            failure.stats.clone(),
        )
        .into();
    }
    anyhow::anyhow!("{error:#}; provider stderr: {stderr}")
}

pub async fn wait_for_response<R, W>(
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

async fn kill_process_tree(child: &mut Child, process_group: Option<u32>) {
    if let Some(pid) = process_group {
        kill_process_group(pid);
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[cfg(unix)]
pub(crate) fn kill_process_group(process_group: u32) {
    // The child starts a new process group, so a negative PID targets it and
    // its descendants. A stale group simply returns ESRCH.
    unsafe {
        let _ = libc::kill(-(process_group as i32), libc::SIGKILL);
    }
}

#[cfg(windows)]
pub(crate) fn kill_process_group(process_group: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &process_group.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_base_allocations_are_unique_and_preserve_prior_files() {
        let first = new_scratch_base().unwrap();
        let marker = first.path().join("prior-result.txt");
        std::fs::write(&marker, "retained").unwrap();

        let second = new_scratch_base().unwrap();

        assert_ne!(first.path(), second.path());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "retained");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                first.path().metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                second.path().metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn conversation_scratch_is_stable_and_retained() {
        let first = conversation_scratch(Some("scratch-stable"))
            .unwrap()
            .unwrap();
        let second = conversation_scratch(Some("scratch-stable"))
            .unwrap()
            .unwrap();
        let other = conversation_scratch(Some("scratch-other"))
            .unwrap()
            .unwrap();
        assert_eq!(first, second);
        assert_ne!(first, other);
        std::fs::write(first.join("result.txt"), "retained").unwrap();
        assert_eq!(
            std::fs::read_to_string(first.join("result.txt")).unwrap(),
            "retained"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(first.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[tokio::test]
    async fn incomplete_json_line_reports_stream_activity() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let (activity, mut seen) = mpsc::channel(1);
        let reader = ActivityReader {
            inner: reader,
            activity,
        };
        let mut lines = BufReader::new(reader).lines();
        writer.write_all(b"{\"method\":").await.unwrap();
        let next = lines.next_line();
        tokio::pin!(next);
        tokio::select! {
            result = &mut next => panic!("partial line completed: {result:?}"),
            event = seen.recv() => assert_eq!(event, Some(())),
            _ = tokio::time::sleep(Duration::from_secs(1)) => panic!("partial bytes did not signal activity"),
        }
        writer.write_all(b"\"progress\"}\n").await.unwrap();
        assert_eq!(next.await.unwrap().unwrap(), "{\"method\":\"progress\"}");
    }

    fn usage(input: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: Some(input),
            cached_input_tokens: Some(input / 2),
            output_tokens: Some(input / 4),
            reasoning_output_tokens: Some(input / 8),
            cache_write_input_tokens: Some(input / 10),
            total_tokens: Some(input + input / 4),
        }
    }

    fn settings() -> SessionSettings {
        SessionSettings::default()
    }

    #[test]
    fn repeated_cumulative_updates_are_not_summed() {
        let mut tracker = UsageTracker::new(None);
        tracker.observe_total(usage(100));
        tracker.observe_total(usage(140));
        let metrics = tracker.finish(false);
        let reported = metrics.usage.unwrap();
        assert_eq!(reported.input_tokens, Some(140));
        assert_eq!(reported.cached_input_tokens, Some(70));
        assert_eq!(reported.output_tokens, Some(35));
        assert_eq!(metrics.usage_status, UsageStatus::Complete);
    }

    #[test]
    fn followup_reports_cumulative_difference() {
        let baseline = usage(100);
        let mut tracker = UsageTracker::new(Some(baseline));
        tracker.observe_total(usage(100));
        tracker.observe_total(usage(145));
        let metrics = tracker.finish(false);
        let reported = metrics.usage.unwrap();
        assert_eq!(reported.input_tokens, Some(45));
        assert_eq!(reported.cached_input_tokens, Some(22));
        assert_eq!(reported.output_tokens, Some(11));
        assert_eq!(reported.total_tokens, Some(56));
    }

    #[test]
    fn cumulative_regression_does_not_create_negative_usage() {
        let mut tracker = UsageTracker::new(Some(usage(100)));
        tracker.observe_total(usage(130));
        tracker.observe_total(usage(80));
        let metrics = tracker.finish(false);
        assert_eq!(metrics.usage.unwrap().input_tokens, Some(30));
        assert_eq!(metrics.usage_status, UsageStatus::Partial);
    }

    #[test]
    fn followup_cumulative_reset_is_partial_not_zero_cost() {
        let mut tracker = UsageTracker::new(Some(usage(100)));
        tracker.observe_total(usage(80));
        let metrics = tracker.finish(false);
        assert_eq!(metrics.usage.unwrap().input_tokens, Some(0));
        assert_eq!(metrics.usage_status, UsageStatus::Partial);
    }

    #[test]
    fn unknown_baseline_dimension_is_not_counted_as_a_new_total() {
        let mut baseline = usage(100);
        baseline.cached_input_tokens = None;
        let mut tracker = UsageTracker::new(Some(baseline));
        tracker.observe_total(usage(150));
        let metrics = tracker.finish(false);
        assert_eq!(metrics.usage.unwrap().cached_input_tokens, None);
        assert_eq!(metrics.usage_status, UsageStatus::Partial);
    }

    #[test]
    fn absent_usage_is_unknown_and_not_zero() {
        let metrics = UsageTracker::new(None).finish(false);
        assert_eq!(metrics.usage_status, UsageStatus::Unknown);
        assert!(metrics.usage.is_none());
    }

    #[test]
    fn latest_last_snapshot_is_used_when_total_is_missing() {
        let mut tracker = UsageTracker::new(None);
        tracker.observe_last(usage(20));
        tracker.observe_last(usage(35));
        let metrics = tracker.finish(false);
        assert_eq!(metrics.usage.unwrap().input_tokens, Some(35));
        assert_eq!(metrics.usage_status, UsageStatus::Partial);
    }

    #[test]
    fn failed_turn_keeps_safe_partial_usage_diagnostic() {
        let mut tracker = UsageTracker::new(None);
        tracker.observe_total(usage(40));
        let metrics = tracker.finish(true);
        let failure = TurnFailure {
            message: "provider stopped".into(),
            metrics,
        };
        let text = failure.to_string();
        assert!(text.contains("scout usage diagnostic"));
        assert!(text.contains("partial"));
        assert!(text.contains("40"));
        assert!(!text.contains("provider prompt"));
    }

    #[test]
    fn attach_stderr_preserves_failed_turn_metrics() {
        let metrics = TurnMetrics {
            index_usage: Some(IndexUsage {
                available: true,
                calls: 1,
                failed_calls: 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        let error: anyhow::Error = TurnFailure {
            message: "turn failed".into(),
            metrics,
        }
        .into();
        let error = attach_stderr(error, b"provider failed\n".to_vec());
        let failure = error.downcast_ref::<TurnFailure>().unwrap();
        assert_eq!(failure.metrics.index_usage.as_ref().unwrap().calls, 1);
        assert_eq!(
            failure.metrics.index_usage.as_ref().unwrap().failed_calls,
            1
        );
        assert!(failure.to_string().contains("provider failed"));
    }

    #[test]
    fn attach_stderr_preserves_backend_error_stats() {
        let stats = repotracer_core::ScoutStats {
            index_usage: Some(IndexUsage {
                available: true,
                calls: 2,
                ..Default::default()
            }),
            ..Default::default()
        };
        let error: anyhow::Error = ScoutBackendError::new("turn failed", stats).into();
        let error = attach_stderr(error, b"provider failed\n".to_vec());
        let failure = error.downcast_ref::<ScoutBackendError>().unwrap();
        assert_eq!(failure.stats.index_usage.as_ref().unwrap().calls, 2);
        assert!(failure.to_string().contains("provider failed"));
    }

    #[test]
    fn missing_turn_baseline_never_charges_historical_usage_as_current() {
        let thread = ActiveThread {
            conversation_id: None,
            id: "t".into(),
            root: PathBuf::from("/tmp/repotracer-test"),
            turns: 2,
            last_input_tokens: None,
            cumulative_usage: None,
        };
        let mut tracker = UsageTracker::new(request_baseline(&thread));
        tracker.observe_total(usage(150));
        let metrics = tracker.finish(false);
        assert_eq!(metrics.usage.unwrap().input_tokens, None);
        assert_eq!(metrics.usage_status, UsageStatus::Partial);
        // Once the current boundary is known, the next request has an exact delta.
        let mut next = UsageTracker::new(metrics.cumulative_usage);
        next.observe_total(usage(180));
        assert_eq!(next.finish(false).usage.unwrap().input_tokens, Some(30));
    }

    #[test]
    fn fields_filled_from_last_remain_partial() {
        let mut total = usage(100);
        total.cached_input_tokens = None;
        let mut tracker = UsageTracker::new(None);
        tracker.observe_total(total);
        tracker.observe_last(usage(100));
        let metrics = tracker.finish(false);
        assert_eq!(metrics.usage.unwrap().cached_input_tokens, Some(50));
        assert_eq!(metrics.usage_status, UsageStatus::Partial);
    }

    #[tokio::test]
    async fn terminal_paths_preserve_completed_tools() {
        for tool in ["commandExecution", "mcpToolCall", "webSearch", "Symbols"] {
            for ending in [
                json!({"method":"turn/completed","params":{"turn":{"status":"completed","items":[{"type":"agentMessage","text":"{}"}]}}}).to_string(),
                json!({"method":"turn/completed","params":{"turn":{"status":"failed","error":{"message":"fixture failure"}}}}).to_string(),
                json!({"method":"error","params":{"willRetry":false,"error":{"message":"fixture failure"}}}).to_string(),
                String::new(),
                "invalid json".into(),
            ] {
                let root = tempfile::tempdir().unwrap();
                std::fs::write(root.path().join("source.rs"), "fn alpha() {}\n").unwrap();
                let index = RepositoryIndex::new(root.path().to_owned());
                let mut input = "{\"id\":1,\"result\":{}}\n".to_owned();
                for id in [2, 3] {
                    let event = if tool == "Symbols" {
                        json!({"id":id,"method":"item/tool/call","params":{"tool":"Symbols","arguments":{"symbol":"alpha"}}})
                    } else {
                        json!({"method":"item/completed","params":{"item":{"id":id.to_string(),"type":tool}}})
                    };
                    input.push_str(&format!("{event}\n"));
                }
                input.push_str("{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"tokenUsage\":{\"total\":{\"inputTokens\":40}}}}\n");
                input.push_str(&ending);
                input.push('\n');
                let mut lines = BufReader::new(input.as_bytes()).lines();
                let mut stdin = tokio::io::sink();
                let (activity, _) = mpsc::channel(1);
                let mut tracker = UsageTracker::new(None);
                let mut index_usage = IndexUsage { available: true, ..Default::default() };
                let outcome = drive_turn(&mut stdin, &mut lines, 1, &activity, &index, &mut tracker, &mut index_usage).await;
                assert_eq!(outcome.is_ok(), ending.contains("agentMessage"), "{tool}: {ending}");
                let metrics = match outcome {
                    Ok(output) => output.metrics,
                    Err(error) => failure_metrics(&error).expect("typed failure metrics"),
                };
                assert_eq!(metrics.tool_calls, 2, "{tool}: {ending}");
                assert_eq!(metrics.usage.unwrap().input_tokens, Some(40));
                assert_eq!(metrics.index_usage.unwrap().calls, if tool == "Symbols" { 2 } else { 0 });
            }
        }
    }

    #[tokio::test]
    async fn eof_preserves_observed_usage() {
        let root = tempfile::tempdir().unwrap();
        let index = RepositoryIndex::new(root.path().to_path_buf());
        let input = concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"tokenUsage\":{\"total\":{\"inputTokens\":40}}}}\n",
        );
        let mut lines = BufReader::new(input.as_bytes()).lines();
        let mut stdin = tokio::io::sink();
        let (activity, _events) = mpsc::channel(1);
        let mut tracker = UsageTracker::new(None);
        let mut index_usage = IndexUsage {
            available: true,
            ..Default::default()
        };
        let error = drive_turn(
            &mut stdin,
            &mut lines,
            1,
            &activity,
            &index,
            &mut tracker,
            &mut index_usage,
        )
        .await
        .err()
        .expect("EOF must fail");
        let failure = error.downcast_ref::<TurnFailure>().unwrap();
        assert_eq!(
            failure.metrics.usage.as_ref().unwrap().input_tokens,
            Some(40)
        );
        assert_eq!(failure.metrics.usage_status, UsageStatus::Partial);
    }

    #[tokio::test]
    async fn dropping_idle_driver_preserves_observed_usage() {
        let root = tempfile::tempdir().unwrap();
        let index = RepositoryIndex::new(root.path().to_path_buf());
        let (mut writer, reader) = tokio::io::duplex(1024);
        writer.write_all(concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"method\":\"item/completed\",\"params\":{\"item\":{\"type\":\"commandExecution\"}}}\n",
            "{\"method\":\"item/completed\",\"params\":{\"item\":{\"type\":\"webSearch\"}}}\n",
            "{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"tokenUsage\":{\"total\":{\"inputTokens\":40}}}}\n",
        ).as_bytes()).await.unwrap();
        let mut lines = BufReader::new(reader).lines();
        let mut stdin = tokio::io::sink();
        let (activity, _events) = mpsc::channel(1);
        let mut tracker = UsageTracker::new(None);
        let mut index_usage = IndexUsage {
            available: true,
            ..Default::default()
        };
        let result = tokio::time::timeout(
            Duration::from_millis(20),
            drive_turn(
                &mut stdin,
                &mut lines,
                1,
                &activity,
                &index,
                &mut tracker,
                &mut index_usage,
            ),
        )
        .await;
        assert!(result.is_err());
        let metrics = tracker.snapshot(true);
        assert_eq!(metrics.tool_calls, 2);
        assert_eq!(metrics.usage.unwrap().input_tokens, Some(40));
        assert_eq!(metrics.usage_status, UsageStatus::Partial);
        drop(writer);
    }

    #[tokio::test]
    async fn symbols_turn_reports_index_usage_without_source_logging() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.rs"), "fn alpha() {}\n").unwrap();
        let index = RepositoryIndex::new(root.path().to_path_buf());
        let input = concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"id\":9,\"method\":\"item/tool/call\",\"params\":{\"tool\":\"Symbols\",\"arguments\":{\"symbol\":\"alpha\"}}}\n",
            "{\"id\":10,\"method\":\"item/tool/call\",\"params\":{\"tool\":\"Symbols\",\"arguments\":{\"mode\":\"invalid\"}}}\n",
            "{\"method\":\"turn/completed\",\"params\":{\"turn\":{\"status\":\"completed\",\"items\":[{\"type\":\"agentMessage\",\"text\":\"{}\"}]}}}\n",
        );
        let mut lines = BufReader::new(input.as_bytes()).lines();
        let mut stdin = tokio::io::sink();
        let (activity, _events) = mpsc::channel(1);
        let mut tracker = UsageTracker::new(None);
        let mut index_usage = IndexUsage {
            available: true,
            ..Default::default()
        };
        let output = drive_turn(
            &mut stdin,
            &mut lines,
            1,
            &activity,
            &index,
            &mut tracker,
            &mut index_usage,
        )
        .await
        .unwrap();
        let usage = output.metrics.index_usage.unwrap();
        assert_eq!(usage.calls, 2);
        assert_eq!(usage.failed_calls, 1);
        assert_eq!(usage.parsed_files, 1);
        assert!(usage.output_bytes > 0);
    }

    #[test]
    fn reuse_is_disabled_by_any_zeroed_bound() {
        assert!(settings().reuses_process());
        assert!(!SessionSettings {
            warm: false,
            ..settings()
        }
        .reuses_process());
        assert!(!SessionSettings {
            idle_secs: 0,
            ..settings()
        }
        .reuses_process());
        assert!(!SessionSettings {
            max_warm: 0,
            ..settings()
        }
        .reuses_process());
    }

    #[test]
    fn thread_reuse_stops_at_the_turn_and_input_bounds() {
        let bounded = SessionSettings {
            max_thread_turns: 3,
            max_thread_input_tokens: 1_000,
            ..settings()
        };
        assert!(bounded.thread_has_turns_left(0));
        assert!(bounded.thread_has_turns_left(2));
        assert!(!bounded.thread_has_turns_left(3));

        assert!(bounded.thread_within_input_budget(None));
        assert!(bounded.thread_within_input_budget(Some(999)));
        assert!(!bounded.thread_within_input_budget(Some(1_000)));

        // `max_thread_turns = 1` means every request gets a fresh thread.
        let single = SessionSettings {
            max_thread_turns: 1,
            ..settings()
        };
        assert!(single.thread_has_turns_left(0));
        assert!(!single.thread_has_turns_left(1));

        // Zero is treated as one rather than as "no turns allowed", so a
        // mis-set config cannot make every request fail.
        let zeroed = SessionSettings {
            max_thread_turns: 0,
            ..settings()
        };
        assert!(zeroed.thread_has_turns_left(0));
        assert!(!zeroed.thread_has_turns_left(1));

        let unlimited_input = SessionSettings {
            max_thread_input_tokens: 0,
            ..settings()
        };
        assert!(unlimited_input.thread_within_input_budget(Some(u32::MAX)));
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
}
