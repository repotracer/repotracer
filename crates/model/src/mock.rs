use crate::types::{
    ChatMessage, FunctionCall, ModelBackend, ModelError, ModelRequest, ModelResponse,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// One scripted model turn.
#[derive(Debug, Clone)]
pub enum MockStep {
    /// Assistant emits tool calls (name, json args).
    Tools(Vec<(String, String)>),
    /// Assistant final text (include `<final_answer>` for citations).
    Final(String),
}

#[derive(Debug, Clone)]
pub struct MockScript {
    pub steps: Vec<MockStep>,
}

/// Deterministic explorer for CI — no GPU, no network.
pub struct MockModel {
    script: MockScript,
    idx: Arc<AtomicUsize>,
    name: String,
}

impl MockModel {
    pub fn new(script: MockScript) -> Self {
        Self {
            script,
            idx: Arc::new(AtomicUsize::new(0)),
            name: "mock".into(),
        }
    }

    /// Simple two-step: Grep then final citation.
    pub fn grep_then_cite(path: &str, start: u32, end: u32, reason: &str) -> Self {
        Self::new(MockScript {
            steps: vec![
                MockStep::Tools(vec![(
                    "Grep".into(),
                    r#"{"pattern":"auth","output_mode":"files_with_matches"}"#.into(),
                )]),
                MockStep::Final(format!(
                    "Found the implementation.\n\n<final_answer>\n{path}:{start}-{end} ({reason})\n</final_answer>"
                )),
            ],
        })
    }
}

#[async_trait]
impl ModelBackend for MockModel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let i = self.idx.fetch_add(1, Ordering::SeqCst);
        let step = self
            .script
            .steps
            .get(i)
            .ok_or(ModelError::ScriptExhausted)?;

        let message = match step {
            MockStep::Tools(calls) => {
                let tool_calls = calls
                    .iter()
                    .enumerate()
                    .map(|(j, (name, args))| FunctionCall {
                        id: format!("call_{i}_{j}"),
                        name: name.clone(),
                        arguments: args.clone(),
                    })
                    .collect();
                ChatMessage::assistant_tools(None, tool_calls)
            }
            MockStep::Final(text) => ChatMessage::assistant(text.clone()),
        };

        Ok(ModelResponse {
            message,
            model: self.name.clone(),
            usage: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn script_advances() {
        let m = MockModel::new(MockScript {
            steps: vec![
                MockStep::Tools(vec![("Read".into(), r#"{"path":"a"}"#.into())]),
                MockStep::Final("done".into()),
            ],
        });
        let r1 = m
            .complete(ModelRequest {
                messages: vec![],
                tools: vec![],
                temperature: 0.0,
                max_tokens: None,
            })
            .await
            .unwrap();
        assert!(r1.message.tool_calls.as_ref().unwrap().len() == 1);
        let r2 = m
            .complete(ModelRequest {
                messages: vec![],
                tools: vec![],
                temperature: 0.0,
                max_tokens: None,
            })
            .await
            .unwrap();
        assert_eq!(r2.message.content.as_deref(), Some("done"));
    }
}
