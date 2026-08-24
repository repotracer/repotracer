use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoTracerConfig {
    #[serde(default)]
    pub model: ModelSettings,
    #[serde(default)]
    pub explorer: ExplorerBudget,
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
        std::fs::write(path, text)?;
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
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub temperature: f32,
}

impl ModelSettings {
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
fn default_reasoning_effort() -> String {
    "medium".into()
}
fn default_base_url() -> String {
    "https://api.openai.com/v1".into()
}
fn default_timeout_ms() -> u64 {
    120_000
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            executable: None,
            model: default_model(),
            reasoning_effort: default_reasoning_effort(),
            base_url: default_base_url(),
            api_key: None,
            timeout_ms: default_timeout_ms(),
            temperature: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorerBudget {
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_timeout_secs")]
    pub timeout_seconds: u64,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_seconds: u64,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
}

fn default_max_turns() -> u32 {
    6
}
fn default_timeout_secs() -> u64 {
    60
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
            timeout_seconds: default_timeout_secs(),
            max_tool_calls: default_max_tool_calls(),
            tool_timeout_seconds: default_tool_timeout_secs(),
            concurrency: default_concurrency(),
        }
    }
}

impl ExplorerBudget {
    pub fn total_timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    pub fn tool_timeout(&self) -> Duration {
        Duration::from_secs(self.tool_timeout_seconds)
    }
}
