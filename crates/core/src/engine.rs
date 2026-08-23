use crate::citations::{parse_citations, validate_citations};
use crate::config::ExplorerBudget;
use crate::prompt::{build_system_prompt, user_query_prompt};
use crate::types::{ScoutRequest, ScoutResult, ScoutStats};
use repotracer_model::{ChatMessage, ModelBackend, ModelRequest, ToolSpec};
use repotracer_repo_tools::{RepoTools, ToolCall};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, warn};

pub struct ScoutEngine {
    model: Arc<dyn ModelBackend>,
    tools: RepoTools,
    budget: ExplorerBudget,
}

impl ScoutEngine {
    pub fn new(model: Arc<dyn ModelBackend>, tools: RepoTools, budget: ExplorerBudget) -> Self {
        let tools = tools
            .with_concurrency(budget.concurrency)
            .with_timeout(budget.tool_timeout());
        Self {
            model,
            tools,
            budget,
        }
    }

    pub fn tools(&self) -> &RepoTools {
        &self.tools
    }

    pub async fn scout(&self, request: ScoutRequest) -> anyhow::Result<ScoutResult> {
        let started = Instant::now();
        let max_turns = request.max_turns.unwrap_or(self.budget.max_turns);
        let total_timeout = request
            .timeout
            .unwrap_or_else(|| self.budget.total_timeout());

        let run = self.scout_inner(request, max_turns);
        match tokio::time::timeout(total_timeout, run).await {
            Ok(res) => {
                let mut r = res?;
                r.stats.duration_ms = started.elapsed().as_millis() as u64;
                Ok(r)
            }
            Err(_) => Ok(ScoutResult {
                summary: format!("Scout timed out after {}s.", total_timeout.as_secs()),
                citations: vec![],
                stats: ScoutStats {
                    turns: 0,
                    tool_calls: 0,
                    duration_ms: started.elapsed().as_millis() as u64,
                    model: self.model.name().to_string(),
                    prompt_tokens: None,
                    completion_tokens: None,
                },
                raw_final: None,
            }),
        }
    }

    async fn scout_inner(
        &self,
        request: ScoutRequest,
        max_turns: u32,
    ) -> anyhow::Result<ScoutResult> {
        let root = request.root.clone();
        let system = build_system_prompt(&root);
        let mut messages = vec![
            ChatMessage::system(system),
            ChatMessage::user(user_query_prompt(&request.query)),
        ];

        let tool_specs: Vec<ToolSpec> = self
            .tools
            .definitions()
            .into_iter()
            .map(|d| ToolSpec {
                name: d.name,
                description: d.description,
                parameters: d.parameters,
            })
            .collect();

        let mut turns: u32 = 0;
        let mut tool_calls_total: u32 = 0;
        let mut prompt_tokens: u32 = 0;
        let mut completion_tokens: u32 = 0;
        let mut correction_used = false;

        loop {
            turns += 1;
            if turns > max_turns + 1 {
                return Ok(empty_result(
                    &format!("No final answer after {max_turns} turns."),
                    turns - 1,
                    tool_calls_total,
                    self.model.name(),
                    prompt_tokens,
                    completion_tokens,
                ));
            }

            if turns == max_turns + 1 {
                messages.push(ChatMessage::user(
                    "Max number of turns reached. Please provide the final answer based on the information you have gathered.",
                ));
            }

            let response = self
                .model
                .complete(ModelRequest {
                    messages: messages.clone(),
                    tools: tool_specs.clone(),
                    temperature: 0.0,
                    max_tokens: None,
                })
                .await?;

            if let Some(u) = &response.usage {
                prompt_tokens = prompt_tokens.saturating_add(u.prompt_tokens);
                completion_tokens = completion_tokens.saturating_add(u.completion_tokens);
            }

            let msg = response.message;
            messages.push(msg.clone());

            if let Some(calls) = &msg.tool_calls {
                if !calls.is_empty() {
                    if tool_calls_total as usize + calls.len() > self.budget.max_tool_calls as usize
                    {
                        warn!("max tool calls reached");
                        messages.push(ChatMessage::user(
                            "Tool call budget exhausted. Provide your final_answer now.",
                        ));
                        continue;
                    }

                    let tool_calls: Vec<ToolCall> = calls
                        .iter()
                        .map(|c| ToolCall {
                            id: c.id.clone(),
                            name: c.name.clone(),
                            arguments: c.arguments.clone(),
                        })
                        .collect();

                    debug!(count = tool_calls.len(), "executing tools concurrently");
                    let results = self.tools.call_many(&tool_calls).await;
                    tool_calls_total += results.len() as u32;

                    for r in results {
                        messages.push(ChatMessage::tool(r.tool_call_id, r.output));
                    }
                    continue;
                }
            }

            // Final assistant message (no tool calls).
            let content = msg.content.clone().unwrap_or_default();
            let (summary, raw_citations) = parse_citations(&content);
            let validated = validate_citations(&root, &raw_citations);

            // One correction turn if all citations invalid but model claimed some.
            if validated.is_empty() && !raw_citations.is_empty() && !correction_used {
                correction_used = true;
                messages.push(ChatMessage::user(
                    "Some citations were invalid (missing file or bad line range). Please correct paths/lines using tools if needed, then provide an updated <final_answer>.",
                ));
                continue;
            }

            // If no final_answer tag and no citations, still return summary.
            if validated.is_empty()
                && raw_citations.is_empty()
                && content.contains("<final_answer>")
            {
                // empty final answer
            }

            return Ok(ScoutResult {
                summary,
                citations: validated,
                stats: ScoutStats {
                    turns,
                    tool_calls: tool_calls_total,
                    duration_ms: 0,
                    model: self.model.name().to_string(),
                    prompt_tokens: if prompt_tokens > 0 {
                        Some(prompt_tokens)
                    } else {
                        None
                    },
                    completion_tokens: if completion_tokens > 0 {
                        Some(completion_tokens)
                    } else {
                        None
                    },
                },
                raw_final: Some(content),
            });
        }
    }
}

