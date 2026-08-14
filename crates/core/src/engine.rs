use crate::citations::{parse_citations, validate_citations};
use crate::config::ExplorerBudget;
use crate::prompt::{build_system_prompt, user_query_prompt};
use crate::types::{ScoutRequest, ScoutResult, ScoutStats};
use grephound_model::{ChatMessage, ModelBackend, ModelRequest, ToolSpec};
use grephound_repo_tools::{resolve_in_root, RepoTools, ToolCall};
use std::collections::{HashMap, HashSet};
use std::path::Path;
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
                    cached_prompt_tokens: None,
                    completion_tokens: None,
                    reasoning_output_tokens: None,
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
        let mut seen_tool_calls = HashSet::new();

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

            let final_turn = turns == max_turns + 1;
            if final_turn {
                messages.push(ChatMessage::user(
                    "Max number of turns reached. Return the final answer now from gathered evidence, with no more tool calls.",
                ));
            }

            let response = self
                .model
                .complete(ModelRequest {
                    messages: messages.clone(),
                    tools: if final_turn {
                        Vec::new()
                    } else {
                        tool_specs.clone()
                    },
                    temperature: self.model.temperature(),
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
                            arguments: sandbox_search_arguments(
                                &c.name,
                                &c.arguments,
                                self.tools.root(),
                            ),
                        })
                        .collect();
                    let mut fresh_calls = Vec::new();
                    let mut duplicate_ids = HashSet::new();
                    for call in &tool_calls {
                        debug!(name = %call.name, arguments = %call.arguments, "model tool call");
                        if seen_tool_calls.insert((call.name.clone(), call.arguments.clone())) {
                            fresh_calls.push(call.clone());
                        } else {
                            duplicate_ids.insert(call.id.clone());
                        }
                    }

                    debug!(count = fresh_calls.len(), "executing tools concurrently");
                    let results = self.tools.call_many(&fresh_calls).await;
                    tool_calls_total += results.len() as u32;
                    let mut results: HashMap<_, _> = results
                        .into_iter()
                        .map(|result| (result.tool_call_id, result.output))
                        .collect();

                    for call in tool_calls {
                        let output = if duplicate_ids.contains(&call.id) {
                            "<system-reminder>This exact tool call already ran. Use the prior result, narrow the search, or provide the final answer.</system-reminder>".into()
                        } else {
                            results.remove(&call.id).unwrap_or_default()
                        };
                        messages.push(ChatMessage::tool(call.id, output));
                    }
                    continue;
                }
            }

            // Final assistant message (no tool calls).
            let content = msg.content.clone().unwrap_or_default();
            let (summary, raw_citations) = parse_citations(&content);
            let validated = validate_citations(&root, &raw_citations);

            // One correction turn if claimed citations are invalid or malformed.
            if validated.is_empty()
                && !correction_used
                && (!raw_citations.is_empty() || content.contains("<final_answer>"))
            {
                correction_used = true;
                messages.push(ChatMessage::user(
                    "The final_answer citations were missing or invalid. Return only one citation per line in the exact form `repository/path:START-END (reason)` inside <final_answer>. Correct paths or lines with tools if needed.",
                ));
                continue;
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
                    cached_prompt_tokens: None,
                    completion_tokens: if completion_tokens > 0 {
                        Some(completion_tokens)
                    } else {
                        None
                    },
                    reasoning_output_tokens: None,
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

fn sandbox_search_arguments(name: &str, arguments: &str, root: &Path) -> String {
    let field = match name {
        "Read" | "Grep" => "path",
        "Glob" => "directory",
        _ => return arguments.to_string(),
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return arguments.to_string();
    };
    let Some(path) = value.get(field).and_then(|path| path.as_str()) else {
        return arguments.to_string();
    };

    let trimmed = path.trim_start_matches(['/', '\\']);
    let root_name = root.file_name().and_then(|name| name.to_str());
    let named_relative = root_name
        .and_then(|root_name| trimmed.strip_prefix(root_name))
        .and_then(|path| path.strip_prefix(['/', '\\']));
    let parts: Vec<_> = trimmed.split(['/', '\\']).collect();
    let existing_relative = (1..parts.len())
        .map(|index| parts[index..].join("/"))
        .find(|path| root.join(path).exists());
    let relative = named_relative
        .map(str::to_owned)
        .or(existing_relative)
        .filter(|path| resolve_in_root(root, path).is_ok());

    if let Some(relative) = relative {
        value[field] = relative.into();
        debug!(field, "normalized model path inside repository root");
    } else if Path::new(path).is_absolute() && resolve_in_root(root, path).is_err() {
        if name == "Read" {
            value[field] = trimmed.into();
            debug!(
                field,
                "normalized escaped model read inside repository root"
            );
        } else {
            value.as_object_mut().unwrap().remove(field);
            debug!(field, "broadened escaped model search to repository root");
        }
    } else {
        return arguments.to_string();
    }
    value.to_string()
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
            cached_prompt_tokens: None,
            completion_tokens: if completion_tokens > 0 {
                Some(completion_tokens)
            } else {
                None
            },
            reasoning_output_tokens: None,
        },
        raw_final: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grephound_model::{MockModel, MockScript, MockStep};
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

    #[tokio::test]
    async fn turn_limit_forces_an_answer_without_tools() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("lib.rs"), "fn answer() {}\n").unwrap();
        let model = Arc::new(MockModel::new(MockScript {
            steps: vec![
                MockStep::Tools(vec![("Read".into(), r#"{"path":"lib.rs"}"#.into())]),
                MockStep::FinalWithoutTools(
                    "<final_answer>\nlib.rs:1-1 (answer)\n</final_answer>".into(),
                ),
            ],
        }));
        let engine = ScoutEngine::new(model, RepoTools::new(dir.path()), ExplorerBudget::default());

        let result = engine
            .scout(ScoutRequest {
                query: "find answer".into(),
                root: dir.path().to_path_buf(),
                focus: None,
                max_turns: Some(1),
                timeout: None,
            })
            .await
            .unwrap();

        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.stats.turns, 2);
    }

    #[tokio::test]
    async fn duplicate_tool_calls_are_not_executed_twice() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn setup() {}\n").unwrap();
        let repeated = ("Grep".into(), r#"{"pattern":"setup"}"#.into());
        let model = Arc::new(MockModel::new(MockScript {
            steps: vec![
                MockStep::Tools(vec![repeated.clone()]),
                MockStep::Tools(vec![repeated]),
                MockStep::Final("<final_answer>\na.rs:1-1\n</final_answer>".into()),
            ],
        }));
        let engine = ScoutEngine::new(model, RepoTools::new(dir.path()), ExplorerBudget::default());

        let result = engine
            .scout(ScoutRequest {
                query: "find setup".into(),
                root: dir.path().into(),
                focus: None,
                max_turns: Some(4),
                timeout: None,
            })
            .await
            .unwrap();

        assert_eq!(result.stats.tool_calls, 1);
        assert_eq!(result.citations.len(), 1);
    }

    #[tokio::test]
    async fn malformed_final_answer_gets_one_correction_turn() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn setup() {}\n").unwrap();
        let model = Arc::new(MockModel::new(MockScript {
            steps: vec![
                MockStep::Final("<final_answer>setup is in a.rs</final_answer>".into()),
                MockStep::Final("<final_answer>\na.rs:1-1 (setup)\n</final_answer>".into()),
            ],
        }));
        let engine = ScoutEngine::new(model, RepoTools::new(dir.path()), ExplorerBudget::default());

        let result = engine
            .scout(ScoutRequest {
                query: "find setup".into(),
                root: dir.path().into(),
                focus: None,
                max_turns: Some(4),
                timeout: None,
            })
            .await
            .unwrap();

        assert_eq!(result.stats.turns, 2);
        assert_eq!(result.citations.len(), 1);
    }

    #[test]
    fn escaped_model_searches_fall_back_to_repository_root() {
        let dir = tempdir().unwrap();
        let arguments = sandbox_search_arguments(
            "Grep",
            r#"{"pattern":"token","path":"/guessed/auth"}"#,
            dir.path(),
        );
        let value: serde_json::Value = serde_json::from_str(&arguments).unwrap();
        assert_eq!(value["pattern"], "token");
        assert!(value.get("path").is_none());
    }

    #[test]
    fn escaped_model_reads_are_normalized_inside_repository_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("Gigo");
        let arguments = sandbox_search_arguments(
            "Read",
            r#"{"path":"/Gigo/internal/server/server.go"}"#,
            &root,
        );
        let value: serde_json::Value = serde_json::from_str(&arguments).unwrap();
        assert_eq!(value["path"], "internal/server/server.go");
    }

    #[test]
    fn model_paths_prefixed_with_repository_name_are_normalized() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("checkout");
        fs::create_dir_all(root.join("crates/core")).unwrap();

        for (tool, field) in [("Read", "path"), ("Grep", "path"), ("Glob", "directory")] {
            let arguments = format!(r#"{{"{field}":"grephound/crates/core","pattern":"engine"}}"#);
            let normalized = sandbox_search_arguments(tool, &arguments, &root);
            let value: serde_json::Value = serde_json::from_str(&normalized).unwrap();
            assert_eq!(value[field], "crates/core");
        }
    }
}
