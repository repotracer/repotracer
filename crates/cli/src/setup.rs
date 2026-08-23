use crate::agents;
use crate::config;
use crate::subscription::{is_subscription_backend, CliScout};
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use repotracer_core::{RepotracerConfig, ModelSettings};
use repotracer_model::{
    ChatMessage, ModelBackend, ModelConfig, ModelRequest, OpenAiCompatBackend, ToolSpec,
};
use serde_json::json;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderChoice {
    Auto,
    Ollama,
    Codex,
    Claude,
    Custom,
}

pub async fn run(
    root: &Path,
    cfg_path: &Path,
    cfg: &RepotracerConfig,
    yes: bool,
    dry_run: bool,
    requested_provider: ProviderChoice,
) -> Result<()> {
    let interactive = io::stdin().is_terminal() && !yes && !dry_run;
    if !interactive && !yes && !dry_run {
        bail!(
            "setup needs an interactive terminal; use `repotracer setup --yes` for unattended setup"
        );
    }

    banner();
    section("1 / 4  Environment");
    item(
        true,
        &os_label(std::env::consts::OS, std::env::consts::ARCH),
    );
    item(root.join(".git").exists(), "Git repository");
    let has_rg = which::which("rg").is_ok();
    item(has_rg, "ripgrep");

    let detected = agents::detect(root);
    for agent in detected
        .iter()
        .filter(|agent| agent.name != "MCP (generic)")
    {
        item(agent.detected, &format!("{} detected", agent.name));
    }
    if !has_rg && !dry_run {
        bail!("ripgrep is required; install it from https://github.com/BurntSushi/ripgrep, then run setup again");
    }

    section("2 / 4  Scout backend");
    let provider = select_provider(requested_provider, interactive)?;
    let mut selected_cfg = cfg.clone();
    configure_provider(&mut selected_cfg, provider)?;
    item(true, &format!("selected {}", provider_label(provider)));
    if uses_local_ollama(&selected_cfg) {
        ensure_ollama(&selected_cfg, yes, dry_run, interactive).await?;
    } else if is_subscription_backend(&selected_cfg) {
        verify_subscription(root, &selected_cfg, dry_run).await?;
    } else if dry_run {
        plan(&format!(
            "would verify custom endpoint {} ({})",
            selected_cfg.model.base_url, selected_cfg.model.model
        ));
    } else {
        verify_tool_calling(&selected_cfg).await?;
        item(
            true,
            &format!("model tool calling verified ({})", selected_cfg.model.model),
        );
    }

    section("3 / 4  Agent integrations");
    let selected = select_agents(&detected, yes, dry_run, interactive)?;
    let binary = agents::current_binary();
    if selected.is_empty() {
        item(
            false,
            "no agent selected; generic MCP command is `repotracer serve`",
        );
    }
    let mut integration_errors = Vec::new();
    for name in selected {
        let result = match name.as_str() {
            "Claude Code" => agents::install_claude(&binary, dry_run),
            "Codex" => agents::install_codex(&binary, dry_run),
            "Cursor" => agents::install_cursor(&binary, root, dry_run),
            "GitHub Copilot" => agents::install_copilot(&binary, root, dry_run),
            _ => continue,
        };
        match result {
            Ok(message) => item(true, &message),
            Err(error) => {
                item(false, &format!("{name}: {error}"));
                integration_errors.push(format!("{name}: {error}"));
            }
        }
    }
    if !integration_errors.is_empty() {
        bail!(
            "agent configuration failed: {}",
            integration_errors.join("; ")
        );
    }

    section("4 / 4  Repotracer configuration");
    if dry_run {
        plan(&format!("would write {}", cfg_path.display()));
    } else {
        if let Some(parent) = cfg_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        selected_cfg.save_to(cfg_path)?;
        item(true, &format!("config written ({})", cfg_path.display()));
    }
    item(true, &format!("binary ({})", binary.display()));

    println!();
    if dry_run {
        println!(
            "{}",
            style("1;34", "DRY RUN COMPLETE — no files or services changed")
        );
    } else {
        println!(
            "{}",
            style("1;32", "READY — small models search, big models solve")
        );
        println!("\nRestart configured agents, then ask a multi-file repository question.");
        println!("Verify any time: repotracer doctor");
    }
    Ok(())
}

