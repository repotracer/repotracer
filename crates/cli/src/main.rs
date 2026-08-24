mod agents;
mod config;
mod doctor;
mod setup;
mod subscription;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use repotracer_core::{ExplorerBudget, RepoTracerConfig, ScoutBackend, ScoutEngine, ScoutRequest};
use repotracer_mcp::McpServer;
use repotracer_model::{MockModel, ModelBackend, ModelConfig, OpenAiCompatBackend};
use repotracer_repo_tools::RepoTools;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "repotracer",
    version,
    about = "Repository scout for AI coding agents. Small models search. Big models solve.",
    long_about = None
)]
struct Cli {
    /// Natural-language question (shorthand for `scout`)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    query: Vec<String>,

    #[command(subcommand)]
    command: Option<Commands>,

    /// JSON output
    #[arg(long, global = true)]
    json: bool,

    /// Repository root (default: cwd)
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Config file path
    #[arg(long, global = true, env = "REPOTRACER_CONFIG")]
    config: Option<PathBuf>,

    /// Override model name
    #[arg(long, global = true)]
    model: Option<String>,

    /// Override model base URL
    #[arg(long, global = true)]
    base_url: Option<String>,

    /// Use deterministic mock model (CI / offline)
    #[arg(long, global = true, hide = true)]
    mock: bool,

    /// Verbose logs on stderr
    #[arg(long, short, global = true)]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Explore the repository with the configured scout
    Scout {
        /// Question to ask
        query: Vec<String>,
        #[arg(long)]
        max_turns: Option<u32>,
    },
    /// Zero-question GPT setup: verify Codex and configure detected agents
    Setup {
        #[arg(long)]
        dry_run: bool,
    },
    /// Run MCP server on stdio
    Serve,
    /// Diagnose installation and connectivity
    Doctor,
    /// Show current configuration
    Status,
    /// Show or write configuration
    Config {
        #[arg(long)]
        init: bool,
        #[arg(long)]
        path: bool,
    },
    /// Run benchmark harness
    Benchmark {
        #[arg(long)]
        suite: Option<String>,
    },
    /// Remove repotracer agent integrations and local config
    Uninstall {
        #[arg(long)]
        yes: bool,
    },
    /// Print version
    Version,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let root = cli
        .root
        .clone()
        .unwrap_or(std::env::current_dir().context("cwd")?);
    let cfg_path = cli
        .config
        .clone()
        .unwrap_or_else(config::default_config_path);
    let mut cfg = config::load_or_default(&cfg_path);
    if let Some(m) = &cli.model {
        cfg.model.model = m.clone();
    }
    if let Some(u) = &cli.base_url {
        cfg.model.base_url = u.clone();
        cfg.model.backend = "openai-compatible".into();
    }

    // Shorthand: repotracer "where is auth?"
    if cli.command.is_none() && !cli.query.is_empty() {
        let q = cli.query.join(" ");
        return cmd_scout(&root, &cfg, &q, None, cli.json, cli.mock).await;
    }

    let Some(command) = cli.command else {
        return cmd_welcome(&root);
    };

