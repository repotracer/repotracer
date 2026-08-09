use crate::pathutil::resolve_in_root;
use crate::types::{ToolError, ToolSchema};
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const DEFAULT_LIMIT: usize = 100;
const DESCRIPTION: &str = include_str!("../prompts/glob.md");

const DEFAULT_IGNORES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "coverage",
    "__pycache__",
    ".venv",
    "venv",
];

#[derive(Debug, Deserialize)]
struct GlobArgs {
    pattern: String,
    #[serde(default)]
    directory: Option<String>,
}

pub struct GlobTool {
    root: PathBuf,
}

impl GlobTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "Glob".into(),
            description: DESCRIPTION.trim().into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "directory": {
                        "type": "string",
                        "description": "The absolute path of the directory to search in. If not provided, the current working directory will be used."
                    },
                    "pattern": {
                        "type": "string",
                        "description": "The glob pattern to match files or directories."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    pub async fn call(&self, arguments: &str) -> Result<String, ToolError> {
        let args: GlobArgs = serde_json::from_str(if arguments.trim().is_empty() {
            "{}"
        } else {
            arguments
        })
        .map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let dir_input = args.directory.as_deref().unwrap_or(".");
        let directory = match resolve_in_root(&self.root, dir_input) {
            Ok(p) => p,
            Err(e) => {
                return Ok(format!(
                    "<system-reminder>Permission error: `{dir_input}` — {e}</system-reminder>"
                ));
            }
        };

        if !directory.is_dir() {
            return Ok(format!(
                "<system-reminder>Error: directory `{dir_input}` does not exist or is not a directory.</system-reminder>"
            ));
        }

        let pattern = args.pattern.clone();
        let root = self.root.clone();
        let out = tokio::task::spawn_blocking(move || run_glob(&root, &directory, &pattern))
            .await
            .map_err(|e| ToolError::Message(e.to_string()))?;

        out
    }
}

fn run_glob(root: &Path, directory: &Path, pattern: &str) -> Result<String, ToolError> {
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map_err(|e| ToolError::InvalidArgs(format!("invalid glob: {e}")))?
        .compile_matcher();

    let mut builder = WalkBuilder::new(directory);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false);

    for ig in DEFAULT_IGNORES {
        builder.add_custom_ignore_filename(ig); // no-op-ish; filter manually below
    }

    let mut matched: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in builder.build().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        // skip default garbage dirs by path component
        if path
            .components()
            .any(|c| DEFAULT_IGNORES.iter().any(|ig| c.as_os_str() == *ig))
        {
            continue;
        }

        let rel_to_dir = path.strip_prefix(directory).unwrap_or(path);
        let rel_str = rel_to_dir.to_string_lossy().replace('\\', "/");
        let full_rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        // Match against relative-to-search-dir and repo-relative path.
        if !(matcher.is_match(&rel_str)
            || matcher.is_match(&full_rel)
            || matcher.is_match(path.file_name().and_then(|s| s.to_str()).unwrap_or("")))
        {
            // Also try **/pattern style if user omitted **/
            if !pattern.contains('/') && !pattern.starts_with('*') {
                let alt = format!("**/{pattern}");
                if let Ok(g) = globset::Glob::new(&alt) {
                    if !g.compile_matcher().is_match(&rel_str)
                        && !g.compile_matcher().is_match(&full_rel)
                    {
                        continue;
                    }
                } else {
                    continue;
                }
            } else {
                continue;
            }
        }

        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        matched.push((path.to_path_buf(), mtime));
    }

    // Deterministic: newest first, then path.
    matched.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    if matched.is_empty() {
        return Ok("No files found".into());
    }

    let total = matched.len();
    let limit = DEFAULT_LIMIT;
    let mut lines: Vec<String> = matched
        .into_iter()
        .take(limit)
        .map(|(p, _)| {
            p.strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    if total > limit {
        lines.push(format!(
            "Results are truncated: showing first {limit} of {total} results. Consider using a more specific path or pattern."
        ));
    }

    let _ = Duration::from_secs(0); // keep import used if optimized later
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn finds_by_pattern() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/auth.rs"), "").unwrap();
        fs::write(dir.path().join("src/main.rs"), "").unwrap();
        let tool = GlobTool::new(dir.path());
        let out = tool.call(r#"{"pattern":"**/*auth*"}"#).await.unwrap();
        assert!(out.contains("auth.rs"));
    }
}