fn select_provider(requested: ProviderChoice, interactive: bool) -> Result<ProviderChoice> {
    if requested != ProviderChoice::Auto {
        return Ok(requested);
    }
    let capable = local_model_recommended();
    let codex = which::which("codex").is_ok();
    let claude = which::which("claude").is_ok();
    let default = provider_for_auto(capable, codex, claude);
    if !interactive {
        return Ok(default);
    }

    println!("Choose the scout backend:");
    println!(
        "  1. Ollama local{}",
        if capable {
            " (recommended for this hardware)"
        } else {
            ""
        }
    );
    println!(
        "  2. Codex subscription{}",
        if codex { " (CLI detected)" } else { "" }
    );
    println!(
        "  3. Claude subscription{}",
        if claude { " (CLI detected)" } else { "" }
    );
    println!("  4. Custom OpenAI-compatible endpoint");
    let default_number = match default {
        ProviderChoice::Ollama => 1,
        ProviderChoice::Codex => 2,
        ProviderChoice::Claude => 3,
        ProviderChoice::Custom | ProviderChoice::Auto => 4,
    };
    print!("Selection [{default_number}]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    match input.trim() {
        "" => Ok(default),
        "1" => Ok(ProviderChoice::Ollama),
        "2" => Ok(ProviderChoice::Codex),
        "3" => Ok(ProviderChoice::Claude),
        "4" => Ok(ProviderChoice::Custom),
        value => bail!("invalid backend selection `{value}`"),
    }
}

fn provider_for_auto(capable: bool, codex: bool, claude: bool) -> ProviderChoice {
    if capable {
        ProviderChoice::Ollama
    } else if codex {
        ProviderChoice::Codex
    } else if claude {
        ProviderChoice::Claude
    } else {
        ProviderChoice::Ollama
    }
}

fn configure_provider(cfg: &mut RepotracerConfig, provider: ProviderChoice) -> Result<()> {
    match provider {
        ProviderChoice::Auto => unreachable!("auto provider must be resolved first"),
        ProviderChoice::Ollama => {
            if !cfg.model.backend.eq_ignore_ascii_case("ollama") {
                cfg.model = ModelSettings::default();
            }
        }
        ProviderChoice::Codex => {
            cfg.model = ModelSettings {
                backend: "codex-cli".into(),
                executable: None,
                model: "default".into(),
                api_key: None,
                timeout_ms: cfg.model.timeout_ms.max(120_000),
                ..ModelSettings::default()
            };
        }
        ProviderChoice::Claude => {
            cfg.model = ModelSettings {
                backend: "claude-cli".into(),
                executable: None,
                model: "haiku".into(),
                api_key: None,
                timeout_ms: cfg.model.timeout_ms.max(120_000),
                ..ModelSettings::default()
            };
        }
        ProviderChoice::Custom => {
            if cfg.model.base_url.contains("127.0.0.1:11434")
                || cfg.model.base_url.contains("localhost:11434")
            {
                bail!("custom setup needs `--base-url <URL> --model <MODEL>` before `setup --provider custom`");
            }
            cfg.model.backend = "openai-compatible".into();
            cfg.model.executable = None;
        }
    }
    Ok(())
}

fn provider_label(provider: ProviderChoice) -> &'static str {
    match provider {
        ProviderChoice::Auto => "automatic selection",
        ProviderChoice::Ollama => "Ollama local",
        ProviderChoice::Codex => "Codex subscription",
        ProviderChoice::Claude => "Claude subscription",
        ProviderChoice::Custom => "custom OpenAI-compatible endpoint",
    }
}

fn local_model_recommended() -> bool {
    physical_memory_bytes().is_some_and(|bytes| bytes >= 16 * 1024 * 1024 * 1024)
}

fn physical_memory_bytes() -> Option<u64> {
    match std::env::consts::OS {
        "macos" => Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|text| text.trim().parse().ok()),
        "linux" => std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("MemTotal:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
            })
            .map(|kilobytes| kilobytes * 1024),
        _ => None,
    }
}

