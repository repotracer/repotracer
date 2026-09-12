use crate::types::ValidatedCitation;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub reason: Option<String>,
}

static FINAL_ANSWER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<final_answer>(.*?)</final_answer>").unwrap());

static ENTRY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?):(\d+)(?:-(\d+))?\s*(.*)$").unwrap());

/// Parse citations from model final text.
pub fn parse_citations(text: &str) -> (String, Vec<Citation>) {
    let Some(caps) = FINAL_ANSWER_RE.captures(text) else {
        let citations = text
            .lines()
            .filter_map(|line| parse_entry(line.trim()))
            .collect::<Vec<_>>();
        let summary = if !citations.is_empty() && text.contains("</final_answer>") {
            String::new()
        } else {
            text.trim().to_string()
        };
        return (summary, citations);
    };

    let body = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
    let summary = FINAL_ANSWER_RE.replace(text, "").trim().to_string();

    let mut citations = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = parse_entry(line) {
            citations.push(c);
        }
    }
    (summary, citations)
}

fn parse_entry(line: &str) -> Option<Citation> {
    let caps = ENTRY_RE.captures(line)?;
    let path = caps.get(1)?.as_str().trim().to_string();
    let start: u32 = caps.get(2)?.as_str().parse().ok()?;
    let end: u32 = caps
        .get(3)
        .map(|m| m.as_str().parse().ok())
        .unwrap_or(Some(start))?;
    let reason_raw = caps.get(4).map(|m| m.as_str().trim()).unwrap_or("");
    let reason = if reason_raw.is_empty() {
        None
    } else {
        Some(
            reason_raw
                .trim_start_matches('(')
                .trim_end_matches(')')
                .trim()
                .to_string(),
        )
    };
    Some(Citation {
        path,
        start_line: start,
        end_line: end,
        reason,
    })
}

/// Validate a citation against the repository.
pub fn validate_citation(root: &Path, c: &Citation) -> Option<ValidatedCitation> {
    if c.start_line == 0 || c.end_line == 0 || c.start_line > c.end_line {
        return None;
    }

    let candidate = if Path::new(&c.path).is_absolute() {
        PathBuf::from(&c.path)
    } else {
        root.join(&c.path)
    };

    let root_canon = root.canonicalize().ok()?;
    let path_canon = candidate.canonicalize().ok()?;
    if !path_canon.is_file() {
        return None;
    }

    let source = std::fs::File::open(&path_canon).ok()?;
    let snapshot_length = source.metadata().ok()?.len();
    let line_count = source_line_count(source.take(snapshot_length), c.end_line)?;
    if line_count == 0 {
        return None;
    }
    if c.start_line > line_count {
        return None;
    }
    let end = c.end_line.min(line_count);

    let rel = crate::investigation::repository_relative(&root_canon, &path_canon).ok()?;

    Some(ValidatedCitation {
        path: rel,
        start_line: c.start_line,
        end_line: end,
        reason: c.reason.clone(),
    })
}

/// Validate locations using bounded buffers, stopping at the requested line.
/// Binary bytes in the examined prefix are rejected. Buffer size is bounded;
/// a valid location is not rejected merely because it is deep in a file.
fn source_line_count(source: impl Read, last_requested: u32) -> Option<u32> {
    let mut reader = BufReader::new(source);
    let mut lines = 0;
    let mut at_line_start = true;
    loop {
        let chunk = reader.fill_buf().ok()?;
        if chunk.is_empty() {
            return Some(lines);
        }
        if chunk.contains(&0) {
            return None;
        }
        for byte in chunk {
            if at_line_start {
                lines += 1;
                if lines == last_requested {
                    return Some(lines);
                }
            }
            at_line_start = *byte == b'\n';
        }
        let consumed = chunk.len();
        reader.consume(consumed);
    }
}

pub fn validate_citations(root: &Path, citations: &[Citation]) -> Vec<ValidatedCitation> {
    citations
        .iter()
        .filter_map(|c| validate_citation(root, c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn validation_does_not_read_beyond_the_requested_lines() {
        struct Source {
            read: bool,
        }
        impl Read for Source {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert!(!self.read, "read the uncited remainder");
                self.read = true;
                buffer[..4].copy_from_slice(b"a\nb\n");
                Ok(4)
            }
        }
        assert_eq!(source_line_count(Source { read: false }, 2), Some(2));
    }

    #[test]
    fn validation_bounds_long_lines_and_preserves_eof_and_binary_checks() {
        assert_eq!(source_line_count(std::io::repeat(b'x'), 1), Some(1));
        assert_eq!(
            source_line_count(std::io::repeat(b'x').take(16 * 1024 * 1024), 2),
            Some(1)
        );
        assert_eq!(source_line_count(&b"a\nb\n"[..], 10), Some(2));
        assert_eq!(source_line_count(&b""[..], 1), Some(0));
        assert_eq!(source_line_count(&b"a\0b"[..], 1), None);
    }

    #[test]
    fn parses_final_answer() {
        let text = r#"Found auth.

<final_answer>
src/auth/session.ts:81-144 (session validation)
src/auth/refresh.ts:10-20
</final_answer>"#;
        let (summary, cites) = parse_citations(text);
        assert!(summary.contains("Found auth"));
        assert_eq!(cites.len(), 2);
        assert_eq!(cites[0].start_line, 81);
        assert_eq!(cites[0].end_line, 144);
    }

    #[test]
    fn parses_untagged_citations_for_compatibility() {
        let text = "/repo/src/auth.rs:3-9 (auth flow)";
        let (_, cites) = parse_citations(text);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].path, "/repo/src/auth.rs");
    }

    #[test]
    fn cleans_dangling_final_answer_close_tag() {
        let text = "/repo/src/auth.rs:3-9 (auth flow)\n</final_answer>";
        let (summary, cites) = parse_citations(text);
        assert!(summary.is_empty());
        assert_eq!(cites.len(), 1);
    }

    #[test]
    fn validates_existing_file() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "a\nb\nc\nd\n").unwrap();
        let c = Citation {
            path: "a.rs".into(),
            start_line: 2,
            end_line: 3,
            reason: Some("x".into()),
        };
        let v = validate_citation(dir.path(), &c).unwrap();
        assert_eq!(v.path, "a.rs");
        assert_eq!(v.start_line, 2);
    }

    #[test]
    fn related_citations_preserve_native_paths() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        // On Unix a backslash is part of a filename, not a separator. On
        // Windows canonicalize supplies a native verbatim path prefix.
        let name = if cfg!(unix) {
            "related\\source.rs"
        } else {
            "source.rs"
        };
        let outside = dir.path().join(name);
        fs::write(&outside, "external evidence\n").unwrap();
        let c = Citation {
            path: outside.to_string_lossy().into_owned(),
            start_line: 1,
            end_line: 1,
            reason: None,
        };
        let v = validate_citation(&root, &c).unwrap();
        assert_eq!(PathBuf::from(&v.path), outside.canonicalize().unwrap());
        assert_eq!(fs::read_to_string(&v.path).unwrap(), "external evidence\n");
        let repeated = Citation { path: v.path, ..c };
        assert!(validate_citation(&root, &repeated).is_some());
    }

    #[test]
    fn rejects_missing_related_source() {
        let dir = tempdir().unwrap();
        let c = Citation {
            path: "../secret".into(),
            start_line: 1,
            end_line: 1,
            reason: None,
        };
        assert!(validate_citation(dir.path(), &c).is_none());
    }
}
