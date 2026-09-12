//! Shared investigation contract. Citation locations do not prove claim truth.
use crate::{parse_citations, validate_citation, Citation, ScoutRequest, ValidatedCitation};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationIntent {
    #[default]
    Locate,
    Explain,
    ChangeImpact,
    Diagnose,
    Inventory,
}

impl InvestigationIntent {
    pub fn strategy(self) -> &'static str {
        match self {
            Self::Locate => "Use the query to decide the depth needed. Find the relevant implementation and explain the requested behavior, with surrounding code, callers, or tests where useful.",
            Self::Explain => "Trace the requested behavior through relevant entry points, implementations, callers, and tests. Explain the material transitions and distinguish resolved relationships from textual matches.",
            Self::ChangeImpact => "Follow the target contract through relevant consumers, configuration, compatibility constraints, and tests. Include downstream effects that matter and identify dynamic or unresolved consumers.",
            Self::Diagnose => "Investigate plausible causes and gather evidence that distinguishes them. Separate observations from hypotheses and identify what remains uncertain.",
            Self::Inventory => "Build the requested inventory across the relevant scope. Note coverage limits, exclusions, and whether the result is exhaustive or partial.",
        }
    }

    /// The configured ceiling applies to every investigation intent.
    pub fn turn_limit(self, ceiling: u32) -> u32 {
        ceiling
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationSpec {
    /// Per-request native subscription effort. Omit to use the user's configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Opt in to bounded subscription conversation reuse. Omit for independent questions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub intent: InvestigationIntent,
    /// Optional questions to clarify the objective. If omitted, the query is the sole question.
    #[serde(default)]
    pub questions: Vec<String>,
    /// Context supplied by the parent, not independently verified evidence.
    #[serde(default)]
    pub known_context: String,
    #[serde(default)]
    pub target_paths: Vec<String>,
    /// Internal context for the one bounded adaptive continuation. It is not
    /// part of the caller's serialized request or the validation budget; the
    /// wrapper fills it with the first report and a narrow evidence gap.
    #[serde(skip)]
    pub continuation_context: Option<String>,
    /// Internal native capability evidence used to constrain the first
    /// report's continuation request. Empty means no escalation is permitted.
    #[serde(skip)]
    pub continuation_efforts: Option<Vec<String>>,
}

/// Normalize paths relative to the current target. Evidence elsewhere keeps
/// its absolute location so related checkouts cannot be confused.
fn normalize_request_paths(request: &mut ScoutRequest) -> anyhow::Result<()> {
    let root = canonical_repo_root(&request.root)?;

    if let Some(focus) = request.focus.take() {
        let normalized = normalize_repository_path(&root, &focus.to_string_lossy())?;
        request.focus = (normalized != ".").then(|| PathBuf::from(normalized));
    }

    for target in &mut request.investigation.target_paths {
        *target = normalize_repository_path(&root, target)?;
    }
    Ok(())
}

impl ScoutRequest {
    /// Normalize focus and target paths before validating a scout request.
    pub fn normalize_paths(&mut self) -> anyhow::Result<()> {
        normalize_request_paths(self)
    }
}

pub fn validate_request(request: &ScoutRequest) -> anyhow::Result<()> {
    if let Some(effort) = &request.investigation.reasoning_effort {
        anyhow::ensure!(
            matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max"),
            "unsupported reasoning_effort `{effort}`; use low, medium, high, xhigh, or max"
        );
    }
    if let Some(id) = &request.investigation.conversation_id {
        anyhow::ensure!(
            !id.trim().is_empty() && id.len() <= 128,
            "conversation_id must be nonempty and at most 128 bytes"
        );
    }
    anyhow::ensure!(!request.query.trim().is_empty(), "query is required");
    anyhow::ensure!(request.query.len() <= 16_384, "query exceeds 16 KiB");
    anyhow::ensure!(
        request.investigation.questions.len() <= 24,
        "at most 24 questions are supported"
    );
    anyhow::ensure!(
        request.investigation.known_context.len() <= 16_384,
        "known_context exceeds 16 KiB"
    );
    anyhow::ensure!(
        request.investigation.target_paths.len() <= 32,
        "at most 32 target paths are supported"
    );
    for question in &request.investigation.questions {
        anyhow::ensure!(
            !question.trim().is_empty() && question.len() <= 2_048,
            "questions must be nonempty and at most 2 KiB"
        );
    }

    // Validation accepts safe absolute inputs for callers that do not pass
    // through MCP. The MCP boundary also stores the normalized values before
    // it invokes this function.
    let mut normalized = request.clone();
    normalize_request_paths(&mut normalized).context("cannot resolve target paths or focus")?;
    Ok(())
}

fn canonical_repo_root(root: &Path) -> anyhow::Result<PathBuf> {
    let canonical = root
        .canonicalize()
        .with_context(|| format!("repository root does not exist: {}", root.display()))?;
    anyhow::ensure!(
        canonical.is_dir(),
        "repository root is not a directory: {}",
        root.display()
    );
    Ok(canonical)
}

fn normalize_repository_path(root: &Path, input: &str) -> anyhow::Result<String> {
    let path = Path::new(input);
    let is_dot = path.as_os_str().is_empty()
        || path
            .components()
            .all(|component| matches!(component, Component::CurDir));
    if is_dot {
        return Ok(".".into());
    }

    // On Unix, Path does not recognize a Windows drive or rooted path. Reject
    // those spellings rather than accidentally treating them as repository
    // filenames. On Windows, the normal Component checks below handle them.
    let windows_absolute = looks_windows_absolute(input);
    let is_absolute = path.is_absolute() || windows_absolute;
    if is_absolute {
        anyhow::ensure!(
            !windows_absolute || path.is_absolute(),
            "absolute path is not valid on this host: {input}"
        );
        let canonical = path
            .canonicalize()
            .with_context(|| format!("absolute path does not exist: {input}"))?;
        return repository_relative(root, &canonical);
    }

    anyhow::ensure!(
        !path
            .components()
            .any(|component| matches!(component, Component::Prefix(_))),
        "path has an unsupported prefix: {input}"
    );

    let candidate = root.join(path);
    let resolved = resolve_with_existing_parent(&candidate, input)?;
    repository_relative(root, &resolved)
}

fn resolve_with_existing_parent(candidate: &Path, input: &str) -> anyhow::Result<PathBuf> {
    let mut current = candidate.to_path_buf();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(_) => {
                let canonical = current
                    .canonicalize()
                    .with_context(|| format!("cannot resolve path: {input}"))?;
                let mut resolved = canonical;
                for component in missing.iter().rev() {
                    resolved.push(component.as_os_str());
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = current
                    .file_name()
                    .with_context(|| format!("invalid repository path: {input}"))?;
                missing.push(name.to_os_string());
                current = current
                    .parent()
                    .with_context(|| format!("invalid repository path: {input}"))?
                    .to_path_buf();
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect path: {input}"));
            }
        }
    }
}

