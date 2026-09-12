//! On-demand syntax index using maintained Tree-sitter tag queries.
//! References are name occurrences, never resolved call edges.
use crate::{resolve_path, ToolError, ToolSchema};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tree_sitter_tags::{TagsConfiguration, TagsContext};

const MAX_FILES: usize = 5_000;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;
const MAX_OCCURRENCES: usize = 2_000;
const MAX_CACHED_OCCURRENCES: usize = 50_000;
const MAX_ITEM_BYTES: usize = 20 * 1024;
const QUERY_REVISION: &str = "tags-v1-ts0247-rs0232-py0236-js0231-ts0232-go0234";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolOccurrence {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub definition: bool,
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: usize,
    pub end_byte: usize,
    pub fingerprint: String,
}

#[derive(Clone)]
struct FileRecord {
    fingerprint: String,
    occurrences: Vec<SymbolOccurrence>,
    parse_errors: bool,
    truncated: bool,
}

#[derive(Default)]
struct IndexState {
    files: BTreeMap<String, FileRecord>,
    generation: u64,
}

#[derive(Clone)]
pub struct RepositoryIndex {
    root: PathBuf,
    state: Arc<Mutex<IndexState>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexArgs {
    #[serde(default)]
    symbol: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default = "default_path")]
    path: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_mode() -> String {
    "definitions".into()
}
fn default_path() -> String {
    ".".into()
}
fn default_limit() -> usize {
    30
}

