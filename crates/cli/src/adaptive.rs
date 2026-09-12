//! One bounded, evidence-driven higher-effort follow-up.
//!
//! The wrapper is deliberately transport-agnostic. Native backends retain
//! ownership of authentication, native tools, and their own budgets;
//! this layer only decides whether the first structured report explicitly asks
//! for one targeted continuation and then replays the request with its prior
//! findings as context.

use anyhow::Result;
use async_trait::async_trait;
use repotracer_core::{
    ConfidenceLevel, InvestigationConfidence, InvestigationStatus, ReasoningContinuation,
    ScoutAttemptStats, ScoutBackend, ScoutBackendError, ScoutRequest, ScoutResult, ScoutStats,
    UsageStats, UsageStatus,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static ADAPTIVE_CONVERSATION: AtomicU64 = AtomicU64::new(1);

const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// A backend decorator which allows at most one explicit targeted escalation.
pub struct AdaptiveScout {
    backend: Arc<dyn ScoutBackend>,
    enabled: bool,
    /// `None` is unknown capability and therefore permits no escalation.
    supported_efforts: Option<Vec<String>>,
    current_effort: String,
    /// A positive configured explorer ceiling is owned by the provider run;
    /// do not silently spend it twice across two automatic requests.
    turn_ceiling: Option<u32>,
}

impl AdaptiveScout {
    pub fn new(
        backend: Arc<dyn ScoutBackend>,
        enabled: bool,
        supported_efforts: Option<Vec<String>>,
        current_effort: impl Into<String>,
    ) -> Self {
        Self {
            backend,
            enabled,
            supported_efforts,
            current_effort: current_effort.into(),
            turn_ceiling: None,
        }
    }

    pub fn with_turn_ceiling(mut self, ceiling: u32) -> Self {
        self.turn_ceiling = (ceiling > 0).then_some(ceiling);
        self
    }

    fn continuation_request(
        &self,
        original: &ScoutRequest,
        first: &ScoutResult,
        continuation: &ReasoningContinuation,
    ) -> ScoutRequest {
        let mut request = original.clone();
        request.investigation.reasoning_effort = Some(continuation.effort.clone());
        // This is the one and only continuation; do not invite the second
        // provider request to recurse if it emits the optional field again.
        request.investigation.continuation_efforts = Some(Vec::new());
        let prior = compact_previous_context(first);
        request.investigation.continuation_context = Some(format!(
            "RepoTracer first-pass findings (unverified context; preserve the original request above):\n{}\n\nTargeted evidence gap to resolve in this one continuation:\nQuestion: {}\nReason: {}",
            prior, continuation.question, continuation.reason
        ));
        request
    }

    fn ensure_conversation_id(request: &mut ScoutRequest) {
        if request.investigation.conversation_id.is_none() {
            let sequence = ADAPTIVE_CONVERSATION.fetch_add(1, Ordering::Relaxed);
            request.investigation.conversation_id =
                Some(format!("adaptive-{}-{}", std::process::id(), sequence));
        }
    }

    fn valid_target(
        &self,
        continuation: &ReasoningContinuation,
        current_effort: &str,
    ) -> Result<(), &'static str> {
        if !EFFORTS.contains(&continuation.effort.as_str()) {
            return Err("the requested reasoning effort is invalid");
        }
        if continuation.question.trim().is_empty() || continuation.reason.trim().is_empty() {
            return Err("the continuation must name a question and evidence gap");
        }
        if effort_rank(&continuation.effort) <= effort_rank(current_effort) {
            return Err("the continuation must request a higher effort than the configured effort");
        }
        let Some(supported) = &self.supported_efforts else {
            return Err("native model capability for higher effort is unavailable");
        };
        if !supported
            .iter()
            .any(|effort| effort == &continuation.effort)
        {
            return Err("the native model does not report support for the requested effort");
        }
        Ok(())
    }

    fn annotate_rejected(
        &self,
        mut result: ScoutResult,
        continuation: &ReasoningContinuation,
        reason: &str,
    ) -> ScoutResult {
        result.investigation.continuation = None;
        result.investigation.status = InvestigationStatus::Partial;
        if !result
            .investigation
            .unresolved
            .iter()
            .any(|question| question == &continuation.question)
        {
            result
                .investigation
                .unresolved
                .push(continuation.question.clone());
        }
        result.investigation.limitations.push(format!(
            "The requested higher-effort continuation did not resolve the gap: {reason}."
        ));
        result
    }

    fn attempt(effort: &str, stats: &ScoutStats, succeeded: bool) -> ScoutAttemptStats {
        ScoutAttemptStats {
            reasoning_effort: effort.to_string(),
            succeeded,
            usage: stats.usage.clone(),
            usage_status: stats.usage_status,
            reported_cost_usd: stats.reported_cost_usd,
            tool_calls: stats.tool_calls,
            duration_ms: stats.duration_ms,
            warm_process: stats.warm_process,
            thread_turn: stats.thread_turn,
            index_usage: stats.index_usage.clone(),
        }
    }
}