fn repository_relative(root: &Path, path: &Path) -> anyhow::Result<String> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative.as_os_str().is_empty() {
        return Ok(".".into());
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn looks_windows_absolute(input: &str) -> bool {
    let bytes = input.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || input.starts_with('\\')
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationStatus {
    Complete,
    #[default]
    Partial,
    NotFound,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub question: String,
    pub answer: String,
    pub citations: Vec<ValidatedCitation>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    High,
    Medium,
    Low,
    #[default]
    Unknown,
}

/// Scout-reported evidence assessment, not a calibrated probability or verifier verdict.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InvestigationConfidence {
    pub level: ConfidenceLevel,
    pub basis: String,
}

/// A single, explicit request for a deeper follow-up pass.
///
/// This is intentionally a narrow output contract.  The scout must identify a
/// concrete evidence gap and a question that the follow-up can answer; a
/// confidence label, query wording, or broad complexity heuristic is not
/// enough to trigger another model turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningContinuation {
    pub effort: String,
    pub question: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InvestigationReport {
    pub intent: InvestigationIntent,
    pub status: InvestigationStatus,
    pub findings: Vec<Finding>,
    pub unresolved: Vec<String>,
    pub searched_scope: Vec<String>,
    pub limitations: Vec<String>,
    #[serde(default)]
    pub confidence: InvestigationConfidence,
    /// At most one targeted continuation may be requested by a report. The
    /// CLI wrapper validates the effort against native model capability before
    /// issuing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ReasoningContinuation>,
}

#[derive(Deserialize)]
struct ModelFinding {
    question: String,
    answer: String,
    citations: Vec<Citation>,
}

#[derive(Deserialize)]
struct ModelReport {
    answer: String,
    #[serde(default)]
    citations: Vec<Citation>,
    // Accept older reports without requiring their writing template.
    #[serde(default)]
    status: Option<InvestigationStatus>,
    #[serde(default)]
    findings: Vec<ModelFinding>,
    #[serde(default)]
    unresolved: Vec<String>,
    #[serde(default)]
    searched_scope: Vec<String>,
    #[serde(default)]
    limitations: Vec<String>,
    #[serde(default)]
    confidence: InvestigationConfidence,
    #[serde(default, alias = "reasoning_continuation")]
    continuation: Option<ReasoningContinuation>,
}

pub fn questions(request: &ScoutRequest) -> Vec<String> {
    if request.investigation.questions.is_empty() {
        vec![request.query.clone()]
    } else {
        request.investigation.questions.clone()
    }
}

pub fn investigation_prompt(request: &ScoutRequest) -> String {
    let context = json!({
        "objective": request.query,
        "repository": request.root,
        "intent": request.investigation.intent,
        "known_context_unverified": request.investigation.known_context,
        "questions": request.investigation.questions,
        "target_paths": request.investigation.target_paths,
        "focus": request.focus,
        "prior_findings_unverified": request.investigation.continuation_context,
        "supported_continuation_efforts": request.investigation.continuation_efforts,
    });
    format!(
        "{}\n\nInvestigation input:\n{}\n\nUse the query as the primary objective. Optional fields are hints. Discover useful leads yourself; the parent supplies what it already knows. Write one coherent answer in whatever structure fits the task, including deciding relationships, useful related discoveries and specific uncertainty. Use citations to select source for attachment, not to duplicate it in prose. For experiments, include the relevant command, inputs, result and what it establishes; citations may be empty. Do not repeat the answer in separate findings or supply operational metadata. Return JSON matching the schema below. If a supported higher effort would resolve a specific reasoning gap, request one continuation with its question and reason; otherwise use null. A continuation must update the complete answer for the original objective, preserving supported evidence and any remaining uncertainty.\n{}",
        request.investigation.intent.strategy(), context, investigation_output_schema()
    )
}

pub fn investigation_output_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "answer": {"type": "string", "description": "The useful investigation answer, including evidence, relevant experiments and specific uncertainties. Choose its structure for the assignment."},
            "citations": {"type": "array", "description": "Source ranges the parent needs, in reading order. Empty is valid for answers supported by experiments or other evidence in the answer.", "items": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "path": {"type": "string", "description": "Source file path relative to the current target, or absolute for evidence elsewhere. Identify the actual checkout."},
                    "start_line": {"type": "integer", "minimum": 1},
                    "end_line": {"type": "integer", "minimum": 1},
                    "reason": {"type": "string", "description": "What this code supports."}
                },
                "required": ["path", "start_line", "end_line", "reason"]
            }},
            "continuation": {"anyOf": [{
                "type": "object", "additionalProperties": false,
                "properties": {
                    "effort": {"type": "string", "description": "A supported higher effort from the supplied list."},
                    "question": {"type": "string", "description": "The specific unresolved relationship."},
                    "reason": {"type": "string", "description": "Why more reasoning would help after useful investigation."}
                },
                "required": ["effort", "question", "reason"]
            }, {"type": "null"}]}
        },
        "required": ["answer", "citations", "continuation"]
    })
}

