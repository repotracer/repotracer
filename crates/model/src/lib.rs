//! Model backends for grephound.

mod openai;
mod types;

pub use openai::OpenAiCompatBackend;
pub use types::{
    ChatMessage, FunctionCall, MessageRole, ModelBackend, ModelConfig, ModelError, ModelRequest,
    ModelResponse, ToolSpec,
};
