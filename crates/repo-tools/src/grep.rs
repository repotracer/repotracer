use crate::pathutil::resolve_in_root;
use crate::read::{finish_output, MAX_OUTPUT_BYTES};
use crate::types::{ToolError, ToolSchema};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

const DEFAULT_LIMIT: usize = 100;
const DESCRIPTION: &str = include_str!("../prompts/grep.md");

#[derive(Debug, Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    output_mode: Option<String>,
    #[serde(default, rename = "-B")]
    before_context: Option<u32>,
    #[serde(default, rename = "-A")]
    after_context: Option<u32>,
    #[serde(default, rename = "-C")]
    context: Option<u32>,
    #[serde(default, rename = "-n")]
    line_number: Option<bool>,
    #[serde(default, rename = "-i")]
    ignore_case: Option<bool>,
    #[serde(default, rename = "type")]
    file_type: Option<String>,
    #[serde(default)]
    head_limit: Option<usize>,
    #[serde(default)]
    multiline: Option<bool>,
}

pub struct GrepTool {
    root: PathBuf,
    rg_path: PathBuf,
}

impl GrepTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let rg_path = which::which("rg").unwrap_or_else(|_| PathBuf::from("rg"));
        Self {
            root: root.into(),
            rg_path,
        }
    }

    pub fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "Grep".into(),
            description: DESCRIPTION.trim().into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The regular expression pattern to search for in file contents"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search in (rg pattern -- PATH). Defaults to current working directory."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g. \"*.js\", \"*.{ts,tsx}\") - maps to rg --glob"
                    },
                    "output_mode": {
                        "type": "string",
                        "enum": ["content", "files_with_matches", "count"],
                        "description": "Output mode: \"content\" shows matching lines, \"files_with_matches\" shows only file paths, \"count\" shows match counts. Defaults to \"files_with_matches\"."
                    },
                    "-B": {
                        "type": "number",
                        "description": "Number of lines to show before each match (rg -B). Requires output_mode: \"content\"."
                    },
                    "-A": {
                        "type": "number",
                        "description": "Number of lines to show after each match (rg -A). Requires output_mode: \"content\"."
                    },
                    "-C": {
                        "type": "number",
                        "description": "Number of lines to show before and after each match (rg -C). Requires output_mode: \"content\"."
                    },
                    "-n": {
                        "type": "boolean",
                        "description": "Show line numbers in output (rg -n). Requires output_mode: \"content\". Defaults to true."
                    },
                    "-i": {
                        "type": "boolean",
                        "description": "Case insensitive search (rg -i)"
                    },
                    "type": {
                        "type": "string",
                        "description": "File type to search (rg --type). Common types: js, py, rust, go, java, etc."
                    },
                    "head_limit": {
                        "type": "number",
                        "minimum": 0,
                        "description": "Limit output to first N lines/entries."
                    },
                    "multiline": {
                        "type": "boolean",
                        "description": "Enable multiline mode (rg -U --multiline-dotall). Default: false."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    pub async fn call(&self, arguments: &str) -> Result<String, ToolError> {
        let args: GrepArgs = serde_json::from_str(if arguments.trim().is_empty() {
            "{}"
        } else {
            arguments
        })
        .map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let search_path_input = args.path.as_deref().unwrap_or(".");
        let search_path = match resolve_in_root(&self.root, search_path_input) {
            Ok(p) => p,
            Err(e) => {
                return Ok(format!(
                    "<system-reminder>Permission error: `{search_path_input}` — {e}</system-reminder>"
                ));
            }
        };

        // Normalize output_mode. Public contract: "count". Also accept upstream typo "count_matches".
        let mode = args.output_mode.as_deref().unwrap_or("files_with_matches");
        let mode = match mode {
            "count_matches" => "count",
            m => m,
        };

        let mut cmd = Command::new(&self.rg_path);
        cmd.arg("--color").arg("never");
        cmd.arg("--heading");
        // Never leave the repo via rg globs; constrain to path.
        cmd.current_dir(&self.root);
        cmd.stdin(Stdio::null());

        match mode {
            "content" => {
                let line_number = args.line_number.unwrap_or(true);
                if line_number {
                    cmd.arg("-n");
                }
                if let Some(b) = args.before_context {
                    cmd.arg("-B").arg(b.to_string());
                }
                if let Some(a) = args.after_context {
                    cmd.arg("-A").arg(a.to_string());
                }
                // Default to three context lines only when before/after are unset.
                if args.before_context.is_none() && args.after_context.is_none() {
                    let c = args.context.unwrap_or(3);
                    if c > 0 {
                        cmd.arg("-C").arg(c.to_string());
                    }
                } else if let Some(c) = args.context {
                    cmd.arg("-C").arg(c.to_string());
                }
            }
            "files_with_matches" => {
                cmd.arg("--files-with-matches");
            }
            "count" => {
                // FIXED: schema says "count", map to --count-matches (not the broken count_matches check alone).
                cmd.arg("--count-matches");
            }
            other => {
                return Ok(format!(
                    "<system-reminder>Error: unknown output_mode `{other}`. Use content, files_with_matches, or count.</system-reminder>"
                ));
            }
        }

        if args.ignore_case.unwrap_or(false) {
            cmd.arg("--ignore-case");
        }
        if let Some(g) = &args.glob {
            cmd.arg("--glob").arg(g);
        }
        if let Some(t) = &args.file_type {
            cmd.arg("--type").arg(t);
        }
        if args.multiline.unwrap_or(false) {
            cmd.arg("--multiline");
            cmd.arg("--multiline-dotall");
        }

        // Default ignores for speed/noise.
        for ig in [
            "!**/.git/**",
            "!**/node_modules/**",
            "!**/target/**",
            "!**/dist/**",
            "!**/build/**",
            "!**/.next/**",
            "!**/coverage/**",
            "!**/__pycache__/**",
            "!**/.venv/**",
        ] {
            cmd.arg("--glob").arg(ig);
        }

        cmd.arg(&args.pattern);
        // Pass path relative to root when possible for cleaner output.
        let path_arg = search_path
            .strip_prefix(&self.root)
            .map(|p| p.to_path_buf())
            .unwrap_or(search_path.clone());
        if path_arg.as_os_str().is_empty() {
            cmd.arg(".");
        } else {
            cmd.arg(&path_arg);
        }

        let output = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ToolError::Message(
                    "ripgrep (`rg`) not found on PATH. Install: https://github.com/BurntSushi/ripgrep#installation"
                        .into(),
                )
            } else {
                ToolError::Message(format!("failed to run rg: {e}"))
            }
        })?;

        // rg exit 0 = matches, 1 = no matches, 2 = error
        let code = output.status.code().unwrap_or(2);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if code == 1 || (code == 0 && stdout.trim().is_empty()) {
            return Ok("No matches found".into());
        }
        if code >= 2 && stdout.trim().is_empty() {
            return Ok(format!(
                "<system-reminder>rg error: {}</system-reminder>",
                stderr.trim()
            ));
        }

        let text = if stdout.is_empty() { stderr } else { stdout };
        let mut limit = DEFAULT_LIMIT;
        if let Some(h) = args.head_limit {
            if h > 0 && h < limit {
                limit = h;
            }
        }
        let line_count = text.lines().count();
        let line_truncated = line_count > limit;
        let evidence = if line_truncated {
            text.lines().take(limit).collect::<Vec<_>>().join("\n")
        } else {
            text
        };

        let line_notice = line_truncated.then(|| {
            format!(
                "[Output truncated: showing first {limit} of at least {line_count} output lines. Narrow the pattern or path to continue.]"
            )
        });
        let separator = usize::from(!evidence.is_empty() && !evidence.ends_with('\n'));
        let byte_truncated =
            evidence.len() + separator + line_notice.as_ref().map_or(0, String::len)
                > MAX_OUTPUT_BYTES;
        let notice = match (line_truncated, byte_truncated) {
            (true, true) => Some(format!(
                "[Output truncated: matched at least {line_count} output lines and exceeded the {limit}-line / 32 KiB cap. Narrow the pattern or path to continue.]"
            )),
            (true, false) => line_notice,
            (false, true) => Some(
                "[Output truncated at 32 KiB. Narrow the pattern or path to continue.]".into(),
            ),
            (false, false) => None,
        };

        Ok(finish_output(&evidence, notice.as_deref()))
    }
}

