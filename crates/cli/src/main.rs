mod adaptive;
mod agents;
mod claude;
mod config;
mod doctor;
mod model_catalog;
mod select;
mod selfupdate;
mod session;
mod settings;
mod setup;
mod subscription;
#[cfg(windows)]
mod windows_job;
mod wizard;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use repotracer_core::{ExplorerBudget, RepoTracerConfig, ScoutBackend, ScoutEngine, ScoutRequest};
use repotracer_core::{InvestigationIntent, InvestigationSpec};
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
    about = "Repository scout for AI coding agents. Small models investigate. Big models solve.",
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
        /// Investigation strategy selected by the caller
        #[arg(long, default_value = "locate", value_parser = ["locate", "explain", "change_impact", "diagnose", "inventory"])]
        intent: String,
        /// Explicit question to cover; repeat for multiple questions
        #[arg(long = "question")]
        questions: Vec<String>,
        /// Context already known to the parent, treated as unverified
        #[arg(long, default_value = "")]
        known_context: String,
        /// Repository-relative path of a likely target; repeat as needed
        #[arg(long = "target")]
        target_paths: Vec<String>,
    },
    /// Install with the two-step terminal wizard, or use --agents for automation
    Setup {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_parser = ["codex", "claude", "both"])]
        agents: Option<String>,
    },
    /// Configure installed harnesses and choose a subscription tracer model
    #[command(visible_alias = "reconfigure")]
    Settings {
        #[arg(long, value_parser = ["codex", "claude", "both"])]
        agents: Option<String>,
        /// One tracer model for the selected harnesses, e.g. claude:sonnet
        #[arg(long, requires = "agents", conflicts_with_all = ["codex_scout", "claude_scout", "codex_model", "claude_model"])]
        tracer_model: Option<String>,
        #[arg(long, value_parser = ["codex", "claude"])]
        codex_scout: Option<String>,
        #[arg(long, value_parser = ["codex", "claude"])]
        claude_scout: Option<String>,
        #[arg(long)]
        codex_model: Option<String>,
        #[arg(long)]
        claude_model: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Run MCP server on stdio
    Serve,
    /// Query repository syntax without a model call
    Symbols {
        #[arg(default_value = "")]
        symbol: String,
        #[arg(long, default_value = "definitions", value_parser = ["definitions", "references", "outline"])]
        mode: String,
        #[arg(long, default_value = ".")]
        path: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
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
    /// Update the installed binary to the newest release now
    Update,
    /// Refresh files managed by the updater
    #[command(name = "__refresh-integration", hide = true)]
    RefreshIntegration,
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
        if cfg.model.is_claude() || subscription::is_subscription_backend(&cfg) {
            cfg.model.reasoning_effort.clear();
        }
        cfg.model.base_url = u.clone();
        cfg.model.backend = "openai-compatible".into();
    }

    // Shorthand: repotracer "where is auth?"
    if cli.command.is_none() && !cli.query.is_empty() {
        let q = cli.query.join(" ");
        return cmd_scout(
            &root,
            &cfg,
            &q,
            None,
            cli.json,
            cli.mock,
            InvestigationSpec::default(),
        )
        .await;
    }

    let Some(command) = cli.command else {
        return cmd_welcome(&root);
    };

    match command {
        Commands::Scout {
            query,
            max_turns,
            intent,
            questions,
            known_context,
            target_paths,
        } => {
            let q = query.join(" ");
            if q.trim().is_empty() {
                bail!("query required. Example: repotracer scout \"where is auth handled?\"");
            }
            let intent: InvestigationIntent =
                serde_json::from_value(serde_json::Value::String(intent))?;
            cmd_scout(
                &root,
                &cfg,
                &q,
                max_turns,
                cli.json,
                cli.mock,
                InvestigationSpec {
                    intent,
                    questions,
                    known_context,
                    target_paths,
                    conversation_id: None,
                    reasoning_effort: None,
                    ..Default::default()
                },
            )
            .await
        }
        Commands::Setup {
            dry_run,
            agents: Some(targets),
        } => settings::run(
            &cfg_path,
            &cfg,
            Some(targets),
            None,
            None,
            None,
            None,
            dry_run,
        ),
        Commands::Setup {
            dry_run,
            agents: None,
        } => {
            use std::io::IsTerminal;
            if cli.model.is_none()
                && cli.base_url.is_none()
                && (std::io::stdin().is_terminal()
                    || cfg_path.with_extension("integrations.json").exists())
            {
                settings::run(&cfg_path, &cfg, None, None, None, None, None, dry_run)
            } else {
                setup::run(&root, &cfg_path, &cfg, dry_run).await
            }
        }
        Commands::Settings {
            agents,
            tracer_model,
            mut codex_scout,
            mut claude_scout,
            mut codex_model,
            mut claude_model,
            dry_run,
        } => {
            if let Some(model) = tracer_model {
                let (provider, model) = settings::parse_tracer_model(&model)?;
                if matches!(agents.as_deref(), Some("codex" | "both")) {
                    codex_scout = Some(provider.into());
                    codex_model = Some(model.into());
                }
                if matches!(agents.as_deref(), Some("claude" | "both")) {
                    claude_scout = Some(provider.into());
                    claude_model = Some(model.into());
                }
            }
            settings::run(
                &cfg_path,
                &cfg,
                agents,
                codex_scout,
                claude_scout,
                codex_model,
                claude_model,
                dry_run,
            )
        }
        Commands::Serve => cmd_serve(&root, &cfg, cli.mock).await,
        Commands::Symbols {
            symbol,
            mode,
            path,
            offset,
        } => {
            let index = repotracer_repo_tools::RepositoryIndex::new(root);
            println!("{}", index.call(&serde_json::json!({"symbol":symbol,"mode":mode,"path":path,"offset":offset}).to_string()).await?);
            Ok(())
        }
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
            println!("{}", redacted_toml(&cfg)?);
            Ok(())
        }
        Commands::Update => selfupdate::run_now().await,
        Commands::RefreshIntegration => {
            if !settings::refresh(&cfg_path)? {
                agents::install_codex(&agents::current_binary(), false)?;
            }
            Ok(())
        }
        Commands::Uninstall { yes } => {
            if yes {
                settings::uninstall(&cfg_path)?;
            }
            setup::uninstall(&root, &cfg_path, yes)
        }
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
    println!("Small models investigate. Big models solve.");
    println!("Better answers than searching alone, at a fraction of the cost.\n");
    if configured {
        println!("Parent integration is configured. Keep prompting normally.\n");
        println!("  repotracer settings            change parent/scout mappings");
        println!("  repotracer doctor              check the installation");
        println!("  repotracer \"where is auth?\"    ask this repository directly");
        println!("  repotracer uninstall --yes     remove installed integrations");
    } else {
        println!("Not set up yet. Choose Codex, Claude Code, or both:\n");
        println!("  repotracer setup --agents both configure both parents with matching scouts");
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
    investigation: InvestigationSpec,
) -> Result<()> {
    let engine = build_scout(root, cfg, mock)?;
    let result = engine
        .scout(ScoutRequest {
            investigation,
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
    let discovered_efforts = (!mock
        && adaptive_enabled(cfg)
        && (cfg.model.is_claude() || subscription::is_subscription_backend(cfg)))
    .then(|| native_supported_efforts(cfg))
    .flatten();
    let engine = build_scout_with_efforts(root, cfg, mock, discovered_efforts.clone())?;
    // Off the request path and before the first message. The swap only ever
    // affects the next launch.
    selfupdate::spawn(cfg.updates.automatic);
    let advertised_efforts = model_catalog::advertised_reasoning_efforts(
        &cfg.model.backend,
        &cfg.model.model,
        discovered_efforts.as_deref(),
    );
    let server =
        McpServer::new(engine, root.to_path_buf()).with_reasoning_efforts(advertised_efforts);
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
        if let Some(base_url) = config.pointer_mut("/model/base_url") {
            if let Some(value) = base_url.as_str() {
                *base_url = serde_json::Value::String(redact_endpoint(value));
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
            println!(
                "Model    {} @ {}",
                cfg.model.model,
                redact_endpoint(&cfg.model.base_url)
            );
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

pub(crate) fn redact_endpoint(value: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(value) else {
        // Invalid input can still contain userinfo, query tokens, or fragments.
        // Do not guess which parts of an unparseable URL are safe to print.
        return "<invalid endpoint>".into();
    };
    if url.query().is_some() {
        url.set_query(None);
    }
    if url.fragment().is_some() {
        url.set_fragment(None);
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
}

fn redacted_toml(cfg: &RepoTracerConfig) -> Result<String> {
    let mut value = toml::Value::try_from(cfg)?;
    if let Some(model) = value.get_mut("model").and_then(toml::Value::as_table_mut) {
        if model.get("api_key").is_some() {
            model.insert("api_key".into(), toml::Value::String("<redacted>".into()));
        }
        if let Some(base_url) = model
            .get("base_url")
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
        {
            let redacted = redact_endpoint(&base_url);
            *model.get_mut("base_url").expect("base_url still present") =
                toml::Value::String(redacted);
        }
    }
    Ok(toml::to_string_pretty(&value)?)
}

fn build_scout(
    root: &std::path::Path,
    cfg: &RepoTracerConfig,
    mock: bool,
) -> Result<Arc<dyn ScoutBackend>> {
    let discovered_efforts = (!mock
        && adaptive_enabled(cfg)
        && (cfg.model.is_claude() || subscription::is_subscription_backend(cfg)))
    .then(|| native_supported_efforts(cfg))
    .flatten();
    build_scout_with_efforts(root, cfg, mock, discovered_efforts)
}

fn build_scout_with_efforts(
    root: &std::path::Path,
    cfg: &RepoTracerConfig,
    mock: bool,
    discovered_efforts: Option<Vec<String>>,
) -> Result<Arc<dyn ScoutBackend>> {
    if !mock && cfg.model.is_claude() {
        let backend: Arc<dyn ScoutBackend> = Arc::new(claude::ClaudeScout::new(cfg)?);
        let enabled = adaptive_enabled(cfg);
        return Ok(Arc::new(
            adaptive::AdaptiveScout::new(
                backend,
                enabled,
                enabled.then(|| discovered_efforts.clone()).flatten(),
                cfg.model.native_reasoning_effort().to_string(),
            )
            .with_turn_ceiling(cfg.explorer.max_turns),
        ));
    }
    if !mock && subscription::is_subscription_backend(cfg) {
        let backend: Arc<dyn ScoutBackend> = Arc::new(subscription::CliScout::from_config(cfg)?);
        let enabled = adaptive_enabled(cfg);
        return Ok(Arc::new(
            adaptive::AdaptiveScout::new(
                backend,
                enabled,
                enabled.then(|| discovered_efforts.clone()).flatten(),
                cfg.model.native_reasoning_effort().to_string(),
            )
            .with_turn_ceiling(cfg.explorer.max_turns),
        ));
    }
    // Compatible endpoints may expose arbitrary model identifiers. Let the
    // endpoint validate the ID instead of guessing from its prefix.
    let model: Arc<dyn ModelBackend> = if mock {
        Arc::new(MockModel::grep_then_cite(
            "README.md",
            1,
            5,
            "mock citation",
        ))
    } else {
        Arc::new(OpenAiCompatBackend::new(ModelConfig {
            reasoning_effort: (!cfg.model.reasoning_effort.trim().is_empty())
                .then(|| cfg.model.reasoning_effort.clone()),
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
    // The generic OpenAI-compatible engine has no per-request effort control
    // in its ModelBackend contract. Keep it unchanged; adaptive continuation
    // is native-provider-only and must never turn a mock into extra calls.
    Ok(Arc::new(ScoutEngine::new(model, tools, budget)))
}

fn adaptive_enabled(cfg: &RepoTracerConfig) -> bool {
    cfg.model.adaptive_reasoning
        && !matches!(
            std::env::var("REPOTRACER_ADAPTIVE_REASONING")
                .ok()
                .as_deref(),
            Some("0" | "false" | "off" | "no")
        )
}

fn native_supported_efforts(cfg: &RepoTracerConfig) -> Option<Vec<String>> {
    model_catalog::discover_efforts(
        &cfg.model.backend,
        cfg.model.executable.as_deref(),
        cfg.model.model.trim(),
    )
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

#[cfg(test)]
mod endpoint_tests {
    #[test]
    fn mixed_case_claude_backends_use_native_validation() {
        for backend in ["Claude", "CLAUDE-CLI"] {
            let mut config = repotracer_core::RepoTracerConfig::default();
            config.model.backend = backend.into();
            config.model.model = "sonnet".into();
            config.model.adaptive_reasoning = false;
            assert!(super::claude::ClaudeScout::new(&config).is_ok());
            config.model.reasoning_effort = "invalid".into();
            assert!(super::build_scout(std::path::Path::new("."), &config, false).is_err());
        }
    }

    #[test]
    fn invalid_endpoints_do_not_expose_credentials() {
        for endpoint in [
            "localhost/v1?key=secret#secret",
            "https://user:secret@bad host/v1?token=secret#secret",
            "https://user:secret@example.com/v1?token=secret#secret",
        ] {
            assert!(!super::redact_endpoint(endpoint).contains("secret"));
            let mut config = repotracer_core::RepoTracerConfig::default();
            config.model.base_url = endpoint.into();
            config.model.api_key = Some("secret".into());
            assert!(!super::redacted_toml(&config).unwrap().contains("secret"));
        }
    }
}
