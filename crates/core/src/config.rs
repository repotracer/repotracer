use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepotracerConfig {
    #[serde(default)]
    pub model: ModelSettings,
    #[serde(default)]
    pub explorer: ExplorerBudget,
}

impl RepotracerConfig {
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
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub temperature: f32,
}

pub const FASTCONTEXT_MODEL: &str = "hf.co/mitkox/FastContext-1.0-4B-RL-Q4_K_M-GGUF:latest";

fn default_backend() -> String {
    "ollama".into()
}
fn default_model() -> String {
    FASTCONTEXT_MODEL.into()
}
fn default_base_url() -> String {
    "http://127.0.0.1:11434/v1".into()
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
            base_url: default_base_url(),
            api_key: Some("ollama".into()),
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
