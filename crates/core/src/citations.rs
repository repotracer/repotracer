use crate::types::ValidatedCitation;
use regex::Regex;
use serde::{Deserialize, Serialize};
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
    if !path_canon.starts_with(&root_canon) {
        return None;
    }
    if !path_canon.is_file() {
        return None;
    }

    let content = std::fs::read(&path_canon).ok()?;
    if content.contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&content);
    let line_count = text.lines().count() as u32;
    if line_count == 0 {
        return None;
    }
    if c.start_line > line_count {
        return None;
    }
    let end = c.end_line.min(line_count);

    let rel = path_canon
        .strip_prefix(&root_canon)
        .unwrap_or(&path_canon)
        .to_string_lossy()
        .replace('\\', "/");

    Some(ValidatedCitation {
        path: rel,
        start_line: c.start_line,
        end_line: end,
        reason: c.reason.clone(),
    })
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
    fn rejects_escape() {
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