    match command {
        Commands::Scout { query, max_turns } => {
            let q = query.join(" ");
            if q.trim().is_empty() {
                bail!("query required. Example: repotracer scout \"where is auth handled?\"");
            }
            cmd_scout(&root, &cfg, &q, max_turns, cli.json, cli.mock).await
        }
        Commands::Setup { dry_run } => setup::run(&root, &cfg_path, &cfg, dry_run).await,
        Commands::Serve => cmd_serve(&root, &cfg, cli.mock).await,
        Commands::Doctor => doctor::run(&root, &cfg, cli.json).await,
        Commands::Status => cmd_status(&root, &cfg_path, &cfg, cli.json),
        Commands::Config { init, path } => {
            if path {
                println!("{}", cfg_path.display());
                return Ok(());
            }
            if init {
                cfg.save_to(&cfg_path)?;
                println!("Wrote {}", cfg_path.display());
                return Ok(());
            }
            println!("{}", toml::to_string_pretty(&cfg)?);
            Ok(())
        }
        Commands::Benchmark { suite } => {
            println!(
                "Benchmark harness: see `repotracer-bench` and benchmarks/.\nSuite: {}",
                suite.as_deref().unwrap_or("default")
            );
            println!("Run: cargo run -p repotracer-bench -- --help");
            Ok(())
        }
        Commands::Uninstall { yes } => setup::uninstall(&root, yes),
        Commands::Version => {
            println!("repotracer {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

/// Bare `repotracer` / `npx repotracer`: orient the user instead of printing a version.
fn cmd_welcome(root: &std::path::Path) -> Result<()> {
    let configured = agents::detect(root).iter().any(|a| a.configured);
    println!("repotracer {}", env!("CARGO_PKG_VERSION"));
    println!("Small models search. Big models solve.\n");
    if configured {
        println!("Codex is configured. Keep prompting Codex normally.\n");
        println!("  repotracer doctor              check the installation");
        println!("  repotracer \"where is auth?\"    ask this repository directly");
        println!("  repotracer uninstall --yes     remove the Codex integration");
    } else {
        println!("Not set up yet. One command connects RepoTracer to Codex:\n");
        println!("  repotracer setup               configure Codex (uses your existing login)");
        println!("  repotracer setup --dry-run     preview without changing anything");
        println!("  repotracer doctor              check what is missing");
    }
    println!("\nAll commands: repotracer --help");
    Ok(())
}

async fn cmd_scout(
    root: &std::path::Path,
    cfg: &RepoTracerConfig,
    query: &str,
    max_turns: Option<u32>,
    json: bool,
    mock: bool,
) -> Result<()> {
    let engine = build_scout(root, cfg, mock)?;
    let result = engine
        .scout(ScoutRequest {
            query: query.to_string(),
            root: root.to_path_buf(),
            focus: None,
            max_turns,
            timeout: None,
        })
        .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print!("{}", result.cli_text());
    }
    Ok(())
}

async fn cmd_serve(root: &std::path::Path, cfg: &RepoTracerConfig, mock: bool) -> Result<()> {
    // Logs must not touch stdout.
    let engine = build_scout(root, cfg, mock)?;
    let server = McpServer::new(engine, root.to_path_buf());
    server.serve_stdio().await
}

fn cmd_status(
    root: &std::path::Path,
    cfg_path: &std::path::Path,
    cfg: &RepoTracerConfig,
    json: bool,
) -> Result<()> {
    let agents = agents::detect(root);
    if json {
        let mut config = serde_json::to_value(cfg)?;
        if let Some(api_key) = config.pointer_mut("/model/api_key") {
            if !api_key.is_null() {
                *api_key = serde_json::Value::String("<redacted>".into());
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "root": root,
                "config_path": cfg_path,
                "config": config,
                "agents": agents,
            }))?
        );
    } else {
        println!("repotracer status\n");
        println!("Root     {}", root.display());
        println!("Config   {}", cfg_path.display());
        if subscription::is_subscription_backend(cfg) {
            println!("Model    {}", cfg.model.model);
        } else {
            println!("Model    {} @ {}", cfg.model.model, cfg.model.base_url);
        }
        println!("Backend  {}", cfg.model.backend);
        println!("\nAgents");
        for a in agents {
            let mark = if a.configured { "✓" } else { "○" };
            println!(
                "  {mark} {} {}",
                a.name,
                if a.configured {
                    "configured"
                } else {
                    "not configured"
                }
            );
        }
    }
    Ok(())
}

fn build_scout(
    root: &std::path::Path,
    cfg: &RepoTracerConfig,
    mock: bool,
) -> Result<Arc<dyn ScoutBackend>> {
    if !mock && subscription::is_subscription_backend(cfg) {
        return Ok(Arc::new(subscription::CliScout::from_config(cfg)?));
    }
    if !mock && !cfg.model.model.starts_with("gpt-") {
        bail!(
            "unsupported model `{}`: RepoTracer currently supports GPT scouts only",
            cfg.model.model
        );
    }
    let model: Arc<dyn ModelBackend> = if mock {
        Arc::new(MockModel::grep_then_cite(
            "README.md",
            1,
            5,
            "mock citation",
        ))
    } else {
        Arc::new(OpenAiCompatBackend::new(ModelConfig {
            base_url: cfg.model.base_url.clone(),
            model: cfg.model.model.clone(),
            api_key: cfg.model.resolved_api_key(),
            timeout_ms: cfg.model.timeout_ms,
            temperature: cfg.model.temperature,
            max_tokens: None,
        })?)
    };
    let tools = RepoTools::new(root);
    let budget = ExplorerBudget {
        max_turns: cfg.explorer.max_turns,
        timeout_seconds: cfg.explorer.timeout_seconds,
        max_tool_calls: cfg.explorer.max_tool_calls,
        tool_timeout_seconds: cfg.explorer.tool_timeout_seconds,
        concurrency: cfg.explorer.concurrency,
    };
    Ok(Arc::new(ScoutEngine::new(model, tools, budget)))
}

fn init_tracing(verbose: bool) {
    let filter = if verbose {
        EnvFilter::new("info,repotracer=debug,repotracer_core=debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}