#[async_trait]
impl ScoutBackend for AdaptiveScout {
    async fn scout(&self, request: ScoutRequest) -> Result<ScoutResult> {
        let mut first_request = request.clone();
        // A continuation needs a stable native conversation. Generate an
        // internal id only for adaptive mode; ordinary requests remain byte
        // for byte unchanged.
        if self.enabled {
            Self::ensure_conversation_id(&mut first_request);
        }
        let baseline_effort = first_request
            .investigation
            .reasoning_effort
            .as_deref()
            .unwrap_or(&self.current_effort);
        first_request.investigation.continuation_efforts = Some(
            if self.enabled
                && request.max_turns.is_none_or(|turns| turns == 0)
                && self.turn_ceiling.is_none()
            {
                self.supported_efforts
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|effort| effort_rank(effort) > effort_rank(baseline_effort))
                    .collect()
            } else {
                Vec::new()
            },
        );
        let mut first = self.backend.scout(first_request.clone()).await?;
        if !self.enabled {
            return Ok(first);
        }
        if first.investigation.status == InvestigationStatus::Complete {
            first.investigation.continuation = None;
            return Ok(first);
        }
        let Some(continuation) = first.investigation.continuation.clone() else {
            return Ok(first);
        };
        let first_effort = first
            .stats
            .reasoning_effort
            .as_deref()
            .or(request.investigation.reasoning_effort.as_deref())
            .unwrap_or(&self.current_effort);
        if let Err(reason) = self.valid_target(&continuation, first_effort) {
            return Ok(self.annotate_rejected(first, &continuation, reason));
        }
        if request.max_turns.is_some_and(|turns| turns > 0) || self.turn_ceiling.is_some() {
            return Ok(self.annotate_rejected(
                first,
                &continuation,
                "the caller supplied an explicit turn budget, so automatic continuation would exceed it",
            ));
        }

        // Use the exact conversation id sent to the first provider call. If
        // the caller omitted one, the wrapper generated a private id solely
        // so this bounded follow-up can reuse the same native thread where
        // that backend supports it.
        let second_request = self.continuation_request(&first_request, &first, &continuation);
        let second_effort = continuation.effort.clone();
        let first_effort = first_effort.to_string();
        match self.backend.scout(second_request).await {
            Ok(mut second) if second.investigation.status != InvestigationStatus::Failed => {
                // `merge_results` carries the two provider calls as separate,
                // nonrecursive attempt records and returns the latest report
                // with prior context retained as historical background.
                first.stats.reasoning_effort = Some(first_effort.clone());
                second.stats.reasoning_effort = Some(second_effort.clone());
                if second.investigation.status == InvestigationStatus::Complete
                    && (second.summary.trim().is_empty()
                        || !second.investigation.unresolved.is_empty()
                        || second.investigation.continuation.is_some())
                {
                    second.investigation.status = InvestigationStatus::Partial;
                    second.investigation.limitations.push(
                        "The continuation left a material question unresolved or returned no answer.".into(),
                    );
                }
                let next_question = second
                    .investigation
                    .continuation
                    .as_ref()
                    .map(|next| next.question.clone());
                if let Some(next_question) = next_question {
                    if !second
                        .investigation
                        .unresolved
                        .iter()
                        .any(|question| question == &next_question)
                    {
                        second.investigation.unresolved.push(next_question);
                    }
                    second.investigation.limitations.push(
                        "Only one adaptive continuation is allowed; a further request was not run."
                            .into(),
                    );
                    second.investigation.status = InvestigationStatus::Partial;
                }
                Ok(merge_results(first, second))
            }
            Ok(second) => {
                let mut second = second;
                second.stats.reasoning_effort = Some(second_effort.clone());
                let mut result = self.annotate_rejected(
                    first,
                    &continuation,
                    "the targeted continuation returned no complete evidence",
                );
                result.stats = aggregate_pair(&result.stats, &second.stats, false);
                Ok(result)
            }
            Err(error) => {
                let error_stats = error
                    .downcast_ref::<ScoutBackendError>()
                    .map(|error| &error.stats);
                let mut result = self.annotate_rejected(
                    first,
                    &continuation,
                    "the targeted continuation failed before resolving the evidence gap",
                );
                result.stats = aggregate_pair_with_unknown(
                    &result.stats,
                    error_stats,
                    &first_effort,
                    &second_effort,
                );
                Ok(result)
            }
        }
    }
}

