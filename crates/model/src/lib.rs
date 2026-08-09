//! Model backends for grephound.

mod types;

pub use types::{
    ChatMessage, FunctionCall, MessageRole, ModelBackend, ModelConfig, ModelError, ModelRequest,
    ModelResponse, ToolSpec,
};
