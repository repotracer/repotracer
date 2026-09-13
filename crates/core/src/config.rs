use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoTracerConfig {
    #[serde(default)]
    pub model: ModelSettings,
    #[serde(default)]
    pub explorer: ExplorerBudget,
    #[serde(default)]
    pub session: SessionSettings,
    #[serde(default, alias = "notifications")]
    pub updates: UpdateSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// Whether the binary may replace itself with a newer release.
    #[serde(default = "default_true", alias = "update_available")]
    pub automatic: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { automatic: true }
    }
}

fn default_true() -> bool {
    true
}

impl RepoTracerConfig {
    pub fn load_from(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        // Write a private sibling first so readers and failed saves retain the
        // previous profile until the complete replacement is ready.
        use std::io::Write;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        // NamedTempFile uses mode 0600 on Unix and cleans up unsuccessful writes.
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(text.as_bytes())?;
        file.as_file().sync_all()?;
        file.into_temp_path().persist(path)?;
        // Persist the directory entry as well as the new file's contents.
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSettings {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default = "default_model")]
    pub model: String,
    /// Optional API reasoning effort. Native providers use medium when empty.
    #[serde(default)]
    pub reasoning_effort: String,
    /// Permit one scout-requested continuation at a higher native-supported effort.
    #[serde(default = "default_true")]
    pub adaptive_reasoning: bool,
    /// Codex service tier. `fast` is accepted as an alias for `priority` by the
    /// subscription backend. Empty means unset, which that backend also reads as
    /// `priority`; only the Codex backend reads this field at all, so a Claude
    /// profile leaves it empty and never writes it out.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service_tier: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model request timeout in milliseconds. Subscription CLIs use it as a stream inactivity timeout. Zero disables it.
    #[serde(default)]
    pub timeout_ms: u64,
    #[serde(default)]
    pub temperature: f32,
}

impl ModelSettings {
    pub fn native_reasoning_effort(&self) -> &str {
        match self.reasoning_effort.trim() {
            "" => "medium",
            effort => effort,
        }
    }

    /// Whether this profile selects the native Claude Code backend.
    pub fn is_claude(&self) -> bool {
        matches!(
            self.backend.to_ascii_lowercase().as_str(),
            "claude" | "claude-cli"
        )
    }

    pub fn resolved_api_key(&self) -> Option<String> {
        std::env::var("REPOTRACER_API_KEY")
            .ok()
            .or_else(|| self.api_key.clone())
    }
}

fn default_backend() -> String {
    "codex-cli".into()
}
fn default_model() -> String {
    "gpt-5.6-luna".into()
}
fn default_base_url() -> String {
    "https://api.openai.com/v1".into()
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            executable: None,
            model: default_model(),
            reasoning_effort: String::new(),
            adaptive_reasoning: true,
            service_tier: String::new(),
            base_url: default_base_url(),
            api_key: None,
            timeout_ms: 0,
            temperature: 0.0,
        }
    }
}

/// Reuse policy for the scout provider process and its conversation thread.
///
/// Two independent levels of reuse, because they have different risk profiles:
///
/// * **Process reuse** (`warm`) removes per-call process spawn, the isolated
///   `CODEX_HOME` setup, and the JSON-RPC handshake. Defaults on; measured
///   latency benefit depends on the workload.
/// * **Thread reuse** (`max_thread_turns > 1`) continues the same conversation
///   only when the parent supplies the same conversation ID and repository.
///   It changes what the model sees: accumulated
///   history costs input tokens and can pollute an unrelated question — so it
///   is bounded on both turn count and input size. Whether it actually pays is
///   measurable from `ScoutStats::cached_prompt_tokens`; do not assume it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSettings {
    /// Retire a process before it creates more threads, bounding retained server state.
    #[serde(default = "default_max_process_threads")]
    pub max_process_threads: u32,
    /// Keep the scout provider process alive between calls.
    #[serde(default = "default_true")]
    pub warm: bool,
    /// Retire a warm process after this many seconds without a request. Zero disables reuse.
    #[serde(default = "default_session_idle_secs")]
    pub idle_secs: u64,
    /// Idle warm processes held at once, across repositories and conversations.
    #[serde(default = "default_max_warm_sessions")]
    pub max_warm: usize,
    /// Turns to run on one conversation thread before starting a fresh one. One disables thread reuse.
    #[serde(default = "default_max_thread_turns")]
    pub max_thread_turns: u32,
    /// Start a fresh thread when the previous turn's input exceeded this many tokens. Zero disables the check.
    #[serde(default = "default_max_thread_input_tokens")]
    pub max_thread_input_tokens: u32,
}

fn default_session_idle_secs() -> u64 {
    300
}
fn default_max_process_threads() -> u32 {
    32
}
fn default_max_warm_sessions() -> usize {
    2
}
fn default_max_thread_turns() -> u32 {
    4
}
fn default_max_thread_input_tokens() -> u32 {
    120_000
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            warm: true,
            max_process_threads: default_max_process_threads(),
            idle_secs: default_session_idle_secs(),
            max_warm: default_max_warm_sessions(),
            max_thread_turns: default_max_thread_turns(),
            max_thread_input_tokens: default_max_thread_input_tokens(),
        }
    }
}

