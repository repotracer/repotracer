use crate::types::{
    ChatMessage, FunctionCall, MessageRole, ModelBackend, ModelConfig, ModelError, ModelRequest,
    ModelResponse, ToolSpec, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

pub struct OpenAiCompatBackend {
    client: reqwest::Client,
    config: ModelConfig,
}

impl OpenAiCompatBackend {
    pub fn new(config: ModelConfig) -> Result<Self, ModelError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| ModelError::Request(e.to_string()))?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &ModelConfig {
        &self.config
    }
}

#[async_trait]
impl ModelBackend for OpenAiCompatBackend {
    fn name(&self) -> &str {
        &self.config.model
    }

    fn temperature(&self) -> f32 {
        self.config.temperature
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let tools: Vec<Value> = request.tools.iter().map(openai_tool).collect();

        let body = json!({
            "model": self.config.model,
            "messages": request.messages.iter().map(to_openai_message).collect::<Vec<_>>(),
            "temperature": request.temperature,
            "tools": tools,
            "tool_choice": "auto",
        });

        let mut body = body;
        if let Some(max) = request.max_tokens.or(self.config.max_tokens) {
            body.as_object_mut()
                .unwrap()
                .insert("max_tokens".into(), json!(max));
        }

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.config.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                ModelError::Timeout
            } else {
                ModelError::Request(format!(
                    "Could not reach GPT endpoint at {}",
                    self.config.base_url
                ))
            }
        })?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ModelError::Request(e.to_string()))?;
        if !status.is_success() {
            return Err(ModelError::Request(format!(
                "model HTTP {status}: {}",
                truncate(&text, 800)
            )));
        }

        let parsed: OpenAiChatResponse = serde_json::from_str(&text)
            .map_err(|e| ModelError::InvalidResponse(format!("{e}: {}", truncate(&text, 400))))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ModelError::InvalidResponse("no choices".into()))?;

        let msg = from_openai_message(choice.message)?;
        Ok(ModelResponse {
            message: msg,
            model: parsed.model.unwrap_or_else(|| self.config.model.clone()),
            usage: parsed.usage.map(|u| Usage {
                prompt_tokens: u.prompt_tokens.unwrap_or(0),
                completion_tokens: u.completion_tokens.unwrap_or(0),
                total_tokens: u.total_tokens.unwrap_or(0),
            }),
        })
    }
}

fn openai_tool(t: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": t.name,
            "description": t.description,
            "parameters": t.parameters,
        }
    })
}

fn to_openai_message(m: &ChatMessage) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "role".into(),
        json!(match m.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }),
    );
    if let Some(c) = &m.content {
        obj.insert("content".into(), json!(c));
    } else if m.role == MessageRole::Assistant {
        obj.insert("content".into(), Value::Null);
    }
    if let Some(calls) = &m.tool_calls {
        let arr: Vec<Value> = calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": {
                        "name": c.name,
                        "arguments": c.arguments,
                    }
                })
            })
            .collect();
        obj.insert("tool_calls".into(), Value::Array(arr));
    }
    if let Some(id) = &m.tool_call_id {
        obj.insert("tool_call_id".into(), json!(id));
    }
    if let Some(name) = &m.name {
        obj.insert("name".into(), json!(name));
    }
    Value::Object(obj)
}

fn from_openai_message(m: OpenAiMessage) -> Result<ChatMessage, ModelError> {
    let tool_calls = m.tool_calls.filter(|calls| !calls.is_empty()).map(|calls| {
        calls
            .into_iter()
            .map(|c| FunctionCall {
                id: c.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                name: c.function.name,
                arguments: c.function.arguments,
            })
            .collect()
    });

    Ok(ChatMessage {
        role: MessageRole::Assistant,
        content: m.content.filter(|content| !content.is_empty()),
        tool_calls,
        tool_call_id: None,
        name: None,
    })
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    model: Option<String>,
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: Option<String>,
    function: OpenAiFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncates_at_utf8_boundary() {
        assert_eq!(truncate("1234567é", 8), "1234567…");
    }
}
