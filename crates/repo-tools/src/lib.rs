//! Repository-scoped read-only tools for repotracer.
//! Model-facing contracts with concurrent execution and bounded output.

mod exec;
mod glob_tool;
mod grep;
mod index;
mod pathutil;
mod read;
mod types;

pub use exec::{execute_tools, ToolExecutor, DEFAULT_CONCURRENCY, DEFAULT_TOOL_TIMEOUT};
pub use glob_tool::GlobTool;
pub use grep::GrepTool;
pub use index::{RepositoryIndex, SymbolOccurrence};
pub use pathutil::{is_within_root, resolve_in_root, resolve_path, PathError};
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
    index: RepositoryIndex,
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
            index: RepositoryIndex::new(root.clone()),
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
        vec![
            self.read.schema(),
            self.glob.schema(),
            self.grep.schema(),
            self.index.schema(),
        ]
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
            "Symbols" => self.index.call(arguments).await,
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

#[cfg(test)]
mod investigation_scope_tests {
    use super::*;

    #[tokio::test]
    async fn nested_locations_are_not_rewritten_to_root_siblings() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(root.path().join("README.md"), "wrong-root-file").unwrap();
        std::fs::write(root.path().join("docs/README.md"), "correct-nested-file").unwrap();
        let tools = RepoTools::new(root.path());
        let output = tools.call_one("Read", r#"{"path":"docs/README.md"}"#).await;
        assert!(!output.failed);
        assert!(output.output.contains("correct-nested-file"));
        assert!(!output.output.contains("wrong-root-file"));
    }

    #[tokio::test]
    async fn related_checkout_reads_and_symbols_keep_absolute_provenance() {
        let root = tempfile::tempdir().unwrap();
        let related = tempfile::tempdir().unwrap();
        let source = related.path().join("dependency.rs");
        std::fs::write(&source, "fn related_symbol() {}\n").unwrap();
        let tools = RepoTools::new(root.path());
        for name in ["Read", "Symbols"] {
            let output = tools
                .call_one(name, &serde_json::json!({"path":source}).to_string())
                .await;
            assert!(!output.failed, "{}", output.output);
            assert!(
                output.output.contains("related_symbol"),
                "{}",
                output.output
            );
            assert!(output.output.contains("dependency.rs"), "{}", output.output);
            assert!(
                output.output.contains(
                    &related
                        .path()
                        .canonicalize()
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                ),
                "{}",
                output.output
            );
        }
    }
}