impl SessionSettings {
    /// Whether a finished session may be kept for the next request.
    pub fn reuses_process(&self) -> bool {
        self.warm && self.idle_secs > 0 && self.max_warm > 0
    }

    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_secs)
    }

    /// Whether a thread that has already run `turns` turns may run another.
    pub fn thread_has_turns_left(&self, turns: u32) -> bool {
        turns < self.max_thread_turns.max(1)
    }

    /// Whether a thread whose last turn consumed `input_tokens` may be continued.
    pub fn thread_within_input_budget(&self, input_tokens: Option<u32>) -> bool {
        match (self.max_thread_input_tokens, input_tokens) {
            (0, _) | (_, None) => true,
            (limit, Some(used)) => used < limit,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorerBudget {
    /// Investigation turn ceiling. Zero leaves depth to the scout.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// Whole exploration timeout in seconds. Zero disables it.
    #[serde(default)]
    pub timeout_seconds: u64,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,
    /// Per-tool timeout in seconds. Zero disables it.
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_seconds: u64,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
}

fn default_max_turns() -> u32 {
    0
}
fn default_max_tool_calls() -> u32 {
    40
}
fn default_tool_timeout_secs() -> u64 {
    10
}
fn default_concurrency() -> usize {
    8
}

impl Default for ExplorerBudget {
    fn default() -> Self {
        Self {
            max_turns: default_max_turns(),
            timeout_seconds: 0,
            max_tool_calls: default_max_tool_calls(),
            tool_timeout_seconds: default_tool_timeout_secs(),
            concurrency: default_concurrency(),
        }
    }
}

impl ExplorerBudget {
    pub fn total_timeout(&self) -> Option<Duration> {
        (self.timeout_seconds > 0).then(|| Duration::from_secs(self.timeout_seconds))
    }

    pub fn tool_timeout(&self) -> Duration {
        Duration::from_secs(self.tool_timeout_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_defaults_belong_to_native_providers() {
        for model in [
            ModelSettings::default(),
            toml::from_str::<ModelSettings>("").unwrap(),
            toml::from_str::<ModelSettings>("backend = 'openai-compatible'").unwrap(),
        ] {
            assert!(model.reasoning_effort.is_empty());
            assert_eq!(model.native_reasoning_effort(), "medium");
            let saved: ModelSettings = toml::from_str(&toml::to_string(&model).unwrap()).unwrap();
            assert!(saved.reasoning_effort.is_empty());
        }
        let explicit: ModelSettings = toml::from_str("reasoning_effort = 'high'").unwrap();
        assert_eq!(explicit.reasoning_effort, "high");
        assert_eq!(explicit.native_reasoning_effort(), "high");
    }

    #[test]
    fn saving_a_profile_does_not_mutate_existing_readers() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.toml");
        let mut config = RepoTracerConfig::default();
        config.save_to(&path).unwrap();
        let previous = std::fs::read_to_string(&path).unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();
        config.model.model = "updated-model".into();
        let saved = config.save_to(&path);
        if let Err(error) = saved {
            // Windows can refuse replacement while another handle is open.
            // That must leave the complete previous profile intact.
            #[cfg(not(windows))]
            panic!("profile replacement failed: {error}");
            #[cfg(windows)]
            {
                drop(error);
                assert_eq!(std::fs::read_to_string(&path).unwrap(), previous);
            }
        } else {
            assert_eq!(
                RepoTracerConfig::load_from(&path).unwrap().model.model,
                "updated-model"
            );
        }
        let mut original = String::new();
        reader.read_to_string(&mut original).unwrap();
        assert_eq!(original, previous);
        drop(reader);
        config.save_to(&path).unwrap();
        assert_eq!(
            RepoTracerConfig::load_from(&path).unwrap().model.model,
            "updated-model"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn adaptive_reasoning_preserves_initial_effort_and_can_be_disabled() {
        let old: RepoTracerConfig = toml::from_str("[model]\nreasoning_effort = 'high'\n").unwrap();
        assert!(old.model.adaptive_reasoning);
        assert_eq!(old.model.reasoning_effort, "high");
        let off: RepoTracerConfig =
            toml::from_str("[model]\nadaptive_reasoning = false\n").unwrap();
        assert!(!off.model.adaptive_reasoning);
        assert_eq!(off.model.native_reasoning_effort(), "medium");
        let round_trip: RepoTracerConfig = toml::from_str(&toml::to_string(&off).unwrap()).unwrap();
        assert!(!round_trip.model.adaptive_reasoning);
    }

    #[test]
    fn updates_are_on_unless_the_user_turns_them_off() {
        assert!(RepoTracerConfig::default().updates.automatic);
        // A config written before this setting existed must not silently
        // disable updates; a missing table has to read as the default.
        let old: RepoTracerConfig = toml::from_str("[model]\nmodel = \"gpt-5.6-luna\"\n").unwrap();
        assert!(old.updates.automatic);

        let off: RepoTracerConfig = toml::from_str("[updates]\nautomatic = false\n").unwrap();
        assert!(!off.updates.automatic);

        let legacy: RepoTracerConfig =
            toml::from_str("[notifications]\nupdate_available = false\n").unwrap();
        assert!(!legacy.updates.automatic);
    }

    #[test]
    fn whole_run_timeouts_are_opt_in() {
        let config = RepoTracerConfig::default();
        assert_eq!(config.model.timeout_ms, 0);
        assert_eq!(config.model.service_tier, "fast");
        assert_eq!(config.explorer.total_timeout(), None);
        assert_eq!(config.explorer.max_turns, 0);
        let explicit: RepoTracerConfig = toml::from_str("[explorer]\nmax_turns = 6\n").unwrap();
        assert_eq!(explicit.explorer.max_turns, 6);
    }

    #[cfg(unix)]
    #[test]
    fn saved_profiles_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.toml");
        let mut config = RepoTracerConfig::default();
        config.model.backend = "openai-compatible".into();
        config.model.api_key = Some("secret".into());
        config.save_to(&path).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
