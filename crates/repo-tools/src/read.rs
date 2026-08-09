use crate::pathutil::{looks_binary, resolve_in_root};
use crate::types::{ToolError, ToolSchema};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::fs;

const MAX_LINES: usize = 2000;
const MAX_LINE_LENGTH: usize = 2000;
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB

const DESCRIPTION: &str = include_str!("../prompts/read.md");

#[derive(Debug, Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

pub struct ReadTool {
    root: PathBuf,
}

impl ReadTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "Read".into(),
            description: DESCRIPTION.trim().into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The absolute path of the file to read."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "The line number to start reading from. Positive values are 1-indexed from the start of the file. Only provide if the file is too large to read at once."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "The number of lines to read. Only provide if the file is too large to read at once."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    pub async fn call(&self, arguments: &str) -> Result<String, ToolError> {
        let args: ReadArgs = serde_json::from_str(if arguments.trim().is_empty() {
            "{}"
        } else {
            arguments
        })
        .map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        if args.path.is_empty() {
            return Ok("<system-reminder>Error: file path is required</system-reminder>".into());
        }

        let path = match resolve_in_root(&self.root, &args.path) {
            Ok(p) => p,
            Err(e) => {
                return Ok(format!(
                    "<system-reminder>Permission error: `{path}` — {e}</system-reminder>",
                    path = args.path
                ));
            }
        };

        if !path.exists() {
            return Ok(format!(
                "<system-reminder>Error: {} does not exist</system-reminder>",
                args.path
            ));
        }
        if !path.is_file() {
            return Ok(format!(
                "<system-reminder>Error: {} is not a file</system-reminder>",
                args.path
            ));
        }

        let meta = fs::metadata(&path).await?;
        if meta.len() > MAX_FILE_BYTES {
            return Ok(format!(
                "<system-reminder>Error: file too large ({} bytes; max {})</system-reminder>",
                meta.len(),
                MAX_FILE_BYTES
            ));
        }

        let bytes = fs::read(&path).await?;
        if looks_binary(&bytes) {
            return Ok(format!(
                "<system-reminder>Error: binary file rejected: {}</system-reminder>",
                display_path(&self.root, &path)
            ));
        }

        let content = String::from_utf8_lossy(&bytes);
        let raw_lines: Vec<&str> = content.split_inclusive('\n').collect();
        if raw_lines.is_empty() || (raw_lines.len() == 1 && raw_lines[0].is_empty()) {
            return Ok("File is empty.".into());
        }

        let offset = args.offset.unwrap_or(1);
        if offset <= 0 {
            return Ok(
                "<system-reminder>Error: offset must be a positive integer</system-reminder>"
                    .into(),
            );
        }
        let offset = offset as usize;

        if let Some(limit) = args.limit {
            if limit <= 0 {
                return Ok(
                    "<system-reminder>Error: limit must be a positive integer</system-reminder>"
                        .into(),
                );
            }
        }

        let mut end_line = match args.limit {
            Some(limit) => offset + limit as usize - 1,
            None => raw_lines.len(),
        };
        if end_line > raw_lines.len() {
            end_line = raw_lines.len();
        }
        if offset > raw_lines.len() {
            return Ok(format!(
                "<system-reminder>Error: offset {offset} beyond end of file ({} lines)</system-reminder>",
                raw_lines.len()
            ));
        }

        let total_read = end_line.saturating_sub(offset - 1);
        let mut truncated = false;
        if total_read > MAX_LINES {
            end_line = offset + MAX_LINES - 1;
            truncated = true;
        }

        let mut lines = Vec::new();
        for (i, raw) in raw_lines.iter().enumerate().take(end_line).skip(offset - 1) {
            let mut line = (*raw).to_string();
            let core = line.trim_end_matches(['\n', '\r']);
            if core.len() > MAX_LINE_LENGTH {
                line = format!("{}...\n", &core[..MAX_LINE_LENGTH]);
            }
            if !line.ends_with('\n') {
                line.push('\n');
            }
            lines.push(format!("{}|{}", i + 1, line));
        }
        if truncated {
            lines.push("...\n".into());
        }

        let shown = display_path(&self.root, &path);
        let body = lines.concat();
        Ok(format!("```{shown}:{offset}-{end_line}\n{body}```"))
    }
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn reads_with_line_numbers() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn main() {}\nlet x = 1;\n").unwrap();
        let tool = ReadTool::new(dir.path());
        let out = tool
            .call(r#"{"path":"a.rs","offset":1,"limit":10}"#)
            .await
            .unwrap();
        assert!(out.contains("1|fn main()"));
        assert!(out.contains("2|let x = 1;"));
    }

    #[tokio::test]
    async fn rejects_escape() {
        let dir = tempdir().unwrap();
        let tool = ReadTool::new(dir.path());
        let out = tool.call(r#"{"path":"../../etc/passwd"}"#).await.unwrap();
        assert!(out.contains("Permission error") || out.contains("does not exist"));
    }
}
