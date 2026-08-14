use crate::pathutil::{looks_binary, resolve_in_root};
use crate::types::{ToolError, ToolSchema};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::fs;

const MAX_LINES: usize = 2000;
const MAX_LINE_LENGTH: usize = 2000;
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB
pub(crate) const MAX_OUTPUT_BYTES: usize = 32 * 1024;
const READ_NOTICE_RESERVE: usize = 128;

pub(crate) fn evidence_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some(newline) = text[..end].rfind('\n') {
        if end - (newline + 1) <= 4096 {
            end = newline + 1;
        }
    }
    &text[..end]
}

pub(crate) fn finish_output(evidence: &str, notice: Option<&str>) -> String {
    if notice.is_none() && evidence.len() <= MAX_OUTPUT_BYTES {
        return evidence.to_owned();
    }

    let notice = notice.unwrap_or("[Output truncated at 32 KiB. Narrow the request to continue.]");
    let separator = usize::from(!evidence.is_empty() && !evidence.ends_with('\n'));
    let budget = MAX_OUTPUT_BYTES.saturating_sub(notice.len() + separator);
    let prefix = evidence_prefix(evidence, budget);
    let separator = if prefix.is_empty() || prefix.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let output = format!("{prefix}{separator}{notice}");
    debug_assert!(output.len() <= MAX_OUTPUT_BYTES);
    output
}

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
                        "description": "Repository-relative path of the file to read. Never use an absolute path."
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
            Some(limit) => offset.saturating_add(limit as usize).saturating_sub(1),
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
        let line_truncated = total_read > MAX_LINES;
        if line_truncated {
            end_line = offset + MAX_LINES - 1;
        }

        let mut lines = Vec::new();
        for (i, raw) in raw_lines.iter().enumerate().take(end_line).skip(offset - 1) {
            let mut line = (*raw).to_string();
            let core = line.trim_end_matches(['\n', '\r']);
            if core.len() > MAX_LINE_LENGTH {
                line = format!("{}...\n", evidence_prefix(core, MAX_LINE_LENGTH));
            }
            if !line.ends_with('\n') {
                line.push('\n');
            }
            lines.push(format!("{}|{}", i + 1, line));
        }

        let shown = display_path(&self.root, &path);
        let body = lines.concat();
        let line_notice = line_truncated.then(|| {
            format!(
                "[Output truncated after {MAX_LINES} lines. Continue with offset {}.]\n",
                end_line + 1
            )
        });
        let header = format!("```{shown}:{offset}-{end_line}\n");
        let notice_separator = usize::from(line_notice.is_some());
        let complete_len = header.len()
            + body.len()
            + 3
            + notice_separator
            + line_notice.as_ref().map_or(0, String::len);
        if complete_len <= MAX_OUTPUT_BYTES {
            return Ok(format!(
                "{header}{body}```{separator}{notice}",
                separator = if line_notice.is_some() { "\n" } else { "" },
                notice = line_notice.as_deref().unwrap_or("")
            ));
        }

        let body_budget = MAX_OUTPUT_BYTES.saturating_sub(header.len() + READ_NOTICE_RESERVE + 3);
        let prefix = evidence_prefix(&body, body_budget);
        let kept_lines = prefix.bytes().filter(|byte| *byte == b'\n').count();
        let shown_end = offset + kept_lines.saturating_sub(1);
        let notice = format!(
            "[Output truncated at 32 KiB. Continue with offset {}.]",
            shown_end + 1
        );
        let output = format!("```{shown}:{offset}-{shown_end}\n{prefix}```\n{notice}");
        debug_assert!(output.len() <= MAX_OUTPUT_BYTES);
        Ok(output)
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
    async fn bounds_output_and_gives_next_offset() {
        let dir = tempdir().unwrap();
        let content = (0..400)
            .map(|_| format!("{}\n", "x".repeat(100)))
            .collect::<String>();
        fs::write(dir.path().join("large.rs"), content).unwrap();
        let tool = ReadTool::new(dir.path());
        let out = tool.call(r#"{"path":"large.rs"}"#).await.unwrap();
        assert!(out.contains("```\n[Output truncated"), "{out}");

        assert!(out.len() <= MAX_OUTPUT_BYTES, "{} bytes", out.len());
        assert!(out.contains("Continue with offset"), "{out}");

        let small = tool.call(r#"{"path":"large.rs","limit":2}"#).await.unwrap();
        assert!(small.contains("2|"), "{small}");
        assert!(!small.contains("3|"), "{small}");
        assert!(!small.contains("Output truncated"), "{small}");
    }

    #[tokio::test]
    async fn rejects_escape() {
        let dir = tempdir().unwrap();
        let tool = ReadTool::new(dir.path());
        let out = tool.call(r#"{"path":"../../etc/passwd"}"#).await.unwrap();
        assert!(out.contains("Permission error") || out.contains("does not exist"));
    }
}
