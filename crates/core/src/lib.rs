//! Grephound scout engine — transport-agnostic repository exploration.

mod citations;
mod config;
mod prompt;
mod types;

pub use citations::{parse_citations, validate_citation, validate_citations, Citation};
pub use config::{ExplorerBudget, GrephoundConfig, ModelSettings};
pub use prompt::build_system_prompt;
pub use types::{ExplorerTurn, ScoutRequest, ScoutResult, ScoutStats, ValidatedCitation};
