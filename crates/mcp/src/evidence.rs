//! Streaming reads for source excerpts embedded in an MCP handoff.
use super::{EvidenceAttachmentError, EvidenceBundle, EvidenceSpan, ValidatedCitation};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek};
use std::path::Path;

/// Rewind the file and stream the requested operation. The operation owns any
/// allocation for selected lines; uncited file content is skipped by the
/// buffered reader and is never collected in memory.
fn with_reader<T>(
    file: &mut File,
    operation: impl FnOnce(&mut dyn BufRead) -> io::Result<T>,
) -> io::Result<T> {
    file.rewind()?;
    // A growing log must not make this scan chase an ever-moving EOF.
    // This bounds the read to the file's current length, not a reply quota.
    let length = file.metadata()?.len();
    operation(&mut BufReader::new(file.take(length)))
}

/// Check only as far as the last requested line; do not read an uncited tail.
fn count_requested_lines(file: &mut File, end: u32) -> io::Result<u32> {
    with_reader(file, |reader| {
        let mut count = 0;
        while count < end && !reader.fill_buf()?.is_empty() {
            count += 1;
            if count < end {
                reader.skip_until(b'\n')?;
            }
        }
        Ok(count)
    })
}

/// Skip uncited lines without allocating them and capture selected lines.
/// There is deliberately no transport-sized capture limit here: a cited line
/// is returned in full, while the reader still avoids allocating the rest of a
/// large file.
fn read_source_span(file: &mut File, start: u32, end: u32) -> io::Result<Option<(String, bool)>> {
    let (mut text, reached_end) = with_reader(file, |reader| {
        for _ in 1..start {
            if reader.skip_until(b'\n')? == 0 {
                return Ok((String::new(), false));
            }
        }
        let mut text = String::new();
        let mut bytes = Vec::new();
        let mut reached_end = false;
        for line in start..=end {
            bytes.clear();
            let read = reader.read_until(b'\n', &mut bytes)?;
            if read == 0 {
                break;
            }
            let raw = if bytes.ends_with(b"\n") {
                let raw = &bytes[..bytes.len() - 1];
                raw.strip_suffix(b"\r").unwrap_or(raw)
            } else {
                bytes.as_slice()
            };
            let source = std::str::from_utf8(raw)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            text.push_str(&format!("{line}: {source}\n"));
            reached_end = line == end;
        }
        Ok((text, reached_end))
    })?;
    if text.is_empty() || !reached_end {
        return Ok(None);
    }
    while text.ends_with('\n') {
        text.pop();
    }
    Ok(Some((text, false)))
}

fn source_path(root: &Path, citation_path: &str) -> io::Result<std::path::PathBuf> {
    let path = Path::new(citation_path);
    if path.is_absolute() {
        path.canonicalize()
    } else {
        repotracer_repo_tools::resolve_in_root(root, citation_path)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))
    }
}

fn attachment_error(
    path: &str,
    start_line: u32,
    end_line: u32,
    message: impl Into<String>,
) -> EvidenceAttachmentError {
    EvidenceAttachmentError {
        path: path.to_owned(),
        start_line,
        end_line,
        message: message.into(),
    }
}