async fn verify_subscription(root: &Path, cfg: &RepotracerConfig, dry_run: bool) -> Result<()> {
    let scout = CliScout::from_config(cfg)?;
    if dry_run {
        plan(&format!(
            "would verify {} with `{}`",
            scout.provider().label(),
            scout.executable().display()
        ));
        return Ok(());
    }
    let installed = scout.executable().is_file() || which::which(scout.executable()).is_ok();
    if !installed {
        bail!(
            "`{}` is not installed or not on PATH",
            scout.executable().display()
        );
    }
    item(true, &format!("{} CLI installed", scout.provider().label()));
    scout.probe(root).await?;
    item(
        true,
        &format!("{} scout verified", scout.provider().label()),
    );
    Ok(())
}

fn select_agents(
    detected: &[agents::AgentInfo],
    yes: bool,
    dry_run: bool,
    interactive: bool,
) -> Result<Vec<String>> {
    let choices: Vec<&agents::AgentInfo> = detected
        .iter()
        .filter(|agent| agent.name != "MCP (generic)")
        .collect();
    if dry_run {
        return Ok(choices.iter().map(|agent| agent.name.clone()).collect());
    }
    if yes {
        return Ok(choices
            .iter()
            .filter(|agent| agent.detected)
            .map(|agent| agent.name.clone())
            .collect());
    }
    if !interactive {
        return Ok(Vec::new());
    }

    println!("Choose agents to configure (comma-separated numbers):");
    for (index, agent) in choices.iter().enumerate() {
        let status = if agent.detected {
            "detected"
        } else {
            "not detected"
        };
        println!("  {}. {:<16} {}", index + 1, agent.name, style("2", status));
    }
    let defaults: Vec<String> = choices
        .iter()
        .enumerate()
        .filter(|(_, agent)| agent.detected)
        .map(|(index, _)| (index + 1).to_string())
        .collect();
    let default = defaults.join(",");
    print!(
        "Selection [{}]: ",
        if default.is_empty() { "none" } else { &default }
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    let input = if input.is_empty() {
        default.as_str()
    } else {
        input
    };
    if input.eq_ignore_ascii_case("none") || input == "0" || input.is_empty() {
        return Ok(Vec::new());
    }

    let mut selected = Vec::new();
    for token in input.split(',').map(str::trim) {
        let index: usize = token
            .parse()
            .with_context(|| format!("invalid agent selection `{token}`"))?;
        let agent = choices
            .get(index.saturating_sub(1))
            .ok_or_else(|| anyhow::anyhow!("agent selection `{index}` is out of range"))?;
        if !selected.contains(&agent.name) {
            selected.push(agent.name.clone());
        }
    }
    Ok(selected)
}

async fn ensure_ollama(
    cfg: &RepotracerConfig,
    yes: bool,
    dry_run: bool,
    interactive: bool,
) -> Result<()> {
    let mut binary = ollama_binary();
    if dry_run {
        if let Some(path) = &binary {
            item(true, &format!("Ollama installed ({})", path.display()));
        } else {
            let command = ollama_install_command(std::env::consts::OS).ok_or_else(|| {
                anyhow::anyhow!("automatic Ollama installation is unsupported on this OS; see https://ollama.com/download")
            })?;
            plan(&format!("would install Ollama: {}", command.display));
        }
        plan("would start and verify the Ollama service");
        plan(&format!("would pull model if missing: {}", cfg.model.model));
        plan("would verify a real model tool call");
        return Ok(());
    }
    if binary.is_none() {
        let command = ollama_install_command(std::env::consts::OS)
            .ok_or_else(|| anyhow::anyhow!("automatic Ollama installation is unsupported on this OS; see https://ollama.com/download"))?;

        if !yes
            && (!interactive || !prompt_yes_no("Ollama is not installed. Install it now?", true)?)
        {
            bail!("Ollama is required for the default local model; install it from https://ollama.com/download or configure another OpenAI-compatible endpoint");
        }
        println!(
            "  {} Installing Ollama with its official installer",
            style("1;34", "→")
        );
        let status = Command::new(&command.program)
            .args(&command.args)
            .status()
            .with_context(|| format!("failed to start `{}`", command.display))?;
        if !status.success() {
            bail!("Ollama installer failed with status {status}");
        }
        binary = ollama_binary();
    }

    let binary = binary.ok_or_else(|| {
        anyhow::anyhow!(
            "Ollama installed but `ollama` was not found. Restart the terminal, then run `repotracer setup` again"
        )
    })?;
    item(true, &format!("Ollama installed ({})", binary.display()));

    if !ollama_reachable(&cfg.model.base_url).await {
        println!("  {} Starting Ollama", style("1;34", "→"));
        start_ollama(&binary)?;
        if !wait_for_ollama(&cfg.model.base_url, Duration::from_secs(20)).await {
            bail!("Ollama did not become ready at {}", cfg.model.base_url);
        }
    }
    item(true, &format!("Ollama reachable ({})", cfg.model.base_url));

    if !model_present(&binary, &cfg.model.model) {
        if !yes
            && (!interactive
                || !prompt_yes_no(&format!("Download model `{}` now?", cfg.model.model), true)?)
        {
            bail!(
                "model `{}` is required; run `ollama pull {}`",
                cfg.model.model,
                cfg.model.model
            );
        }
        println!("  {} Pulling {}", style("1;34", "→"), cfg.model.model);
        let status = Command::new(&binary)
            .args(["pull", &cfg.model.model])
            .status()
            .context("failed to start `ollama pull`")?;
        if !status.success() {
            bail!("could not pull model `{}`", cfg.model.model);
        }
    }
    item(true, &format!("model available ({})", cfg.model.model));

    verify_tool_calling(cfg).await?;
    item(true, "real model tool call verified");
    Ok(())
}

async fn verify_tool_calling(cfg: &RepotracerConfig) -> Result<()> {
    let backend = OpenAiCompatBackend::new(ModelConfig {
        base_url: cfg.model.base_url.clone(),
        model: cfg.model.model.clone(),
        api_key: cfg.model.api_key.clone(),
        timeout_ms: cfg.model.timeout_ms.min(60_000),
        temperature: 0.0,
        max_tokens: Some(128),
    })?;
    let request = || ModelRequest {
        messages: vec![
            ChatMessage::system(
                "You are a repository scout. Use the supplied tools exactly when requested.",
            ),
            ChatMessage::user("Call the Glob tool once with pattern `*`. Do not answer in text."),
        ],
        tools: vec![ToolSpec {
            name: "Glob".into(),
            description: "Find files matching a glob pattern.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "pattern": { "type": "string" } },
                "required": ["pattern"]
            }),
        }],
        temperature: 0.0,
        max_tokens: Some(128),
    };

    for _ in 0..2 {
        let response = backend.complete(request()).await?;
        if response
            .message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| calls.iter().any(|call| call.name == "Glob"))
        {
            return Ok(());
        }
    }
    bail!(
        "model `{}` responded but did not produce an OpenAI-compatible tool call; check the model and Ollama versions",
        cfg.model.model
    )
}

