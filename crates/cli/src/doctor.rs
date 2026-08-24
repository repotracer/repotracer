use crate::agents;
use crate::subscription::{is_subscription_backend, CliScout};
use anyhow::{bail, Result};
use repotracer_core::RepoTracerConfig;
use repotracer_model::{ModelBackend, ModelConfig, OpenAiCompatBackend};
use repotracer_repo_tools::{RepoTools, ToolCall};
use serde_json::json;
use std::path::Path;
use std::time::Instant;

pub async fn run(root: &Path, cfg: &RepoTracerConfig, json_mode: bool) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    checks.push(Check::ok("binary", "repotracer binary"));
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

    // Configured scout backend
    if is_subscription_backend(cfg) {
        match CliScout::from_config(cfg) {
            Ok(scout) => {
                let installed =
                    scout.executable().is_file() || which::which(scout.executable()).is_ok();
                checks.push(if installed {
                    Check::ok(
                        "GPT scout CLI",
                        &format!("{} ({})", scout.label(), scout.executable().display()),
                    )
                } else {
                    Check::fail(
                        "GPT scout CLI",
                        &format!("{} not found", scout.executable().display()),
                    )
                });
                if installed {
                    checks.push(match scout.probe(root).await {
                        Ok(()) => Check::ok("model", &format!("{} ready", scout.label())),
                        Err(error) => Check::fail("model", &error.to_string()),
                    });
                }
            }
            Err(error) => checks.push(Check::fail("GPT scout CLI", &error.to_string())),
        }
    } else {
        let backend = OpenAiCompatBackend::new(ModelConfig {
            base_url: cfg.model.base_url.clone(),
            model: cfg.model.model.clone(),
            api_key: cfg.model.resolved_api_key(),
            timeout_ms: cfg.model.timeout_ms.min(10_000),
            temperature: 0.0,
            max_tokens: Some(16),
        });
        match backend {
            Ok(backend) => {
                let request = repotracer_model::ModelRequest {
                    messages: vec![repotracer_model::ChatMessage::user("Reply with ok.")],
                    tools: vec![],
                    temperature: 0.0,
                    max_tokens: Some(8),
                };
                checks.push(match backend.complete(request).await {
                    Ok(_) => Check::ok(
                        "model",
                        &format!("{} @ {}", cfg.model.model, cfg.model.base_url),
                    ),
                    Err(error) => Check::fail("model", &error.to_string()),
                });
            }
            Err(error) => checks.push(Check::fail("model", &error.to_string())),
        }
    }

    // MCP
    checks.push(Check::ok("MCP", "repotracer serve"));

    // Agents
    for a in agents::detect(root) {
        if a.configured {
            checks.push(Check::ok(&a.name, "configured"));
        } else {
            checks.push(Check::warn(
                &a.name,
                "not configured — run repotracer setup",
            ));
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
        if !ready {
            bail!("installation is not ready");
        }
        return Ok(());
    }

    println!("repotracer doctor\n");
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
    for check in checks
        .iter()
        .filter(|check| matches!(check.name.as_str(), "GPT scout CLI" | "model"))
    {
        print_check(check);
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
            || c.name.contains("Copilot")
            || c.name.contains("MCP (generic)")
    }) {
        print_check(c);
    }

    println!();
    if ready {
        println!("READY");
        Ok(())
    } else {
        println!("NOT READY — fix the ✗ items above, then re-run: repotracer doctor");
        bail!("installation is not ready")
    }
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