/// Validate output integrity and citation locations. Status and unresolved
/// questions remain the scout's report; claim truth remains the caller's
/// responsibility.
pub fn assess_output(
    request: &ScoutRequest,
    raw: &str,
) -> (String, Vec<ValidatedCitation>, InvestigationReport) {
    let raw = raw.trim();
    let raw = raw
        .strip_prefix("```json")
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(raw)
        .trim();
    let mut report = InvestigationReport {
        intent: request.investigation.intent,
        ..Default::default()
    };
    let mut all_citations = Vec::new();
    let summary = match serde_json::from_str::<ModelReport>(raw) {
        Ok(model) => {
            report.status = model.status.unwrap_or(InvestigationStatus::Complete);
            report.unresolved = model.unresolved;
            report.searched_scope = model.searched_scope;
            report.limitations = model.limitations;
            report.confidence = model.confidence;
            report.continuation = model.continuation;
            for citation in model.citations {
                attach_citation(
                    request,
                    &citation,
                    &mut all_citations,
                    &mut report.limitations,
                );
            }
            for finding in model.findings {
                let mut citations = Vec::new();
                for citation in finding.citations {
                    if let Some(valid) = attach_citation(
                        request,
                        &citation,
                        &mut all_citations,
                        &mut report.limitations,
                    ) {
                        citations.push(valid);
                    }
                }
                report.findings.push(Finding {
                    question: finding.question,
                    answer: finding.answer,
                    citations,
                });
            }
            model.answer
        }
        Err(_) => {
            let (answer, citations) = parse_citations(raw);
            for citation in citations {
                attach_citation(
                    request,
                    &citation,
                    &mut all_citations,
                    &mut report.limitations,
                );
            }
            // Preserve a useful plain answer; structured attachment parsing is
            // an integration concern, not a reason to repeat paid investigation.
            report.status = InvestigationStatus::Complete;
            if answer.trim().is_empty() && all_citations.is_empty() {
                raw.to_owned()
            } else {
                answer
            }
        }
    };
    if summary.trim().is_empty() || !report.unresolved.is_empty() || report.continuation.is_some() {
        report.status = InvestigationStatus::Partial;
    }
    if report.confidence.basis.trim().is_empty() {
        report.confidence = InvestigationConfidence::default();
    }
    (summary, all_citations, report)
}

