//! RepoTracer scout engine — transport-agnostic repository exploration.

mod citations;
mod config;
mod engine;
mod investigation;
mod prompt;
mod types;

pub use citations::{parse_citations, validate_citation, validate_citations, Citation};
pub use config::{ExplorerBudget, ModelSettings, RepoTracerConfig, SessionSettings};
pub use engine::ScoutEngine;
pub use investigation::{
    assess_output, investigation_output_schema, investigation_prompt, questions, validate_request,
    ConfidenceLevel, Finding, InvestigationConfidence, InvestigationIntent, InvestigationReport,
    InvestigationSpec, InvestigationStatus, ReasoningContinuation,
};
pub use prompt::{build_system_prompt, workspace_facts};
pub use types::{
    ConversationInfo, ExplorerTurn, IndexUsage, ScoutAttemptStats, ScoutBackend, ScoutBackendError,
    ScoutRequest, ScoutResult, ScoutStats, UsageStats, UsageStatus, ValidatedCitation,
};