fn effort_rank(effort: &str) -> usize {
    EFFORTS
        .iter()
        .position(|candidate| candidate == &effort)
        .unwrap_or(usize::MAX)
}

fn add_usage(first: &UsageStats, second: &UsageStats) -> UsageStats {
    UsageStats {
        input_tokens: add_dimension(first.input_tokens, second.input_tokens),
        cached_input_tokens: add_dimension(first.cached_input_tokens, second.cached_input_tokens),
        cache_write_input_tokens: add_dimension(
            first.cache_write_input_tokens,
            second.cache_write_input_tokens,
        ),
        output_tokens: add_dimension(first.output_tokens, second.output_tokens),
        reasoning_output_tokens: add_dimension(
            first.reasoning_output_tokens,
            second.reasoning_output_tokens,
        ),
        total_tokens: add_dimension(first.total_tokens, second.total_tokens),
    }
}

fn add_dimension(first: Option<u32>, second: Option<u32>) -> Option<u32> {
    first?.checked_add(second?)
}

fn aggregate_index_usage(
    first: Option<&repotracer_core::IndexUsage>,
    second: Option<&repotracer_core::IndexUsage>,
) -> Option<repotracer_core::IndexUsage> {
    let (Some(first), Some(second)) = (first, second) else {
        return None;
    };
    Some(repotracer_core::IndexUsage {
        available: first.available || second.available,
        calls: first.calls.saturating_add(second.calls),
        failed_calls: first.failed_calls.saturating_add(second.failed_calls),
        parsed_files: first.parsed_files.saturating_add(second.parsed_files),
        reused_files: first.reused_files.saturating_add(second.reused_files),
        incomplete_calls: first
            .incomplete_calls
            .saturating_add(second.incomplete_calls),
        duration_ms: first.duration_ms.saturating_add(second.duration_ms),
        output_bytes: first.output_bytes.saturating_add(second.output_bytes),
    })
}

fn merge_results(first: ScoutResult, mut second: ScoutResult) -> ScoutResult {
    let first_stats = first.stats.clone();
    let second_stats = second.stats.clone();
    let previous_context = compact_previous_context(&first);
    let identical = reports_identical(&first, &second);
    let second_status = second.investigation.status;

    // The latest report owns current findings. Earlier findings remain labelled
    // narrative background, not additional current conclusions.
    second.investigation.continuation = None;
    // Keep source for that background available to the handoff renderer. It
    // re-reads these locations from disk; the label does not certify that the
    // earlier interpretation still holds. Current evidence takes priority.
    for mut prior in first.citations {
        if !second.citations.iter().any(|current| {
            current.path == prior.path
                && current.start_line == prior.start_line
                && current.end_line == prior.end_line
        }) {
            prior.reason = Some(format!(
                "Previous-pass background, interpretation not revalidated: {}",
                prior.reason.as_deref().unwrap_or("source location")
            ));
            second.citations.push(prior);
        }
    }

    if !identical {
        let current_summary = if second.summary.trim().is_empty() {
            "(The continuation returned no summary.)"
        } else {
            second.summary.trim()
        };
        second.summary = format!(
            "Current continuation report:\n{current_summary}\n\nPrevious-pass context, not revalidated by the continuation; current findings may supersede it:\n{previous_context}"
        );
        second.investigation.limitations.push(
            "Previous-pass context is included as background only; it was not revalidated by the continuation and is not certified as current. Current findings may supersede it.".into(),
        );
    }

    if matches!(
        second_status,
        InvestigationStatus::Partial | InvestigationStatus::NotFound
    ) {
        second.investigation.status = InvestigationStatus::Partial;
        second.investigation.confidence = InvestigationConfidence {
            level: ConfidenceLevel::Unknown,
            basis: format!(
                "The continuation returned {second_status:?}. Its current findings are retained, but prior-pass context was not revalidated; this result does not certify that no findings exist outside the continuation's searched scope."
            ),
        };
        second.investigation.limitations.push(format!(
            "The continuation returned {second_status:?}; the combined result remains Partial. Current findings are retained, and prior-pass context is background only, not a global no-findings conclusion."
        ));
    }

    second.stats = aggregate_pair(&first_stats, &second_stats, true);
    second
}