impl RepositoryIndex {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            state: Arc::new(Mutex::new(IndexState::default())),
        }
    }

    pub fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "Symbols".into(),
            description: "Query an on-demand, content-hash refreshed syntax index for Rust, Python, JavaScript, TypeScript/TSX, or Go. Modes: definitions, references, outline. References are same-name syntactic occurrences, NOT resolved callers. Use exact symbol names, optional path relative to the current target or absolute for related evidence, and next_offset for pagination. Read returned ranges to establish behavior; use Grep for unsupported languages, aliases, generated code, and dynamic dispatch. Output reports parse errors, exclusions, and truncation. No source is injected until this tool is called.".into(),
            parameters: json!({"type":"object", "additionalProperties":false, "properties":{
                "symbol":{"type":"string","description":"Exact symbol name; empty lists all symbols in the scope."},
                "mode":{"type":"string","enum":["definitions","references","outline"]},
                "path":{"type":"string","description":"File or directory relative to the current target, or absolute for related evidence; default ."},
                "offset":{"type":"integer","minimum":0},
                "limit":{"type":"integer","minimum":1,"maximum":100}
            }})
        }
    }

    pub async fn call(&self, arguments: &str) -> Result<String, ToolError> {
        Ok(self.call_with_metrics(arguments).await?.0)
    }

    /// Return the normal Symbols JSON plus bounded telemetry as
    /// `(output, parsed_files, reused_files, incomplete, duration_ms,
    /// output_bytes)`. A tuple keeps this additive API available through the
    /// repository-tools facade without exposing another model-facing type.
    pub async fn call_with_metrics(
        &self,
        arguments: &str,
    ) -> Result<(String, u64, u64, bool, u64, u64), ToolError> {
        let args: IndexArgs = serde_json::from_str(if arguments.trim().is_empty() {
            "{}"
        } else {
            arguments
        })
        .map_err(|error| ToolError::InvalidArgs(error.to_string()))?;
        if !["definitions", "references", "outline"].contains(&args.mode.as_str()) {
            return Err(ToolError::InvalidArgs("unknown Symbols mode".into()));
        }
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.query(args))
            .await
            .map_err(|e| ToolError::Message(e.to_string()))?
    }

    fn query(&self, args: IndexArgs) -> Result<(String, u64, u64, bool, u64, u64), ToolError> {
        let started = std::time::Instant::now();
        let root = self.root.canonicalize()?;
        let scope = resolve_path(&root, &args.path).map_err(|e| ToolError::Path(e.to_string()))?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| ToolError::Message("index lock poisoned".into()))?;
        let mut seen = BTreeSet::new();
        let mut verified = BTreeSet::new();
        let mut excluded = Vec::new();
        let mut manifests = Vec::new();
        let mut parsed_files = 0;
        let mut reused_files = 0;
        let mut scan_bytes = 0;
        let mut incomplete = false;
        let mut configs = BTreeMap::new();
        let walker = WalkBuilder::new(&scope)
            .hidden(true)
            .follow_links(false)
            .sort_by_file_path(|a, b| a.cmp(b))
            .filter_entry(|entry| {
                !entry.file_type().is_some_and(|t| t.is_dir())
                    || !matches!(
                        entry.file_name().to_str(),
                        Some(
                            "node_modules"
                                | "target"
                                | "dist"
                                | "build"
                                | "vendor"
                                | "coverage"
                                | "__pycache__"
                        )
                    )
            })
            .build();
        for entry in walker {
            if started.elapsed() > std::time::Duration::from_secs(5) {
                incomplete = true;
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    incomplete = true;
                    if excluded.len() < 50 {
                        excluded.push(e.to_string());
                    }
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            if seen.len() >= MAX_FILES {
                incomplete = true;
                break;
            }
            seen.insert(rel.clone());
            if matches!(
                path.file_name().and_then(|s| s.to_str()),
                Some("Cargo.toml" | "package.json" | "pyproject.toml" | "go.mod")
            ) && manifests.len() < 100
            {
                manifests.push(rel.clone());
            }
            let Some(language) = language_key(path) else {
                continue;
            };
            let canonical =
                resolve_path(&root, &rel).map_err(|e| ToolError::Path(e.to_string()))?;
            let file = std::fs::File::open(canonical)?;
            if file.metadata()?.len() > MAX_FILE_BYTES {
                incomplete = true;
                state.files.remove(&rel);
                if excluded.len() < 50 {
                    excluded.push(format!("{rel}: exceeds 1 MiB"));
                }
                continue;
            }
            let mut source = Vec::new();
            file.take(MAX_FILE_BYTES + 1).read_to_end(&mut source)?;
            scan_bytes += source.len();
            if source.len() as u64 > MAX_FILE_BYTES || scan_bytes > MAX_SCAN_BYTES {
                incomplete = true;
                break;
            }
            let fingerprint = Sha256::digest(&source)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            if state
                .files
                .get(&rel)
                .is_some_and(|r| r.fingerprint == fingerprint)
            {
                verified.insert(rel);
                reused_files += 1;
                continue;
            }
            if !configs.contains_key(language) {
                configs.insert(language, configuration(language)?);
            }
            let config = &configs[language];
            let mut context = TagsContext::new();
            context.parser().set_timeout_micros(100_000);
            let generated = context.generate_tags(config, &source, None);
            let (tags, parse_errors) = match generated {
                Ok(result) => result,
                Err(e) => {
                    incomplete = true;
                    state.files.remove(&rel);
                    if excluded.len() < 50 {
                        excluded.push(format!("{rel}: {e}"));
                    }
                    continue;
                }
            };
            let mut occurrences = Vec::new();
            let mut truncated = false;
            for tag in tags {
                if occurrences.len() >= MAX_OCCURRENCES {
                    truncated = true;
                    break;
                }
                let tag = tag.map_err(|e| ToolError::Message(e.to_string()))?;
                let name = String::from_utf8_lossy(&source[tag.name_range.clone()]).to_string();
                if name.len() > 512 {
                    truncated = true;
                    continue;
                }
                occurrences.push(SymbolOccurrence {
                    path: rel.clone(),
                    name,
                    kind: config.syntax_type_name(tag.syntax_type_id).into(),
                    definition: tag.is_definition,
                    start_line: tag.span.start.row as u32 + 1,
                    end_line: tag.span.end.row as u32 + 1,
                    start_byte: tag.range.start,
                    end_byte: tag.range.end,
                    fingerprint: fingerprint.clone(),
                });
            }
            verified.insert(rel.clone());
            state.files.insert(
                rel,
                FileRecord {
                    fingerprint,
                    occurrences,
                    parse_errors,
                    truncated,
                },
            );
            parsed_files += 1;
        }
        // Only remove records within a fully traversed scope; queries outside it remain cached.
        if !incomplete {
            state
                .files
                .retain(|path, _| !root.join(path).starts_with(&scope) || seen.contains(path));
        }
        state.generation = state.generation.saturating_add(1);
        let mut cached_occurrences = 0;
        state.files.retain(|path, record| {
            cached_occurrences += record.occurrences.len();
            if cached_occurrences > MAX_CACHED_OCCURRENCES {
                incomplete = true;
                verified.remove(path);
                false
            } else {
                true
            }
        });
        let records: Vec<_> = state
            .files
            .iter()
            .filter(|(path, _)| verified.contains(*path) && root.join(path).starts_with(&scope))
            .collect();
        let parse_error_files: Vec<_> = records
            .iter()
            .filter(|(_, r)| r.parse_errors)
            .map(|(p, _)| (*p).clone())
            .collect();
        incomplete |= records.iter().any(|(_, r)| r.truncated);
        let matches: Vec<_> = records
            .iter()
            .flat_map(|(_, r)| r.occurrences.iter())
            .filter(|o| {
                (args.symbol.is_empty() || o.name == args.symbol)
                    && (args.mode == "outline" || o.definition == (args.mode == "definitions"))
            })
            .collect();
        let items = page_items(
            matches.iter().copied(),
            args.offset,
            args.limit.clamp(1, 100),
        )?;
        let next = args.offset.saturating_add(items.len());
        let mut output = json!({"source":"tree-sitter-tags", "query_revision":QUERY_REVISION,
            "relationship_precision":"syntactic occurrences only; no cross-file binding or completeness guarantee",
            "generation":state.generation, "scope":args.path, "items":items,
            "next_offset":if next<matches.len(){Some(next)}else{None}, "matched":matches.len(),
            "index_incomplete":incomplete, "parse_error_files":parse_error_files.into_iter().take(50).collect::<Vec<_>>(),
            "excluded":excluded, "package_manifests":manifests,
            "supported_languages":["rust","python","javascript","typescript","tsx","go"],
            "default_exclusions":"hidden, gitignored, generated/vendor directories, unsupported languages",
            "parsed_files":parsed_files, "reused_files":reused_files});
        // Preserve valid JSON under the tool byte budget, even with long paths.
        if output.to_string().len() > 32 * 1024 {
            output["excluded"] = json!(["Exclusion details omitted to fit output budget"]);
            output["package_manifests"] = json!([]);
            output["parse_error_files"] = json!([]);
            output["index_incomplete"] = json!(true);
        }
        let output = output.to_string();
        let output_bytes = output.len() as u64;
        Ok((
            output,
            parsed_files,
            reused_files,
            incomplete,
            started.elapsed().as_millis() as u64,
            output_bytes,
        ))
    }
}

