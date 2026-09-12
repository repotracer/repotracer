use crate::McpServer;
use async_trait::async_trait;
use repotracer_core::{ExplorerBudget, ScoutEngine};
use repotracer_model::{
    ChatMessage, FunctionCall, MessageRole, ModelBackend, ModelError, ModelRequest, ModelResponse,
};
use repotracer_repo_tools::RepoTools;
use serde_json::{json, Value};
use std::sync::Arc;

struct RepositoryModel;

#[async_trait]
impl ModelBackend for RepositoryModel {
    fn name(&self) -> &str {
        "repository-fixture"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let query = request.messages[1].content.as_deref().unwrap();
        let expected = if query.contains("alpha") {
            "alpha"
        } else {
            "beta"
        };
        let other = if expected == "alpha" { "beta" } else { "alpha" };
        let outputs: Vec<_> = request
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
            .collect();
        let message = if outputs.is_empty() {
            tokio::task::yield_now().await;
            ChatMessage::assistant_tools(
                None,
                [
                    ("Read", json!({"path": format!("{expected}.rs")})),
                    ("Grep", json!({"pattern": expected})),
                    ("Glob", json!({"pattern": "*.rs"})),
                    ("Symbols", json!({"symbol": expected})),
                ]
                .into_iter()
                .map(|(name, arguments)| FunctionCall {
                    id: name.into(),
                    name: name.into(),
                    arguments: arguments.to_string(),
                })
                .collect(),
            )
        } else {
            assert_eq!(outputs.len(), 4);
            for output in outputs {
                let text = output.content.as_deref().unwrap();
                assert!(
                    text.contains(expected),
                    "{}: {text}",
                    output.tool_call_id.as_deref().unwrap()
                );
                assert!(!text.contains(other), "foreign repository content: {text}");
            }
            ChatMessage::assistant(format!(
                "<final_answer>\n{expected}.rs:1-1 (definition)\n</final_answer>"
            ))
        };
        Ok(ModelResponse {
            message,
            model: self.name().into(),
            usage: None,
        })
    }
}

#[tokio::test]
async fn generic_tools_follow_selected_repository_and_conversation() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("alpha.rs"), "fn alpha() {}\n").unwrap();
    std::fs::write(b.path().join("beta.rs"), "fn beta() {}\n").unwrap();
    let engine = ScoutEngine::new(
        Arc::new(RepositoryModel),
        RepoTools::new(a.path()),
        ExplorerBudget::default(),
    );
    let server = McpServer::new(Arc::new(engine), a.path().to_owned());
    let call = |arguments: Value| json!({"name":"repo_scout", "arguments":arguments});
    let (first, second) = tokio::join!(
        server.tools_call(call(json!({"query":"find alpha", "repository":a.path()}))),
        server.tools_call(call(json!({"query":"find beta", "repository":b.path()}))),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        first["structuredContent"]["evidence"][0]["text"],
        "1: fn alpha() {}"
    );
    assert_eq!(
        second["structuredContent"]["evidence"][0]["text"],
        "1: fn beta() {}"
    );
    let id = second["structuredContent"]["conversation"]["id"]
        .as_str()
        .unwrap();
    let followup = server
        .tools_call(call(
            json!({"query":"find beta", "investigation":{"conversation_id":id}}),
        ))
        .await
        .unwrap();
    assert_eq!(
        followup["structuredContent"]["evidence"][0]["text"],
        "1: fn beta() {}"
    );
}
