//! Grephound scout engine — transport-agnostic repository exploration.

mod config;
mod types;

pub use config::{ExplorerBudget, GrephoundConfig, ModelSettings};
pub use types::{ExplorerTurn, ScoutRequest, ScoutResult, ScoutStats, ValidatedCitation};
