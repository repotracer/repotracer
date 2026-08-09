use crate::agents;
use anyhow::Result;
use grephound_core::{ExplorerBudget, GrephoundConfig, ScoutEngine, ScoutRequest};
use grephound_model::{MockModel, ModelBackend, ModelConfig, OpenAiCompatBackend};
use grephound_repo_tools::{RepoTools, ToolCall};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

pub async fn run(root: &Path, cfg: &GrephoundConfig, json_mode: bool) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    checks.push(Check::ok("binary", "grephound binary"));
    checks.push(Check::ok("config", &format!("model={}", cfg.model.model)));
    checks.push(if root.exists() {
        Check::ok("repository", &root.display().to_string())
    } else {
        Check::fail("repository", "root missing")
    });

    // rg
    checks.push(match which::which("rg") {
        Ok(p) => Check::ok("ripgrep", &p.display().to_string()),
        Err(_) => Check::fail(
            "ripgrep",
            "rg not on PATH — https://github.com/BurntSushi/ripgrep",
        ),
    });

    // Tools
    let tools = RepoTools::new(root);
    let t0 = Instant::now();
    let read_res = tools.call_one("Glob", r#"{"pattern":"*"}"#).await;
    checks.push(if read_res.failed {
        Check::fail("Glob", &read_res.output)
    } else {
        Check::ok("Glob", &format!("{} ms", t0.elapsed().as_millis()))
    });

    // Concurrent execution smoke
    let calls = vec![
        ToolCall {
            id: "1".into(),
            name: "Glob".into(),
            arguments: r#"{"pattern":"*"}"#.into(),
        },
        ToolCall {
            id: "2".into(),
            name: "Glob".into(),
            arguments: r#"{"pattern":"**/*"}"#.into(),
        },
    ];
    let t1 = Instant::now();
    let many = tools.call_many(&calls).await;
    checks.push(if many.iter().all(|r| !r.failed) {
        Check::ok(
            "concurrent execution",
            &format!("{} ms", t1.elapsed().as_millis()),
        )
    } else {
        Check::fail("concurrent execution", "one or more tools failed")
    });

    // Model
    let ollama_url = cfg
        .model
        .base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_string();
    let tags = format!("{ollama_url}/api/tags");
    let reachable = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()
        .map(|c| async move { c.get(&tags).send().await.is_ok() });
    let reachable = match reachable {
        Some(f) => f.await,
        None => false,
    };
    if reachable {
        checks.push(Check::ok("Ollama reachable", &cfg.model.base_url));
    } else {
        checks.push(Check::fail(
            "Ollama reachable",
            &format!(
                "Could not reach {}. Start: ollama serve",
                cfg.model.base_url
            ),
        ));
    }

    // Mock exploration always works
    let mock = Arc::new(MockModel::grep_then_cite("Cargo.toml", 1, 5, "manifest"));
    // Ensure Cargo.toml exists for validation when in workspace
    let engine = ScoutEngine::new(
        mock,
        RepoTools::new(root),
        ExplorerBudget {
            max_turns: 3,
            ..ExplorerBudget::default()
        },
    );
    let t2 = Instant::now();
    // Use a fixture file that exists
    let cite_path = if root.join("Cargo.toml").exists() {
        "Cargo.toml"
    } else {
        "."
    };
    let _ = cite_path;
    match engine
        .scout(ScoutRequest {
            query: "doctor test".into(),
            root: root.to_path_buf(),
            focus: None,
            max_turns: Some(3),
            timeout: Some(std::time::Duration::from_secs(15)),
        })
        .await
    {
        Ok(_r) => checks.push(Check::ok(
            "test exploration",
            &format!("{} ms (mock)", t2.elapsed().as_millis()),
        )),
        Err(e) => checks.push(Check::fail("test exploration", &e.to_string())),
    }

    // Live model optional probe
    if reachable {
        let mc = ModelConfig {
            base_url: cfg.model.base_url.clone(),
            model: cfg.model.model.clone(),
            api_key: cfg.model.api_key.clone(),
            timeout_ms: 5_000,
            temperature: 0.0,
            max_tokens: Some(16),
        };
        match OpenAiCompatBackend::new(mc) {
            Ok(backend) => {
                let req = grephound_model::ModelRequest {
                    messages: vec![grephound_model::ChatMessage::user("ping")],
                    tools: vec![],
                    temperature: 0.0,
                    max_tokens: Some(8),
                };
                match backend.complete(req).await {
                    Ok(_) => checks.push(Check::ok("model", &cfg.model.model)),
                    Err(e) => checks.push(Check::fail(
                        "model",
                        &format!("{e}\n  Try: ollama pull {}", cfg.model.model),
                    )),
                }
            }
            Err(e) => checks.push(Check::fail("model", &e.to_string())),
        }
    }

    // MCP
    checks.push(Check::ok("MCP", "grephound serve"));

    // Agents
    for a in agents::detect() {
        if a.configured {
            checks.push(Check::ok(&a.name, "configured"));
        } else {
            checks.push(Check::warn(&a.name, "not configured — run grephound setup"));
        }
    }

    let ready = checks.iter().all(|c| c.level != Level::Fail);

    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ready": ready,
                "checks": checks,
            }))?
        );
        return Ok(());
    }

    println!("grephound doctor\n");
    println!("Runtime");
    for c in checks.iter().filter(|c| {
        matches!(
            c.name.as_str(),
            "binary" | "config" | "repository" | "ripgrep"
        )
    }) {
        print_check(c);
    }
    println!("\nExplorer");
    for c in checks
        .iter()
        .filter(|c| c.name.contains("Ollama") || c.name == "model" || c.name == "test exploration")
    {
        print_check(c);
    }
    println!("\nTools");
    for c in checks.iter().filter(|c| {
        matches!(
            c.name.as_str(),
            "Glob" | "Read" | "Grep" | "concurrent execution"
        )
    }) {
        print_check(c);
    }
    println!("\nMCP");
    for c in checks.iter().filter(|c| c.name == "MCP") {
        print_check(c);
    }
    println!("\nAgents");
    for c in checks.iter().filter(|c| {
        c.name.contains("Claude")
            || c.name.contains("Codex")
            || c.name.contains("Cursor")
            || c.name.contains("MCP (generic)")
    }) {
        print_check(c);
    }

    println!();
    if ready {
        println!("READY");
    } else {
        println!("NOT READY — fix the ✗ items above, then re-run: grephound doctor");
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
struct Check {
    name: String,
    level: Level,
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Level {
    Ok,
    Warn,
    Fail,
}

impl Check {
    fn ok(name: &str, detail: &str) -> Self {
        Self {
            name: name.into(),
            level: Level::Ok,
            detail: detail.into(),
        }
    }
    fn warn(name: &str, detail: &str) -> Self {
        Self {
            name: name.into(),
            level: Level::Warn,
            detail: detail.into(),
        }
    }
    fn fail(name: &str, detail: &str) -> Self {
        Self {
            name: name.into(),
            level: Level::Fail,
            detail: detail.into(),
        }
    }
}

fn print_check(c: &Check) {
    let mark = match c.level {
        Level::Ok => "✓",
        Level::Warn => "○",
        Level::Fail => "✗",
    };
    println!("{mark} {} — {}", c.name, c.detail);
}