pub(super) fn evidence_excerpts(root: &Path, citations: &[ValidatedCitation]) -> EvidenceBundle {
    #[derive(Debug)]
    struct PathRanges {
        path: String,
        ranges: Vec<(u32, u32, usize)>,
    }

    let mut groups: Vec<PathRanges> = Vec::new();
    let mut bundle = EvidenceBundle::default();
    for citation in citations {
        if citation.start_line == 0 || citation.end_line < citation.start_line {
            bundle.omitted_citations += 1;
            bundle.omitted_spans += 1;
            bundle.errors.push(attachment_error(
                &citation.path,
                citation.start_line,
                citation.end_line,
                "invalid source line range",
            ));
            continue;
        }
        let Some(group) = groups.iter_mut().find(|group| group.path == citation.path) else {
            groups.push(PathRanges {
                path: citation.path.clone(),
                ranges: vec![(citation.start_line, citation.end_line, 1)],
            });
            continue;
        };
        group
            .ranges
            .push((citation.start_line, citation.end_line, 1));
    }

    for group in groups {
        let path = match source_path(root, &group.path) {
            Ok(path) => path,
            Err(error) => {
                let message = format!("could not resolve source path: {error}");
                bundle.omitted_citations += group.ranges.iter().map(|range| range.2).sum::<usize>();
                bundle.omitted_spans += group.ranges.len();
                for &(start, end, _) in &group.ranges {
                    bundle
                        .errors
                        .push(attachment_error(&group.path, start, end, &message));
                }
                continue;
            }
        };
        if !path.is_file() {
            let message = "source path is not a regular file";
            bundle.omitted_citations += group.ranges.iter().map(|range| range.2).sum::<usize>();
            bundle.omitted_spans += group.ranges.len();
            for &(start, end, _) in &group.ranges {
                bundle
                    .errors
                    .push(attachment_error(&group.path, start, end, message));
            }
            continue;
        }
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) => {
                let message = format!("could not open source file: {error}");
                bundle.omitted_citations += group.ranges.iter().map(|range| range.2).sum::<usize>();
                bundle.omitted_spans += group.ranges.len();
                for &(start, end, _) in &group.ranges {
                    bundle
                        .errors
                        .push(attachment_error(&group.path, start, end, &message));
                }
                continue;
            }
        };
        let last_requested = group.ranges.iter().map(|range| range.1).max().unwrap_or(0);
        let line_count = match count_requested_lines(&mut file, last_requested) {
            Ok(count) => count,
            Err(error) => {
                let message = format!("could not read source ranges: {error}");
                bundle.omitted_citations += group.ranges.iter().map(|range| range.2).sum::<usize>();
                bundle.omitted_spans += group.ranges.len();
                for &(start, end, _) in &group.ranges {
                    bundle
                        .errors
                        .push(attachment_error(&group.path, start, end, &message));
                }
                continue;
            }
        };
        let mut ranges = group.ranges;
        ranges.sort_by_key(|range| (range.0, range.1));
        let mut merged: Vec<(u32, u32, usize)> = Vec::new();
        for (start, end, count) in ranges {
            if start > line_count || end > line_count {
                bundle.omitted_citations += count;
                bundle.omitted_spans += 1;
                bundle.errors.push(attachment_error(
                    &group.path,
                    start,
                    end,
                    format!(
                        "requested lines are outside the file (last available line: {line_count})"
                    ),
                ));
                continue;
            }
            if let Some(previous) = merged.last_mut() {
                if start <= previous.1.saturating_add(1) {
                    previous.1 = previous.1.max(end);
                    previous.2 += count;
                    continue;
                }
            }
            merged.push((start, end, count));
        }
        for (start_line, end_line, citation_count) in merged {
            match read_source_span(&mut file, start_line, end_line) {
                Ok(Some((text, truncated))) => bundle.spans.push(EvidenceSpan {
                    path: group.path.clone(),
                    start_line,
                    end_line,
                    text,
                    truncated,
                }),
                captured => {
                    bundle.omitted_citations += citation_count;
                    bundle.omitted_spans += 1;
                    let message = match captured {
                        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                            format!("source is not valid UTF-8: {error}")
                        }
                        Err(error) => format!("could not read source range: {error}"),
                        Ok(None) => "requested source range could not be read".into(),
                        Ok(Some(_)) => unreachable!("successful source read was handled above"),
                    };
                    bundle.errors.push(attachment_error(
                        &group.path,
                        start_line,
                        end_line,
                        message,
                    ));
                }
            }
        }
    }
    bundle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn citation(path: &str, start: u32, end: u32) -> ValidatedCitation {
        ValidatedCitation {
            path: path.into(),
            start_line: start,
            end_line: end,
            reason: None,
        }
    }

    #[test]
    fn source_capture_is_not_capped_across_files() {
        let root = tempfile::tempdir().unwrap();
        let citations = (0..3)
            .map(|index| {
                let path = format!("{index}.rs");
                std::fs::write(root.path().join(&path), "x".repeat(80_000)).unwrap();
                citation(&path, 1, 1)
            })
            .collect::<Vec<_>>();
        let bundle = evidence_excerpts(root.path(), &citations);
        assert_eq!(bundle.spans.len(), 3);
        assert!(bundle.spans.iter().all(|span| span.text.len() > 80_000));
        assert!(bundle.spans.iter().all(|span| !span.truncated));
        assert_eq!(bundle.omitted_citations, 0);
    }

    #[test]
    fn selected_long_line_is_delivered_in_full() {
        use std::io::Write;
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"0123456789long line\n").unwrap();
        let (text, truncated) = read_source_span(&mut file, 1, 1).unwrap().unwrap();
        assert_eq!(text, "1: 0123456789long line");
        assert!(!truncated);
    }

    #[test]
    fn invalid_overlapping_ranges_do_not_hide_valid_crlf_source() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "one\r\ntwo\r\nthree").unwrap();
        let bundle = evidence_excerpts(
            root.path(),
            &[citation("lib.rs", 2, 3), citation("lib.rs", 1, 99)],
        );
        assert_eq!(bundle.spans.len(), 1);
        assert_eq!(bundle.spans[0].text, "2: two\n3: three");
        assert!(!bundle.spans[0].truncated);
        assert_eq!(bundle.omitted_citations, 1);
    }

    #[test]
    fn a_bad_span_does_not_discard_other_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("bad.rs"), b"valid\n\xff\n").unwrap();
        std::fs::write(root.path().join("good.rs"), "fn good() {}\n").unwrap();
        let bundle = evidence_excerpts(
            root.path(),
            &[citation("bad.rs", 2, 2), citation("good.rs", 1, 1)],
        );
        assert_eq!(bundle.spans.len(), 1);
        assert_eq!(bundle.spans[0].path, "good.rs");
        assert_eq!(bundle.errors.len(), 1);
        assert!(bundle.errors[0].message.contains("UTF-8"));
    }

    #[test]
    fn absolute_source_paths_are_allowed_and_preserved() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = outside.path().join("dependency.rs");
        std::fs::write(&source, "pub fn dependency() {}\n").unwrap();
        let bundle = evidence_excerpts(
            root.path(),
            &[citation(&source.display().to_string(), 1, 1)],
        );
        assert_eq!(bundle.spans.len(), 1);
        assert_eq!(bundle.spans[0].path, source.display().to_string());
    }
}