fn uses_local_ollama(cfg: &RepotracerConfig) -> bool {
    cfg.model.backend.eq_ignore_ascii_case("ollama")
        && (cfg.model.base_url.contains("127.0.0.1:11434")
            || cfg.model.base_url.contains("localhost:11434"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallCommand {
    program: String,
    args: Vec<String>,
    display: String,
}

fn ollama_install_command(os: &str) -> Option<InstallCommand> {
    match os {
        "macos" | "linux" => Some(InstallCommand {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "curl -fsSL https://ollama.com/install.sh | sh".into(),
            ],
            display: "curl -fsSL https://ollama.com/install.sh | sh".into(),
        }),
        "windows" => Some(InstallCommand {
            program: "powershell.exe".into(),
            args: vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                "irm https://ollama.com/install.ps1 | iex".into(),
            ],
            display: "irm https://ollama.com/install.ps1 | iex".into(),
        }),
        _ => None,
    }
}

fn ollama_binary() -> Option<PathBuf> {
    if let Ok(path) = which::which("ollama") {
        return Some(path);
    }
    let mut candidates = vec![
        PathBuf::from("/usr/local/bin/ollama"),
        PathBuf::from("/opt/homebrew/bin/ollama"),
        PathBuf::from("/usr/bin/ollama"),
        PathBuf::from("/Applications/Ollama.app/Contents/Resources/ollama"),
    ];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("Programs")
                .join("Ollama")
                .join("ollama.exe"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn start_ollama(binary: &Path) -> Result<()> {
    Command::new(binary)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start `ollama serve`")?;
    Ok(())
}

async fn wait_for_ollama(base_url: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if ollama_reachable(base_url).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

async fn ollama_reachable(base_url: &str) -> bool {
    let base = base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_string();
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    client
        .get(format!("{base}/api/tags"))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn model_present(binary: &Path, model: &str) -> bool {
    Command::new(binary)
        .arg("list")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                line.split_whitespace().next() == Some(model) || line.starts_with(model)
            })
        })
}

pub fn uninstall(root: &Path, yes: bool) -> Result<()> {
    if !yes {
        println!("This removes Repotracer MCP entries, skills, and project routing instructions.");
        println!("Re-run with --yes to confirm.");
        return Ok(());
    }
    for message in agents::uninstall_all(root)? {
        item(true, &message);
    }
    let cfg = config::default_config_path();
    if cfg.exists() {
        std::fs::remove_file(&cfg)?;
        item(true, &format!("removed {}", cfg.display()));
    }
    println!("Uninstall complete. Ollama, downloaded models, and the Repotracer binary were left in place.");
    Ok(())
}

fn prompt_yes_no(question: &str, default: bool) -> Result<bool> {
    print!("{} {} ", style("1;34", "?"), question);
    print!("{}: ", if default { "[Y/n]" } else { "[y/N]" });
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    match input.trim().to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => bail!("expected yes or no"),
    }
}