fn compact_previous_context(first: &ScoutResult) -> String {
    let mut previous = first.clone();
    // Do not feed the already-consumed continuation request back as if it
    // were still actionable context for a parent or another renderer.
    previous.investigation.continuation = None;
    previous.compact_text()
}

fn reports_identical(first: &ScoutResult, second: &ScoutResult) -> bool {
    first.summary == second.summary
        && first.citations.len() == second.citations.len()
        && first
            .citations
            .iter()
            .zip(&second.citations)
            .all(|(left, right)| {
                left.path == right.path
                    && left.start_line == right.start_line
                    && left.end_line == right.end_line
                    && left.reason == right.reason
            })
        && first.investigation.intent == second.investigation.intent
        && first.investigation.status == second.investigation.status
        && first.investigation.findings.len() == second.investigation.findings.len()
        && first
            .investigation
            .findings
            .iter()
            .zip(&second.investigation.findings)
            .all(|(left, right)| {
                left.question == right.question
                    && left.answer == right.answer
                    && left.citations.len() == right.citations.len()
                    && left
                        .citations
                        .iter()
                        .zip(&right.citations)
                        .all(|(left, right)| same_citation(left, right))
            })
        && first.investigation.unresolved == second.investigation.unresolved
        && first.investigation.searched_scope == second.investigation.searched_scope
        && first.investigation.limitations == second.investigation.limitations
        && first.investigation.confidence.level == second.investigation.confidence.level
        && first.investigation.confidence.basis == second.investigation.confidence.basis
}

fn same_citation(
    left: &repotracer_core::ValidatedCitation,
    right: &repotracer_core::ValidatedCitation,
) -> bool {
    left.path == right.path
        && left.start_line == right.start_line
        && left.end_line == right.end_line
        && left.reason == right.reason
}

fn aggregate_pair(first: &ScoutStats, second: &ScoutStats, provider_completed: bool) -> ScoutStats {
    let mut stats = first.clone();
    stats.turns = first.turns.saturating_add(second.turns);
    stats.tool_calls = first.tool_calls.saturating_add(second.tool_calls);
    stats.duration_ms = first.duration_ms.saturating_add(second.duration_ms);
    stats.warm_process = first.warm_process || second.warm_process;
    stats.thread_turn = first.thread_turn.max(second.thread_turn);
    stats.index_usage =
        aggregate_index_usage(first.index_usage.as_ref(), second.index_usage.as_ref());
    stats.reasoning_effort = first.reasoning_effort.clone();
    stats.attempts = vec![
        AdaptiveScout::attempt(
            first.reasoning_effort.as_deref().unwrap_or("unknown"),
            first,
            true,
        ),
        AdaptiveScout::attempt(
            second.reasoning_effort.as_deref().unwrap_or("unknown"),
            second,
            provider_completed,
        ),
    ];
    let usage = add_usage(&first.usage, &second.usage);
    let overflow = [
        (first.usage.input_tokens, second.usage.input_tokens),
        (
            first.usage.cached_input_tokens,
            second.usage.cached_input_tokens,
        ),
        (
            first.usage.cache_write_input_tokens,
            second.usage.cache_write_input_tokens,
        ),
        (first.usage.output_tokens, second.usage.output_tokens),
        (
            first.usage.reasoning_output_tokens,
            second.usage.reasoning_output_tokens,
        ),
        (first.usage.total_tokens, second.usage.total_tokens),
    ]
    .iter()
    .any(|(a, b)| a.zip(*b).is_some_and(|(a, b)| a.checked_add(b).is_none()));
    usage.apply_to(&mut stats);
    stats.usage_status = if first.usage_status == UsageStatus::Complete
        && second.usage_status == UsageStatus::Complete
        && !overflow
    {
        UsageStatus::Complete
    } else if first.usage_status == UsageStatus::Unknown
        && second.usage_status == UsageStatus::Unknown
    {
        UsageStatus::Unknown
    } else {
        UsageStatus::Partial
    };
    stats.reported_cost_usd = first
        .reported_cost_usd
        .zip(second.reported_cost_usd)
        .and_then(|(a, b)| (a >= 0.0 && b >= 0.0 && (a + b).is_finite()).then_some(a + b));
    stats
}

