use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<FunctionCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_tools(content: Option<String>, tool_calls: Vec<FunctionCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub message: ChatMessage,
    pub model: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout_ms: u64,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "hf.co/mitkox/FastContext-1.0-4B-RL-Q4_K_M-GGUF:latest".into(),
            api_key: Some("ollama".into()),
            timeout_ms: 120_000,
            temperature: 0.0,
            max_tokens: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("model request failed: {0}")]
    Request(String),
    #[error("model response invalid: {0}")]
    InvalidResponse(String),
    #[error("timeout")]
    Timeout,
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("mock script exhausted")]
    ScriptExhausted,
}

#[async_trait]
pub trait ModelBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;
}