fn banner() {
    println!("{}", style("1;34", "GREPHOUND SETUP"));
    println!("Small models search. Big models solve.\n");
}

fn section(title: &str) {
    println!("\n{}", style("1", title));
}

fn item(success: bool, text: &str) {
    let (color, mark) = if success {
        ("1;32", "✓")
    } else {
        ("1;33", "○")
    };
    println!("  {} {text}", style(color, mark));
}

fn plan(text: &str) {
    println!("  {} {text}", style("1;34", "·"));
}

fn style(code: &str, text: &str) -> String {
    if io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn os_label(os: &str, arch: &str) -> String {
    match (os, arch) {
        ("macos", "aarch64") => "Apple Silicon".into(),
        ("macos", _) => format!("macOS {arch}"),
        ("linux", _) => format!("Linux {arch}"),
        ("windows", _) => format!("Windows {arch}"),
        _ => format!("{os} {arch}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_commands_match_official_platform_docs() {
        let unix = ollama_install_command("linux").unwrap();
        assert_eq!(unix.program, "sh");
        assert!(unix.display.contains("https://ollama.com/install.sh"));

        let windows = ollama_install_command("windows").unwrap();
        assert_eq!(windows.program, "powershell.exe");
        assert!(windows.display.contains("https://ollama.com/install.ps1"));
        assert!(ollama_install_command("freebsd").is_none());
    }

    #[test]
    fn local_ollama_detection_does_not_capture_custom_endpoints() {
        let mut config = RepotracerConfig::default();
        assert!(uses_local_ollama(&config));
        config.model.base_url = "https://models.example.com/v1".into();
        assert!(!uses_local_ollama(&config));
    }

    #[test]
    fn auto_prefers_local_when_capable_then_subscriptions() {
        assert_eq!(provider_for_auto(true, true, true), ProviderChoice::Ollama);
        assert_eq!(provider_for_auto(false, true, true), ProviderChoice::Codex);
        assert_eq!(
            provider_for_auto(false, false, true),
            ProviderChoice::Claude
        );
        assert_eq!(
            provider_for_auto(false, false, false),
            ProviderChoice::Ollama
        );
    }
}