/// Serialize one page of occurrences under the item byte budget.
///
/// The page always contains at least one item when the requested range is not
/// empty, even if that single occurrence exceeds the budget on its own. An
/// empty page would leave `next_offset` equal to the caller's own offset, so a
/// client that follows `next_offset` would page forever without progress.
fn page_items<'a>(
    matches: impl Iterator<Item = &'a SymbolOccurrence>,
    offset: usize,
    limit: usize,
) -> Result<Vec<serde_json::Value>, ToolError> {
    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut bytes = 0;
    for occurrence in matches.skip(offset).take(limit) {
        let value = serde_json::to_value(occurrence)?;
        let size = value.to_string().len();
        if !items.is_empty() && bytes + size > MAX_ITEM_BYTES {
            break;
        }
        bytes += size;
        items.push(value);
    }
    Ok(items)
}

fn language_key(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "go" => Some("go"),
        _ => None,
    }
}

fn configuration(language: &str) -> Result<TagsConfiguration, ToolError> {
    let (grammar, tags, locals) = match language {
        "rust" => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::TAGS_QUERY.to_string(),
            "",
        ),
        "python" => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::TAGS_QUERY.to_string(),
            "",
        ),
        "javascript" => (
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::TAGS_QUERY.to_string(),
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        "typescript" | "tsx" => (
            if language == "tsx" {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            } else {
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
            },
            format!(
                "{}\n{}",
                tree_sitter_javascript::TAGS_QUERY,
                tree_sitter_typescript::TAGS_QUERY
            ),
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        "go" => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::TAGS_QUERY.to_string(),
            "",
        ),
        _ => return Err(ToolError::InvalidArgs("unsupported index language".into())),
    };
    TagsConfiguration::new(grammar, &tags, locals).map_err(|e| ToolError::Message(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_arguments_list_symbols_and_bad_json_is_invalid_args() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn answer() {}\n").unwrap();
        let index = RepositoryIndex::new(root.path());
        for args in ["", "   ", "{}"] {
            let result: serde_json::Value =
                serde_json::from_str(&index.call(args).await.unwrap()).unwrap();
            assert_eq!(result["items"][0]["name"], "answer");
        }
        assert!(matches!(
            index.call("{").await,
            Err(ToolError::InvalidArgs(_))
        ));
    }
    #[tokio::test]
    async fn five_languages_and_changed_deleted_files() {
        let root = tempfile::tempdir().unwrap();
        for (path, source) in [
            ("a.rs", "fn alpha() {}\n"),
            ("a.py", "def alpha():\n    pass\n"),
            ("a.js", "function alpha() {}\n"),
            ("a.ts", "function alpha(): void {}\n"),
            ("a.go", "package main\nfunc alpha() {}\n"),
        ] {
            std::fs::write(root.path().join(path), source).unwrap();
        }
        let index = RepositoryIndex::new(root.path());
        let query = r#"{"symbol":"alpha"}"#;
        let first: serde_json::Value =
            serde_json::from_str(&index.call(query).await.unwrap()).unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 5, "{first}");
        let second: serde_json::Value =
            serde_json::from_str(&index.call(query).await.unwrap()).unwrap();
        assert_eq!(second["reused_files"], 5);
        std::fs::write(root.path().join("a.rs"), "fn beta() {}\n").unwrap();
        std::fs::remove_file(root.path().join("a.py")).unwrap();
        let third: serde_json::Value =
            serde_json::from_str(&index.call(query).await.unwrap()).unwrap();
        assert_eq!(third["items"].as_array().unwrap().len(), 3);
        assert_eq!(third["parsed_files"], 1);
    }

    #[tokio::test]
    async fn call_with_metrics_reports_parse_and_cache_work() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.rs"), "fn alpha() {}\n").unwrap();
        let index = RepositoryIndex::new(root.path());

        let first = index
            .call_with_metrics(r#"{"symbol":"alpha"}"#)
            .await
            .unwrap();
        assert_eq!(first.1, 1);
        assert_eq!(first.2, 0);
        assert!(!first.3);
        assert_eq!(first.5, first.0.len() as u64);

        let second = index
            .call_with_metrics(r#"{"symbol":"alpha"}"#)
            .await
            .unwrap();
        assert_eq!(second.1, 0);
        assert_eq!(second.2, 1);
        assert!(!second.3);
    }

    fn occurrence(name: &str) -> SymbolOccurrence {
        SymbolOccurrence {
            path: "a.rs".into(),
            name: name.into(),
            kind: "function".into(),
            definition: true,
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 1,
            fingerprint: "f".into(),
        }
    }

    #[test]
    fn a_page_always_advances_past_an_oversized_occurrence() {
        // Serializes to more than the item budget on its own. The name cap in
        // `query` keeps this out of reach today, so the guard is checked here
        // rather than through a repository fixture.
        let huge = occurrence(&"a".repeat(MAX_ITEM_BYTES + 1));
        let small = occurrence("alpha");

        // An empty page would report next_offset == offset and loop a client
        // that follows it.
        let matches = [huge.clone(), small.clone(), small.clone()];
        let first = page_items(matches.iter(), 0, 30).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0]["name"], huge.name);
        assert_eq!(page_items(matches.iter(), 1, 30).unwrap().len(), 2);

        // The budget still ends a page that already carries an item.
        let matches = [small.clone(), huge.clone(), small.clone()];
        assert_eq!(page_items(matches.iter(), 0, 30).unwrap().len(), 1);

        // Ordinary pages are unaffected.
        let matches = [small.clone(), small.clone(), small];
        assert_eq!(page_items(matches.iter(), 0, 30).unwrap().len(), 3);
        assert_eq!(page_items(matches.iter(), 1, 1).unwrap().len(), 1);
        assert!(page_items(matches.iter(), 3, 30).unwrap().is_empty());
    }
}