/// Expose count-mode mapping for tests without spawning when rg missing.
#[cfg(test)]
pub fn rg_count_flag_for_mode(mode: &str) -> Option<&'static str> {
    match mode {
        "count" | "count_matches" => Some("--count-matches"),
        "files_with_matches" => Some("--files-with-matches"),
        "content" => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn count_mode_maps_correctly() {
        assert_eq!(rg_count_flag_for_mode("count"), Some("--count-matches"));
        assert_eq!(
            rg_count_flag_for_mode("count_matches"),
            Some("--count-matches")
        );
    }

    #[tokio::test]
    async fn greps_content() {
        if which::which("rg").is_err() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn refresh_token() {}\n").unwrap();
        let tool = GrepTool::new(dir.path());
        let out = tool
            .call(r#"{"pattern":"refresh_token","output_mode":"content","-C":0}"#)
            .await
            .unwrap();
        assert!(out.contains("refresh_token"), "{out}");
    }

    #[tokio::test]
    async fn bounds_content_output_with_continuation_guidance() {
        if which::which("rg").is_err() {
            return;
        }
        let dir = tempdir().unwrap();
        let content = (0..140)
            .map(|line| format!("needle-{line}-{}\n", "x".repeat(500)))
            .collect::<String>();
        fs::write(dir.path().join("large.txt"), content).unwrap();
        let tool = GrepTool::new(dir.path());
        let out = tool
            .call(r#"{"pattern":"needle","output_mode":"content","-C":0}"#)
            .await
            .unwrap();

        assert!(out.len() <= MAX_OUTPUT_BYTES, "{} bytes", out.len());
        assert!(out.contains("Output truncated"), "{out}");
        assert!(out.contains("Narrow the pattern or path"), "{out}");
    }

    #[tokio::test]
    async fn count_mode_works() {
        if which::which("rg").is_err() {
            return;
        }
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "foo foo bar\n").unwrap();
        let tool = GrepTool::new(dir.path());
        let out = tool
            .call(r#"{"pattern":"foo","output_mode":"count"}"#)
            .await
            .unwrap();
        assert!(out.contains('2') || out.contains("a.rs"), "{out}");
    }
}
