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

/// Require encrypted transport when a custom endpoint receives an API key.
/// HTTP remains available for unauthenticated local model servers.
pub fn validate_api_endpoint(base_url: &str, api_key: Option<&str>) -> Result<(), ModelError> {
    if api_key.is_some_and(|key| !key.trim().is_empty()) {
        let url = reqwest::Url::parse(base_url)
            .map_err(|_| ModelError::Request("invalid model endpoint URL".into()))?;
        if url.scheme() != "https" {
            return Err(ModelError::Request(
                "an API key requires an HTTPS endpoint".into(),
            ));
        }
    }
    Ok(())
}

impl OpenAiCompatBackend {
    pub fn new(config: ModelConfig) -> Result<Self, ModelError> {
        validate_api_endpoint(&config.base_url, config.api_key.as_deref())?;
        let mut builder = reqwest::Client::builder().https_only(
            config
                .api_key
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty()),
        );
        if config.timeout_ms > 0 {
            builder = builder.timeout(Duration::from_millis(config.timeout_ms));
        }
        let client = builder
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
        if let Some(effort) = self
            .config
            .reasoning_effort
            .as_deref()
            .filter(|effort| !effort.trim().is_empty())
        {
            body["reasoning_effort"] = json!(effort);
            // GPT reasoning models do not consistently accept temperature overrides.
            body.as_object_mut().unwrap().remove("temperature");
        }
        if request.tools.is_empty() {
            body.as_object_mut().unwrap().remove("tools");
            body.as_object_mut().unwrap().remove("tool_choice");
        }
        if let Some(max) = request.max_tokens.or(self.config.max_tokens) {
            let field = if body.get("reasoning_effort").is_some() {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            body.as_object_mut()
                .unwrap()
                .insert(field.into(), json!(max));
        }

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = self
            .config
            .api_key
            .as_deref()
            .filter(|key| !key.trim().is_empty())
        {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                ModelError::Timeout
            } else {
                ModelError::Request(format!(
                    "Could not reach model endpoint at {}",
                    safe_endpoint(&self.config.base_url)
                ))
            }
        })?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ModelError::Request(e.to_string()))?;
        if !status.is_success() {
            return Err(ModelError::Request(format!("model HTTP {status}")));
        }

        let parsed: OpenAiChatResponse = serde_json::from_str(&text)
            .map_err(|e| ModelError::InvalidResponse(format!("invalid model response: {e}")))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ModelError::InvalidResponse("no choices".into()))?;

        let msg = from_openai_message(choice.message)?;
        Ok(ModelResponse {
            message: msg,
            model: parsed.model.unwrap_or_else(|| self.config.model.clone()),
            usage: parsed.usage.map(Into::into),
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

fn safe_endpoint(value: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(value) else {
        return "configured endpoint".into();
    };
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
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
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiInputDetails>,
    #[serde(default)]
    completion_tokens_details: Option<OpenAiOutputDetails>,
    #[serde(default)]
    input_tokens_details: Option<OpenAiInputDetails>,
    #[serde(default)]
    output_tokens_details: Option<OpenAiOutputDetails>,
    /// Some OpenAI-compatible gateways use the Responses API names while
    /// still exposing the Chat Completions response envelope.
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
    #[serde(default)]
    cache_write_input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cached_input_tokens: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OpenAiInputDetails {
    #[serde(default, alias = "cachedInputTokens")]
    cached_tokens: Option<u32>,
    #[serde(default)]
    cache_write_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OpenAiOutputDetails {
    #[serde(default, alias = "reasoningTokens")]
    reasoning_tokens: Option<u32>,
}

impl From<OpenAiUsage> for Usage {
    fn from(value: OpenAiUsage) -> Self {
        let input = value.prompt_tokens.or(value.input_tokens);
        let output = value.completion_tokens.or(value.output_tokens);
        let input_details = value.prompt_tokens_details.or(value.input_tokens_details);
        let cached = input_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
            .or(value.cached_input_tokens);
        let cache_write = value
            .cache_write_input_tokens
            .or(value.cache_creation_input_tokens)
            .or_else(|| input_details.as_ref().and_then(|d| d.cache_write_tokens))
            .or_else(|| {
                input_details
                    .as_ref()
                    .and_then(|d| d.cache_creation_input_tokens)
            });
        let reasoning = value
            .completion_tokens_details
            .or(value.output_tokens_details)
            .and_then(|d| d.reasoning_tokens);
        Usage {
            prompt_tokens: input,
            completion_tokens: output,
            total_tokens: value.total_tokens,
            cached_prompt_tokens: cached,
            cache_write_prompt_tokens: cache_write,
            reasoning_output_tokens: reasoning,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OpenAiUsage;
    use crate::types::Usage;
    use serde_json::json;

    #[test]
    fn parses_cache_and_reasoning_subsets_without_filling_missing_fields() {
        let parsed: Usage = serde_json::from_value::<OpenAiUsage>(json!({
            "prompt_tokens": 100,
            "prompt_tokens_details": {"cached_tokens": 40},
            "completion_tokens": 20,
            "completion_tokens_details": {"reasoning_tokens": 5},
            "total_tokens": 120
        }))
        .unwrap()
        .into();
        assert_eq!(parsed.prompt_tokens, Some(100));
        assert_eq!(parsed.cached_prompt_tokens, Some(40));
        assert_eq!(parsed.completion_tokens, Some(20));
        assert_eq!(parsed.reasoning_output_tokens, Some(5));
        assert_eq!(parsed.total_tokens, Some(120));
        assert_eq!(parsed.cache_write_prompt_tokens, None);
    }

    #[test]
    fn omitted_usage_dimensions_remain_unknown() {
        let parsed: Usage = serde_json::from_value::<OpenAiUsage>(json!({
            "prompt_tokens": 100
        }))
        .unwrap()
        .into();
        assert_eq!(parsed.prompt_tokens, Some(100));
        assert_eq!(parsed.completion_tokens, None);
        assert_eq!(parsed.total_tokens, None);
        assert_eq!(parsed.cached_prompt_tokens, None);
    }

    #[test]
    fn authenticated_completions_require_https_but_local_http_still_works() {
        for (base_url, api_key, accepted) in [
            ("http://127.0.0.1:8080/v1", None, true),
            ("http://127.0.0.1:8080/v1", Some(""), true),
            ("http://example.com/v1", Some("test-key"), false),
            ("https://example.com/v1", Some("test-key"), true),
            ("HTTPS://example.com/v1", Some("test-key"), true),
        ] {
            let config = super::ModelConfig {
                base_url: base_url.into(),
                api_key: api_key.map(str::to_owned),
                ..Default::default()
            };
            assert_eq!(
                super::OpenAiCompatBackend::new(config).is_ok(),
                accepted,
                "{base_url}"
            );
        }
    }

    #[test]
    fn parses_responses_api_usage_names_too() {
        let parsed: Usage = serde_json::from_value::<OpenAiUsage>(json!({
            "input_tokens": 100,
            "input_tokens_details": {"cached_tokens": 40},
            "output_tokens": 20,
            "output_tokens_details": {"reasoning_tokens": 5},
            "total_tokens": 120
        }))
        .unwrap()
        .into();
        assert_eq!(parsed.prompt_tokens, Some(100));
        assert_eq!(parsed.cached_prompt_tokens, Some(40));
        assert_eq!(parsed.completion_tokens, Some(20));
        assert_eq!(parsed.reasoning_output_tokens, Some(5));
        assert_eq!(parsed.total_tokens, Some(120));
    }

    #[test]
    fn endpoint_redaction_removes_query_credentials() {
        assert_eq!(
            super::safe_endpoint("https://gateway.example/v1?api_key=secret#fragment"),
            "https://gateway.example/v1"
        );
        assert_eq!(
            super::safe_endpoint("https://user:secret@gateway.example/v1"),
            "https://gateway.example/v1"
        );
        assert_eq!(super::safe_endpoint("not-a-url"), "configured endpoint");
    }
}
