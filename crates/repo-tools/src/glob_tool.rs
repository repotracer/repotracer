use crate::pathutil::resolve_in_root;
use crate::read::{finish_output, MAX_OUTPUT_BYTES};
use crate::types::{ToolError, ToolSchema};
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
                        "description": "Repository-relative directory to search. Use `.` or omit it for the repository root; never use an absolute path."
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
    let lines: Vec<String> = matched
        .into_iter()
        .take(limit)
        .map(|(p, _)| {
            p.strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    let evidence = lines.join("\n");
    let line_truncated = total > limit;
    let line_notice = line_truncated.then(|| {
        format!(
            "[Output truncated: showing first {limit} of {total} results. Use a more specific directory or pattern to continue.]"
        )
    });
    let separator = usize::from(!evidence.is_empty() && !evidence.ends_with('\n'));
    let byte_truncated =
        evidence.len() + separator + line_notice.as_ref().map_or(0, String::len) > MAX_OUTPUT_BYTES;
    let notice = match (line_truncated, byte_truncated) {
        (true, true) => Some(format!(
            "[Output truncated: {total} results exceeded the {limit}-result / 32 KiB cap. Use a more specific directory or pattern to continue.]"
        )),
        (true, false) => line_notice,
        (false, true) => Some(
            "[Output truncated at 32 KiB. Use a more specific directory or pattern to continue.]"
                .into(),
        ),
        (false, false) => None,
    };

    Ok(finish_output(&evidence, notice.as_deref()))
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

    #[tokio::test]
    async fn bounds_paths_and_reports_how_to_continue() {
        let dir = tempdir().unwrap();
        let deep = dir.path().join("a".repeat(180)).join("b".repeat(180));
        fs::create_dir_all(&deep).unwrap();
        for index in 0..101 {
            fs::write(deep.join(format!("file-{index:03}.txt")), "").unwrap();
        }
        let tool = GlobTool::new(dir.path());
        let out = tool.call(r#"{"pattern":"**/*.txt"}"#).await.unwrap();

        assert!(out.len() <= MAX_OUTPUT_BYTES, "{} bytes", out.len());
        assert!(out.contains("Output truncated"), "{out}");
        assert!(out.contains("specific directory or pattern"), "{out}");
    }
}
