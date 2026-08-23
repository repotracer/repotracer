//! Read / Glob / Grep tools for repotracer.
//! FastContext-compatible contracts with concurrent execution and fixed count mode.

mod exec;
mod glob_tool;
mod grep;
mod pathutil;
mod read;
mod types;

pub use exec::{execute_tools, ToolExecutor, DEFAULT_CONCURRENCY, DEFAULT_TOOL_TIMEOUT};
pub use glob_tool::GlobTool;
pub use grep::GrepTool;
pub use pathutil::{is_within_root, resolve_in_root, PathError};
pub use read::ReadTool;
pub use types::{
    ToolCall, ToolDefinition, ToolError, ToolName, ToolResult, ToolSchema, TOOL_DESCRIPTIONS,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Repository-scoped tool host.
#[derive(Clone)]
pub struct RepoTools {
    root: PathBuf,
    read: Arc<ReadTool>,
    glob: Arc<GlobTool>,
    grep: Arc<GrepTool>,
    concurrency: usize,
    timeout: Duration,
}

impl RepoTools {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            read: Arc::new(ReadTool::new(root.clone())),
            glob: Arc::new(GlobTool::new(root.clone())),
            grep: Arc::new(GrepTool::new(root.clone())),
            root,
            concurrency: DEFAULT_CONCURRENCY,
            timeout: DEFAULT_TOOL_TIMEOUT,
        }
    }

    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn schemas(&self) -> Vec<ToolSchema> {
        vec![self.read.schema(), self.glob.schema(), self.grep.schema()]
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.schemas()
            .into_iter()
            .map(|s| ToolDefinition {
                name: s.name.clone(),
                description: s.description.clone(),
                parameters: s.parameters.clone(),
            })
            .collect()
    }

    pub async fn call_one(&self, name: &str, arguments: &str) -> ToolResult {
        let started = std::time::Instant::now();
        let output = match name {
            "Read" => self.read.call(arguments).await,
            "Glob" => self.glob.call(arguments).await,
            "Grep" => self.grep.call(arguments).await,
            other => Err(ToolError::UnknownTool(other.to_string())),
        };
        match output {
            Ok(text) => ToolResult {
                tool_call_id: String::new(),
                name: name.to_string(),
                output: text,
                failed: false,
                duration_ms: started.elapsed().as_millis() as u64,
            },
            Err(err) => ToolResult {
                tool_call_id: String::new(),
                name: name.to_string(),
                output: format!("<system-reminder>Error: {err}</system-reminder>"),
                failed: true,
                duration_ms: started.elapsed().as_millis() as u64,
            },
        }
    }

    /// Execute tool calls concurrently, preserving input order in results.
    pub async fn call_many(&self, calls: &[ToolCall]) -> Vec<ToolResult> {
        execute_tools(calls, self, self.concurrency, self.timeout).await
    }
}

impl ToolExecutor for RepoTools {
    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let mut result = self.call_one(&call.name, &call.arguments).await;
        result.tool_call_id = call.id.clone();
        result
    }
}