fn attach_citation(
    request: &ScoutRequest,
    citation: &Citation,
    all: &mut Vec<ValidatedCitation>,
    limitations: &mut Vec<String>,
) -> Option<ValidatedCitation> {
    match validate_citation(&request.root, citation)
        .filter(|valid| valid.end_line == citation.end_line)
    {
        Some(valid) => {
            if !all.iter().any(|old| {
                old.path == valid.path
                    && old.start_line == valid.start_line
                    && old.end_line == valid.end_line
            }) {
                all.push(valid.clone());
            }
            Some(valid)
        }
        None => {
            limitations.push(format!(
                "Source could not be attached: {}:{}-{} (unavailable file or invalid range).",
                citation.path, citation.start_line, citation.end_line
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_effort_is_optional_and_validated() {
        let root = tempfile::tempdir().unwrap();
        let mut request = request(root.path());
        validate_request(&request).unwrap();
        for effort in ["low", "medium", "high", "xhigh", "max"] {
            request.investigation.reasoning_effort = Some(effort.into());
            validate_request(&request).unwrap();
        }
        request.investigation.reasoning_effort = Some("whatever".into());
        assert!(validate_request(&request).is_err());
    }

    #[test]
    fn legacy_confidence_is_not_a_program_verified_verdict() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn start() {}\n").unwrap();
        let request = request(root.path());
        let mut raw = json!({"answer":"start", "status":"complete", "findings":[{
            "question":"entry", "answer":"start is defined here", "citations":[
                {"path":"lib.rs", "start_line":1,"end_line":1,"reason":"entry"}]
        }], "unresolved":[], "searched_scope":["lib.rs"], "limitations":[],
        "confidence":{"level":"high","basis":"Read the definition. Runtime was not tested."}});
        let report = assess_output(&request, &raw.to_string()).2;
        assert_eq!(report.confidence.level, ConfidenceLevel::High);
        assert!(report.confidence.basis.contains("Runtime was not tested"));

        raw["confidence"]["basis"] = json!(" ");
        assert_eq!(
            assess_output(&request, &raw.to_string()).2.confidence.level,
            ConfidenceLevel::Unknown
        );
        raw["confidence"]["basis"] = json!("Read the definition.");
        raw["findings"][0]["citations"][0]["end_line"] = json!(10);
        let report = assess_output(&request, &raw.to_string()).2;
        assert_eq!(report.confidence.level, ConfidenceLevel::High);
        assert_eq!(report.status, InvestigationStatus::Complete);
        assert!(!report.limitations.is_empty());
        raw.as_object_mut().unwrap().remove("confidence");
        assert_eq!(
            assess_output(&request, &raw.to_string()).2.confidence.level,
            ConfidenceLevel::Unknown
        );
    }

    fn request(root: &Path) -> ScoutRequest {
        ScoutRequest {
            query: "trace".into(),
            root: root.to_path_buf(),
            focus: None,
            timeout: None,
            max_turns: None,
            investigation: InvestigationSpec::default(),
        }
    }

    #[test]
    fn omitted_and_dot_focus_use_the_repository_root() {
        let root = tempfile::tempdir().unwrap();
        let mut omitted = request(root.path());
        normalize_request_paths(&mut omitted).unwrap();
        assert_eq!(omitted.focus, None);

        let mut explicit = request(root.path());
        explicit.focus = Some(PathBuf::from("."));
        normalize_request_paths(&mut explicit).unwrap();
        assert_eq!(explicit.focus, None);
    }

    #[test]
    fn absolute_paths_inside_root_become_relative() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "fn start() {}\n").unwrap();
        let mut request = request(root.path());
        request.focus = Some(root.path().join("src"));
        request.investigation.target_paths =
            vec![root.path().join("src/lib.rs").display().to_string()];

        normalize_request_paths(&mut request).unwrap();

        assert_eq!(request.focus, Some(PathBuf::from("src")));
        assert_eq!(request.investigation.target_paths, ["src/lib.rs"]);
        validate_request(&request).unwrap();
    }

    #[test]
    fn relative_nonexistent_target_remains_a_repository_hint() {
        let root = tempfile::tempdir().unwrap();
        let mut request = request(root.path());
        request.investigation.target_paths = vec!["src/new_module.rs".into()];

        normalize_request_paths(&mut request).unwrap();

        assert_eq!(request.investigation.target_paths, ["src/new_module.rs"]);
        validate_request(&request).unwrap();
    }

    #[test]
    fn external_target_paths_keep_explicit_provenance() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let outside = parent.path().join("secret.rs");
        std::fs::write(&outside, "secret\n").unwrap();

        let mut traversal = request(&root);
        traversal.focus = Some(PathBuf::from("../secret.rs"));
        normalize_request_paths(&mut traversal).unwrap();
        assert_eq!(traversal.focus, Some(outside.canonicalize().unwrap()));

        let mut absolute_outside = request(&root);
        absolute_outside.investigation.target_paths = vec![outside.display().to_string()];
        normalize_request_paths(&mut absolute_outside).unwrap();
        assert_eq!(
            absolute_outside.investigation.target_paths,
            [outside
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn related_symlink_targets_are_identified_by_real_location() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let outside = parent.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();

        let mut focus = request(&root);
        focus.focus = Some(PathBuf::from("linked"));
        normalize_request_paths(&mut focus).unwrap();
        assert_eq!(focus.focus, Some(outside.canonicalize().unwrap()));

        let mut missing = request(&root);
        missing.investigation.target_paths = vec!["linked/new.rs".into()];
        normalize_request_paths(&mut missing).unwrap();
        assert_eq!(
            missing.investigation.target_paths,
            [outside
                .canonicalize()
                .unwrap()
                .join("new.rs")
                .to_string_lossy()
                .to_string()]
        );
    }

    #[test]
    fn explicit_unresolved_question_keeps_partial_status() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn start() {}\n").unwrap();
        let request = ScoutRequest {
            query: "trace".into(),
            root: root.path().into(),
            focus: None,
            timeout: None,
            max_turns: None,
            investigation: InvestigationSpec {
                questions: vec!["entry?".into(), "cleanup?".into()],
                ..Default::default()
            },
        };
        let raw = json!({"answer":"done", "status":"complete", "findings":[{
            "question":"entry?", "answer":"start", "citations":[{"path":"lib.rs", "start_line":1,"end_line":1,"reason":"entry"}]
        }], "unresolved":["cleanup?"], "searched_scope":["lib.rs"], "limitations":[]}).to_string();
        let (_, citations, report) = assess_output(&request, &raw);
        assert_eq!(citations.len(), 1);
        assert_eq!(report.status, InvestigationStatus::Partial);
        assert_eq!(report.unresolved, ["cleanup?"]);
    }

    #[test]
    fn legacy_not_found_is_a_model_report_not_a_program_verdict() {
        let request = ScoutRequest {
            query: "missing".into(),
            root: ".".into(),
            focus: None,
            timeout: None,
            max_turns: None,
            investigation: InvestigationSpec::default(),
        };
        let raw = json!({"answer":"absent", "status":"not_found", "findings":[],
            "unresolved":[], "searched_scope":[], "limitations":[]})
        .to_string();
        assert_eq!(
            assess_output(&request, &raw).2.status,
            InvestigationStatus::NotFound
        );
    }

    #[test]
    fn concise_finding_label_can_cover_multiple_questions() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn start() {}\n").unwrap();
        let request = ScoutRequest {
            investigation: InvestigationSpec {
                questions: vec!["where is entry?".into(), "what does it do?".into()],
                ..Default::default()
            },
            ..request(root.path())
        };
        let raw = json!({"answer":"start is defined in lib.rs", "status":"complete", "findings":[{
            "question":"entry implementation", "answer":"The entry function is start.",
            "citations":[{"path":"lib.rs", "start_line":1,"end_line":1,"reason":"entry"}]
        }], "unresolved":[], "searched_scope":["lib.rs"], "limitations":[]})
        .to_string();

        let (_, _, report) = assess_output(&request, &raw);
        assert_eq!(report.status, InvestigationStatus::Complete);
        assert!(report.unresolved.is_empty());
    }

    #[test]
    fn invalid_citation_keeps_the_answer_and_reports_attachment_failure() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn start() {}\n").unwrap();
        let request = request(root.path());
        let raw = json!({"answer":"start", "status":"complete", "findings":[{
            "question":"entry", "answer":"The entry function is start.",
            "citations":[{"path":"lib.rs", "start_line":1,"end_line":2,"reason":"entry"}]
        }], "unresolved":[], "searched_scope":["lib.rs"], "limitations":[]})
        .to_string();

        let (answer, citations, report) = assess_output(&request, &raw);
        assert_eq!(answer, "start");
        assert!(citations.is_empty());
        assert_eq!(report.status, InvestigationStatus::Complete);
        assert!(report
            .limitations
            .iter()
            .any(|limitation| limitation.contains("Source could not be attached: lib.rs:1-2")));
        assert!(report.unresolved.is_empty());
    }

    #[test]
    fn intent_does_not_reduce_the_configured_turn_ceiling() {
        for intent in [
            InvestigationIntent::Locate,
            InvestigationIntent::Explain,
            InvestigationIntent::ChangeImpact,
            InvestigationIntent::Diagnose,
            InvestigationIntent::Inventory,
        ] {
            assert_eq!(intent.turn_limit(9), 9);
        }
    }

    #[test]
    fn requirements_and_existing_code_assumptions_remain_distinct() {
        let root = tempfile::tempdir().unwrap();
        let mut request = request(root.path());
        request.query = "Find change points. Requirements: reject multi-file writes.".into();
        request.investigation.known_context = "I think writes use the shared loader.".into();
        let prompt = investigation_prompt(&request);
        assert!(prompt.contains(&request.query));
        assert!(prompt.contains("known_context_unverified"));
        assert!(prompt.contains(&request.investigation.known_context));
        let system = include_str!("../prompts/system.md");
        assert!(system.contains("not claims that the repository already implements it"));
        assert!(system.contains("Do not reopen decisions supplied in the request"));
        assert!(system.contains("Missing requirement:"));
    }

    #[test]
    fn explicit_continuation_is_preserved_as_a_targeted_gap() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn start() {}\n").unwrap();
        let request = request(root.path());
        let raw = json!({
            "answer": "The entry point is in lib.rs.",
            "status": "partial",
            "confidence": {"level": "medium", "basis": "The caller was not traced."},
            "findings": [{"question": "entry", "answer": "start is defined here.",
                "citations": [{"path": "lib.rs", "start_line": 1, "end_line": 1, "reason": "entry"}]}],
            "unresolved": ["Which override wins?"],
            "searched_scope": ["lib.rs"],
            "limitations": [],
            "continuation": {"effort": "max", "question": "Which override wins?", "reason": "Two traced override branches disagree at their merge boundary."}
        }).to_string();
        let report = assess_output(&request, &raw).2;
        assert_eq!(report.continuation.as_ref().unwrap().effort, "max");
        assert_eq!(
            report.continuation.as_ref().unwrap().question,
            "Which override wins?"
        );
    }

    #[test]
    fn completed_change_map_is_not_downgraded_for_an_unbuilt_feature() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn load_one() {}\n").unwrap();
        let request = request(root.path());
        let mut raw = json!({
            "answer": "Requirements specify later files win. Add a multi-file loader beside load_one.",
            "status": "complete",
            "confidence": {"level": "high", "basis": "Read the single-file loader; tests not executed."},
            "findings": [{"question": "change location", "answer": "load_one is the existing entry point.",
                "citations": [{"path": "lib.rs", "start_line": 1, "end_line": 1, "reason": "existing loader"}]}],
            "unresolved": [], "searched_scope": ["lib.rs"],
            "limitations": ["The proposed feature is not implemented. An optional multi-file writer is outside this request."]
        });
        assert_eq!(
            assess_output(&request, &raw.to_string()).2.status,
            InvestigationStatus::Complete
        );
        raw["unresolved"] =
            json!(["Existing behavior: generated caller precedence could not be inspected."]);
        assert_eq!(
            assess_output(&request, &raw.to_string()).2.status,
            InvestigationStatus::Partial
        );
        raw["unresolved"] =
            json!(["Missing requirement: which file may a multi-file write modify?"]);
        assert_eq!(
            assess_output(&request, &raw.to_string()).2.status,
            InvestigationStatus::Partial
        );
    }

    #[test]
    fn investigation_prompt_allows_concise_labels_and_query_only_input() {
        let root = tempfile::tempdir().unwrap();
        let prompt = investigation_prompt(&request(root.path()));
        assert!(prompt.contains("query as the primary objective"));
        assert!(prompt.contains("one coherent answer"));
        assert!(!prompt.contains("Copy question strings exactly"));
    }

    #[test]
    fn coherent_experiment_answer_needs_no_source_citation() {
        let root = tempfile::tempdir().unwrap();
        let raw = json!({"answer": "Ran the parser on an empty field: it returned an empty string. This reproduces the behavior, not the cause.", "citations": [], "continuation": null});
        let (answer, citations, report) = assess_output(&request(root.path()), &raw.to_string());
        assert_eq!(answer, raw["answer"].as_str().unwrap());
        assert!(citations.is_empty());
        assert!(report.limitations.is_empty());
        assert_eq!(report.status, InvestigationStatus::Complete);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn minimal_schema_does_not_require_operational_or_repeated_report_fields() {
        let schema = investigation_output_schema();
        assert_eq!(
            schema["required"],
            json!(["answer", "citations", "continuation"])
        );
        for field in [
            "findings",
            "confidence",
            "status",
            "stats",
            "repository",
            "next_action",
        ] {
            assert!(schema["properties"].get(field).is_none());
        }
    }

    #[test]
    fn external_source_is_attached_with_its_actual_absolute_location() {
        let root = tempfile::tempdir().unwrap();
        let dependency = tempfile::tempdir().unwrap();
        let path = dependency.path().join("library.rs");
        std::fs::write(&path, "fn external() {}\n").unwrap();
        let raw = json!({"answer": "The dependency exports external.", "citations": [{"path": path, "start_line": 1, "end_line": 1}], "continuation": null});
        let (_, citations, _) = assess_output(&request(root.path()), &raw.to_string());
        assert_eq!(
            citations[0].path,
            path.canonicalize()
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        );
    }
}
