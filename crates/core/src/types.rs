use repotracer_repo_tools::ToolResult;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[async_trait::async_trait]
pub trait ScoutBackend: Send + Sync {
    async fn scout(&self, request: ScoutRequest) -> anyhow::Result<ScoutResult>;
}

#[derive(Debug, Clone)]
pub struct ScoutRequest {
    pub query: String,
    pub root: PathBuf,
    pub focus: Option<PathBuf>,
    pub max_turns: Option<u32>,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedCitation {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScoutStats {
    pub turns: u32,
    pub tool_calls: u32,
    pub duration_ms: u64,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_prompt_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoutResult {
    pub summary: String,
    pub citations: Vec<ValidatedCitation>,
    pub stats: ScoutStats,
    /// Raw final assistant text (for debugging).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_final: Option<String>,
}

impl ScoutResult {
    /// Compact text for MCP / frontier model consumption.
    pub fn compact_text(&self) -> String {
        let mut out = String::new();
        if !self.summary.is_empty() {
            out.push_str(&self.summary);
            out.push_str("\n\n");
        }
        if self.citations.is_empty() {
            out.push_str("No validated citations.");
        } else {
            out.push_str("Citations:\n");
            for c in &self.citations {
                if let Some(r) = &c.reason {
                    out.push_str(&format!(
                        "- {}:{}-{} — {}\n",
                        c.path, c.start_line, c.end_line, r
                    ));
                } else {
                    out.push_str(&format!("- {}:{}-{}\n", c.path, c.start_line, c.end_line));
                }
            }
        }
        out.push_str(&format!(
            "\n(scout: {} · {} model steps · {} tools · {} ms)",
            self.stats.model, self.stats.turns, self.stats.tool_calls, self.stats.duration_ms
        ));
        out
    }

    pub fn cli_text(&self) -> String {
        let mut out = String::new();
        let n = self.citations.len();
        out.push_str(&format!(
            "Found {n} relevant location{} in {:.1}s\n\n",
            if n == 1 { "" } else { "s" },
            self.stats.duration_ms as f64 / 1000.0
        ));
        for c in &self.citations {
            out.push_str(&format!("{}:{}-{}\n", c.path, c.start_line, c.end_line));
            if let Some(r) = &c.reason {
                let reason = r.trim().trim_start_matches('(').trim_end_matches(')');
                out.push_str(&format!("  {reason}\n"));
            }
            out.push('\n');
        }
        if !self.summary.is_empty() {
            out.push_str(&format!("Summary: {}\n\n", self.summary.trim()));
        }
        out.push_str(&format!(
            "Scout: {}\nModel steps: {}\nTool calls: {}\n",
            self.stats.model, self.stats.turns, self.stats.tool_calls
        ));
        out
    }
}

#[derive(Debug, Clone)]
pub struct ExplorerTurn {
    pub index: u32,
    pub assistant_text: Option<String>,
    pub tool_results: Vec<ToolResult>,
}
