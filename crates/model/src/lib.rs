//! OpenAI-compatible model backends + deterministic mock explorer.

mod mock;
mod openai;
mod types;

pub use mock::{MockModel, MockScript, MockStep};
pub use openai::OpenAiCompatBackend;
pub use types::{
    ChatMessage, FunctionCall, MessageRole, ModelBackend, ModelConfig, ModelError, ModelRequest,
    ModelResponse, ToolSpec,
};