fn aggregate_pair_with_unknown(
    first: &ScoutStats,
    second: Option<&ScoutStats>,
    first_effort: &str,
    second_effort: &str,
) -> ScoutStats {
    let unknown = ScoutStats {
        reasoning_effort: Some(second_effort.to_string()),
        ..Default::default()
    };
    let second = second.unwrap_or(&unknown);
    let mut stats = aggregate_pair(first, second, false);
    if stats.attempts.len() >= 2 {
        stats.attempts[0].reasoning_effort = first_effort.to_string();
        stats.attempts[1].reasoning_effort = second_effort.to_string();
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use repotracer_core::{
        ConfidenceLevel, Finding, InvestigationConfidence, InvestigationIntent,
        InvestigationReport, ValidatedCitation,
    };
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use tokio::sync::Mutex;

    struct ScriptedBackend {
        responses: Mutex<VecDeque<Result<ScoutResult, anyhow::Error>>>,
        requests: Mutex<Vec<ScoutRequest>>,
    }

    impl ScriptedBackend {
        fn new(responses: Vec<Result<ScoutResult, anyhow::Error>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ScoutBackend for ScriptedBackend {
        async fn scout(&self, request: ScoutRequest) -> Result<ScoutResult, anyhow::Error> {
            self.requests.lock().await.push(request);
            self.responses
                .lock()
                .await
                .pop_front()
                .expect("scripted response")
        }
    }

    fn request() -> ScoutRequest {
        ScoutRequest {
            investigation: repotracer_core::InvestigationSpec {
                intent: InvestigationIntent::Diagnose,
                questions: vec!["Where is the conflicting precedence?".into()],
                known_context: "The parent traced both loaders.".into(),
                target_paths: vec!["src".into()],
                ..Default::default()
            },
            query: "Trace precedence".into(),
            root: PathBuf::from("."),
            focus: Some(PathBuf::from("src")),
            max_turns: None,
            timeout: None,
        }
    }

    fn result(
        status: InvestigationStatus,
        continuation: Option<ReasoningContinuation>,
    ) -> ScoutResult {
        let mut stats = ScoutStats {
            reasoning_effort: Some("medium".into()),
            usage_status: UsageStatus::Complete,
            usage: UsageStats {
                input_tokens: Some(10),
                cached_input_tokens: Some(1),
                cache_write_input_tokens: Some(1),
                output_tokens: Some(4),
                reasoning_output_tokens: Some(2),
                total_tokens: Some(14),
            },
            ..Default::default()
        };
        stats.usage.clone().apply_to(&mut stats);
        ScoutResult {
            investigation: InvestigationReport {
                intent: InvestigationIntent::Diagnose,
                status,
                findings: vec![Finding {
                    question: "precedence".into(),
                    answer: "The first loader wins.".into(),
                    citations: vec![ValidatedCitation {
                        path: "src/lib.rs".into(),
                        start_line: 1,
                        end_line: 1,
                        reason: Some("loader".into()),
                    }],
                }],
                unresolved: vec!["Which override wins?".into()],
                searched_scope: vec!["src".into()],
                limitations: vec![],
                confidence: InvestigationConfidence {
                    level: ConfidenceLevel::Medium,
                    basis: "Both loaders were traced; the final override is unclear.".into(),
                },
                continuation,
            },
            summary: "First answer".into(),
            citations: vec![ValidatedCitation {
                path: "src/lib.rs".into(),
                start_line: 1,
                end_line: 1,
                reason: Some("loader".into()),
            }],
            stats,
            raw_final: Some("first".into()),
        }
    }

    #[test]
    fn continuation_corrections_keep_background_source_separate() {
        let first = result(InvestigationStatus::Partial, None);
        let mut second = result(InvestigationStatus::Complete, None);
        second.summary = "The final override wins.".into();
        second.investigation.findings[0].answer = second.summary.clone();
        second.investigation.unresolved.clear();
        second.citations[0].start_line = 2;
        second.citations[0].end_line = 2;
        second.investigation.findings[0].citations = second.citations.clone();
        let merged = merge_results(first, second);
        assert_eq!(merged.investigation.status, InvestigationStatus::Complete);
        assert_eq!(merged.investigation.findings.len(), 1);
        assert_eq!(
            merged.investigation.findings[0].answer,
            "The final override wins."
        );
        assert_eq!(merged.citations.len(), 2);
        assert_eq!(merged.citations[0].start_line, 2);
        assert!(merged.citations[1]
            .reason
            .as_deref()
            .unwrap()
            .starts_with("Previous-pass background"));
        assert!(merged.summary.contains("The first loader wins."));
        assert_eq!(merged.stats.usage.total_tokens, Some(28));
    }

    #[test]
    fn partial_or_not_found_does_not_erase_prior_context() {
        for status in [InvestigationStatus::Partial, InvestigationStatus::NotFound] {
            let first = result(InvestigationStatus::Partial, None);
            let mut second = result(status, None);
            second.summary = "Additional scope remains unclear.".into();
            second.investigation.findings[0].answer = "The second path adds a caller.".into();
            let merged = merge_results(first, second);
            assert_eq!(merged.investigation.status, InvestigationStatus::Partial);
            assert_eq!(
                merged.investigation.findings[0].answer,
                "The second path adds a caller."
            );
            assert!(merged.summary.contains("The first loader wins."));
            assert_eq!(merged.stats.attempts.len(), 2);
        }
    }

    fn continuation(effort: &str) -> ReasoningContinuation {
        ReasoningContinuation {
            effort: effort.into(),
            question: "Which override wins?".into(),
            reason: "The traced branches conflict at the merge boundary.".into(),
        }
    }

    fn wrapper(backend: Arc<ScriptedBackend>, supported: Option<Vec<&str>>) -> AdaptiveScout {
        AdaptiveScout::new(
            backend,
            true,
            supported.map(|values| values.into_iter().map(str::to_owned).collect()),
            "medium",
        )
    }

    #[test]
    fn partial_usage_keeps_known_cache_dimensions_and_reported_cost() {
        let mut first = result(InvestigationStatus::Partial, None).stats;
        let mut second = first.clone();
        first.reported_cost_usd = Some(0.2);
        second.reported_cost_usd = Some(0.3);
        second.usage.reasoning_output_tokens = None;
        second.usage_status = UsageStatus::Partial;
        let total = aggregate_pair(&first, &second, false);
        assert_eq!(total.usage.input_tokens, Some(20));
        assert_eq!(total.usage.cached_input_tokens, Some(2));
        assert_eq!(total.usage.cache_write_input_tokens, Some(2));
        assert_eq!(total.usage.reasoning_output_tokens, None);
        assert_eq!(total.reported_cost_usd, Some(0.5));
        assert_eq!(total.usage_status, UsageStatus::Partial);
        assert_eq!(total.attempts[0].usage.reasoning_output_tokens, Some(2));
        assert!(!total.attempts[1].succeeded);
    }

    #[test]
    fn overflow_cannot_look_like_an_exact_total() {
        let mut stats = result(InvestigationStatus::Partial, None).stats;
        stats.usage.input_tokens = Some(u32::MAX);
        stats.reported_cost_usd = Some(f64::MAX);
        let total = aggregate_pair(&stats, &stats, true);
        assert_eq!(total.usage.input_tokens, None);
        assert_eq!(total.usage_status, UsageStatus::Partial);
        assert_eq!(total.reported_cost_usd, None);
        assert_eq!(total.attempts[0].usage.input_tokens, Some(u32::MAX));
    }

    #[tokio::test]
    async fn no_request_does_not_make_a_second_call() {
        let backend = Arc::new(ScriptedBackend::new(vec![Ok(result(
            InvestigationStatus::Complete,
            None,
        ))]));
        let output = wrapper(backend.clone(), Some(vec!["high", "max"]))
            .scout(request())
            .await
            .unwrap();
        assert_eq!(backend.requests.lock().await.len(), 1);
        assert!(output.stats.attempts.is_empty());
        assert_eq!(output.investigation.status, InvestigationStatus::Complete);
    }

    #[tokio::test]
    async fn complete_first_report_never_starts_a_continuation() {
        let backend = Arc::new(ScriptedBackend::new(vec![Ok(result(
            InvestigationStatus::Complete,
            Some(continuation("max")),
        ))]));
        let output = wrapper(backend.clone(), Some(vec!["max"]))
            .scout(request())
            .await
            .unwrap();
        assert_eq!(backend.requests.lock().await.len(), 1);
        assert!(output.investigation.continuation.is_none());
        assert_eq!(output.investigation.status, InvestigationStatus::Complete);
    }

    #[tokio::test]
    async fn zero_is_uncapped_but_positive_turn_budgets_do_not_double_spend() {
        let mut uncapped = request();
        uncapped.max_turns = Some(0);
        let backend = Arc::new(ScriptedBackend::new(vec![
            Ok(result(
                InvestigationStatus::Partial,
                Some(continuation("max")),
            )),
            Ok({
                let mut output = result(InvestigationStatus::Complete, None);
                output.investigation.unresolved.clear();
                output
            }),
        ]));
        let output = wrapper(backend.clone(), Some(vec!["max"]))
            .scout(uncapped)
            .await
            .unwrap();
        assert_eq!(backend.requests.lock().await.len(), 2);
        assert_eq!(output.investigation.status, InvestigationStatus::Complete);

        let backend = Arc::new(ScriptedBackend::new(vec![Ok(result(
            InvestigationStatus::Partial,
            Some(continuation("max")),
        ))]));
        let scout = wrapper(backend.clone(), Some(vec!["max"])).with_turn_ceiling(4);
        let output = scout.scout(request()).await.unwrap();
        assert_eq!(backend.requests.lock().await.len(), 1);
        assert_eq!(output.investigation.status, InvestigationStatus::Partial);
    }

    #[tokio::test]
    async fn opt_out_leaves_the_provider_result_unchanged() {
        let backend = Arc::new(ScriptedBackend::new(vec![Ok(result(
            InvestigationStatus::Partial,
            Some(continuation("max")),
        ))]));
        let scout = AdaptiveScout::new(backend.clone(), false, Some(vec!["max".into()]), "medium");
        let output = scout.scout(request()).await.unwrap();
        assert_eq!(backend.requests.lock().await.len(), 1);
        assert!(output.investigation.continuation.is_some());
        assert!(output.stats.attempts.is_empty());
    }

    #[tokio::test]
    async fn same_lower_unsupported_and_invalid_efforts_are_rejected() {
        for effort in ["medium", "low", "xhigh", "invalid"] {
            let backend = Arc::new(ScriptedBackend::new(vec![Ok(result(
                InvestigationStatus::Partial,
                Some(continuation(effort)),
            ))]));
            let output = wrapper(backend.clone(), Some(vec!["high", "max"]))
                .scout(request())
                .await
                .unwrap();
            assert_eq!(backend.requests.lock().await.len(), 1, "{effort}");
            assert_eq!(output.investigation.status, InvestigationStatus::Partial);
            assert!(output
                .investigation
                .limitations
                .iter()
                .any(|limitation| limitation.contains("did not resolve the gap")));
        }
    }

    #[tokio::test]
    async fn request_effort_is_used_when_provider_stats_omit_it() {
        let mut first = result(InvestigationStatus::Partial, Some(continuation("max")));
        first.stats.reasoning_effort = None;
        let mut original = request();
        original.investigation.reasoning_effort = Some("high".into());
        let mut complete = result(InvestigationStatus::Complete, None);
        complete.investigation.unresolved.clear();
        let backend = Arc::new(ScriptedBackend::new(vec![Ok(first), Ok(complete)]));
        let output = wrapper(backend.clone(), Some(vec!["max"]))
            .scout(original)
            .await
            .unwrap();
        assert_eq!(backend.requests.lock().await.len(), 2);
        assert_eq!(output.stats.attempts[0].reasoning_effort, "high");
        assert_eq!(output.stats.attempts[1].reasoning_effort, "max");
    }

    #[tokio::test]
    async fn max_supported_runs_one_continuation_with_original_context() {
        let mut complete = result(InvestigationStatus::Complete, None);
        complete.investigation.unresolved.clear();
        let backend = Arc::new(ScriptedBackend::new(vec![
            Ok(result(
                InvestigationStatus::Partial,
                Some(continuation("max")),
            )),
            Ok(complete),
        ]));
        let output = wrapper(backend.clone(), Some(vec!["high", "max"]))
            .scout(request())
            .await
            .unwrap();
        let requests = backend.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].query, requests[1].query);
        assert_eq!(requests[0].focus, requests[1].focus);
        assert_eq!(
            requests[0].investigation.questions,
            requests[1].investigation.questions
        );
        assert_eq!(
            requests[0].investigation.known_context,
            requests[1].investigation.known_context
        );
        assert_eq!(
            requests[0].investigation.target_paths,
            requests[1].investigation.target_paths
        );
        assert_eq!(
            requests[0].investigation.conversation_id,
            requests[1].investigation.conversation_id
        );
        assert_eq!(
            requests[1].investigation.reasoning_effort.as_deref(),
            Some("max")
        );
        assert!(requests[1]
            .investigation
            .continuation_context
            .as_deref()
            .unwrap()
            .contains("Which override wins?"));
        assert_eq!(output.stats.attempts.len(), 2);
        assert_eq!(output.investigation.status, InvestigationStatus::Complete);
    }

    #[tokio::test]
    async fn failed_second_without_usage_keeps_partial_and_unknown_attempt() {
        let backend = Arc::new(ScriptedBackend::new(vec![
            Ok(result(
                InvestigationStatus::Partial,
                Some(continuation("max")),
            )),
            Err(anyhow::anyhow!("provider stopped")),
        ]));
        let output = wrapper(backend.clone(), Some(vec!["high", "max"]))
            .scout(request())
            .await
            .unwrap();
        assert_eq!(backend.requests.lock().await.len(), 2);
        assert_eq!(output.investigation.status, InvestigationStatus::Partial);
        assert_eq!(output.stats.usage_status, UsageStatus::Partial);
        assert_eq!(output.stats.attempts.len(), 2);
        assert!(!output.stats.attempts[1].succeeded);
        assert_eq!(output.stats.attempts[1].usage_status, UsageStatus::Unknown);
        assert!(output.stats.prompt_tokens.is_none());
        assert!(output
            .investigation
            .limitations
            .iter()
            .any(|limitation| limitation.contains("failed")));
    }

    #[tokio::test]
    async fn second_report_with_an_unresolved_question_remains_partial() {
        let mut second = result(InvestigationStatus::Complete, None);
        second.citations.clear();
        second.investigation.findings.clear();
        second
            .investigation
            .unresolved
            .push("The merge boundary".into());
        let backend = Arc::new(ScriptedBackend::new(vec![
            Ok(result(
                InvestigationStatus::Partial,
                Some(continuation("max")),
            )),
            Ok(second),
        ]));
        let output = wrapper(backend, Some(vec!["high", "max"]))
            .scout(request())
            .await
            .unwrap();
        assert_eq!(output.investigation.status, InvestigationStatus::Partial);
        assert!(output
            .investigation
            .limitations
            .iter()
            .any(|limitation| limitation.contains("material question unresolved")));
        assert!(output.investigation.findings.is_empty());
        assert!(!output.citations.is_empty());
        assert!(output.citations.iter().all(|citation| citation
            .reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("Previous-pass background"))));
        assert!(output
            .summary
            .contains("Previous-pass context, not revalidated by the continuation"));
        assert_eq!(output.stats.attempts.len(), 2);
    }

    #[tokio::test]
    async fn complete_experiment_continuation_does_not_require_source_citations() {
        let mut second = result(InvestigationStatus::Complete, None);
        second.summary =
            "Ran a focused reproduction with an empty field; the parser returned an empty string."
                .into();
        second.citations.clear();
        second.investigation.findings.clear();
        second.investigation.unresolved.clear();
        let backend = Arc::new(ScriptedBackend::new(vec![
            Ok(result(
                InvestigationStatus::Partial,
                Some(continuation("max")),
            )),
            Ok(second),
        ]));
        let output = wrapper(backend, Some(vec!["high", "max"]))
            .scout(request())
            .await
            .unwrap();
        assert_eq!(output.investigation.status, InvestigationStatus::Complete);
        assert!(output.summary.contains("focused reproduction"));
        assert!(!output
            .investigation
            .limitations
            .iter()
            .any(|line| line.contains("returned no answer")));
    }

    #[tokio::test]
    async fn second_continuation_request_is_not_recursed() {
        let mut complete = result(InvestigationStatus::Complete, Some(continuation("max")));
        complete.investigation.unresolved.clear();
        let backend = Arc::new(ScriptedBackend::new(vec![
            Ok(result(
                InvestigationStatus::Partial,
                Some(continuation("max")),
            )),
            Ok(complete),
        ]));
        let output = wrapper(backend.clone(), Some(vec!["max"]))
            .scout(request())
            .await
            .unwrap();
        assert_eq!(backend.requests.lock().await.len(), 2);
        assert_eq!(output.investigation.status, InvestigationStatus::Partial);
        assert!(output.investigation.continuation.is_none());
        assert!(output
            .investigation
            .limitations
            .iter()
            .any(|limitation| limitation.contains("Only one adaptive continuation")));
    }
}