#[async_trait::async_trait]
impl crate::types::ScoutBackend for ScoutEngine {
    async fn scout(&self, request: ScoutRequest) -> anyhow::Result<ScoutResult> {
        ScoutEngine::scout(self, request).await
    }
}

fn empty_result(
    summary: &str,
    turns: u32,
    tool_calls: u32,
    model: &str,
    prompt_tokens: u32,
    completion_tokens: u32,
) -> ScoutResult {
    ScoutResult {
        summary: summary.into(),
        citations: vec![],
        stats: ScoutStats {
            turns,
            tool_calls,
            duration_ms: 0,
            model: model.into(),
            prompt_tokens: if prompt_tokens > 0 {
                Some(prompt_tokens)
            } else {
                None
            },
            completion_tokens: if completion_tokens > 0 {
                Some(completion_tokens)
            } else {
                None
            },
        },
        raw_final: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repotracer_model::{MockModel, MockScript, MockStep};
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn mock_end_to_end() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/auth")).unwrap();
        fs::write(
            dir.path().join("src/auth/session.rs"),
            (1..=100)
                .map(|i| format!("// line {i} session\n"))
                .collect::<String>(),
        )
        .unwrap();

        let model = Arc::new(MockModel::new(MockScript {
            steps: vec![
                MockStep::Tools(vec![
                    (
                        "Glob".into(),
                        r#"{"pattern":"**/*auth*"}"#.into(),
                    ),
                    (
                        "Grep".into(),
                        r#"{"pattern":"session","output_mode":"files_with_matches"}"#.into(),
                    ),
                ]),
                MockStep::Final(
                    "Session handling lives here.\n\n<final_answer>\nsrc/auth/session.rs:10-40 (session module)\n</final_answer>"
                        .into(),
                ),
            ],
        }));

        let tools = RepoTools::new(dir.path());
        let engine = ScoutEngine::new(model, tools, ExplorerBudget::default());
        let result = engine
            .scout(ScoutRequest {
                query: "where is session handled?".into(),
                root: dir.path().to_path_buf(),
                focus: None,
                max_turns: Some(4),
                timeout: None,
            })
            .await
            .unwrap();

        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].path, "src/auth/session.rs");
        assert!(result.stats.tool_calls >= 2);
        assert!(result.stats.turns >= 2);
    }
}
