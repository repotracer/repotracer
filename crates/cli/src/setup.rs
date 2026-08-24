use crate::agents;
use crate::config;
use crate::subscription::{is_subscription_backend, CliScout};
use anyhow::{bail, Context, Result};
use repotracer_core::RepoTracerConfig;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

pub async fn run(
    root: &Path,
    cfg_path: &Path,
    cfg: &RepoTracerConfig,
    dry_run: bool,
) -> Result<()> {
    banner();
    section("1 / 4  Environment");
    item(
        true,
        &os_label(std::env::consts::OS, std::env::consts::ARCH),
    );
    item(root.join(".git").exists(), "Git repository");
    item(
        which::which("rg").is_ok(),
        "ripgrep (optional with GPT scouts)",
    );

    let detected = agents::detect(root);
    for agent in &detected {
        if agent.configured {
            item(
                true,
                &format!("{} detected — already configured", agent.name),
            );
        } else {
            item(agent.detected, &format!("{} detected", agent.name));
        }
    }
    let already_installed = detected.iter().any(|agent| agent.configured);

    // An existing install is the one case where setup is not the obvious action,
    // so offer the alternative instead of only mentioning it after the fact.
    if already_installed && !dry_run {
        match prompt_existing_install()? {
            ExistingChoice::Update => {}
            ExistingChoice::Uninstall => return uninstall(root, true),
            ExistingChoice::Cancel => {
                println!("\nCancelled. Nothing was changed.");
                return Ok(());
            }
        }
    }

    section("2 / 4  GPT scout");
    let selected_cfg = gpt_config(cfg)?;
    if is_subscription_backend(&selected_cfg) {
        verify_codex(&selected_cfg, dry_run)?;
    } else if dry_run {
        plan(&format!(
            "would use {} at {}",
            selected_cfg.model.model, selected_cfg.model.base_url
        ));
    } else {
        item(
            true,
            &format!(
                "GPT endpoint configured ({} at {})",
                selected_cfg.model.model, selected_cfg.model.base_url
            ),
        );
    }

    section("3 / 4  Codex integration");
    let binary = agents::current_binary();
    match agents::install_codex(&binary, dry_run) {
        Ok(message) => item(true, &message),
        Err(error) => bail!("Codex configuration failed: {error}"),
    }

    section("4 / 4  RepoTracer configuration");
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
        if already_installed {
            println!("\nRepoTracer is already configured. Re-running setup refreshes it.");
            println!("Remove it instead:  repotracer uninstall --yes");
        }
    } else if already_installed {
        println!("{}", style("1;32", "UPDATED — configuration refreshed"));
        println!("\nRestart configured agents, then ask a multi-file repository question.");
        println!("Verify any time:  repotracer doctor");
        println!("Remove it again:  repotracer uninstall --yes");
    } else {
        println!(
            "{}",
            style("1;32", "READY — small models search, big models solve")
        );
        println!("\nRestart configured agents, then ask a multi-file repository question.");
        println!("Verify any time:  repotracer doctor");
        println!("Remove it again:  repotracer uninstall --yes");
    }
    Ok(())
}

enum ExistingChoice {
    Update,
    Uninstall,
    Cancel,
}

/// Ask what to do about an existing install. Non-interactive callers (CI, a piped
/// `npx` run) must never block, so they get the safe default of refreshing.
fn prompt_existing_install() -> Result<ExistingChoice> {
    if !io::stdin().is_terminal() {
        return Ok(ExistingChoice::Update);
    }

    println!("\n{}", style("1;33", "RepoTracer is already configured."));
    println!("  {} Update the configuration (default)", style("1", "1"));
    println!("  {} Uninstall RepoTracer", style("1", "2"));
    println!("  {} Cancel", style("1", "3"));
    print!("\nChoose [1]: ");
    io::stdout().flush().ok();

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return Ok(ExistingChoice::Update);
    }
    Ok(parse_existing_choice(&answer))
}

fn parse_existing_choice(answer: &str) -> ExistingChoice {
    match answer.trim() {
        "2" | "u" | "uninstall" => ExistingChoice::Uninstall,
        "3" | "c" | "cancel" | "q" => ExistingChoice::Cancel,
        _ => ExistingChoice::Update,
    }
}

fn gpt_config(cfg: &RepoTracerConfig) -> Result<RepoTracerConfig> {
    let mut selected = cfg.clone();
    match selected.model.backend.to_ascii_lowercase().as_str() {
        "codex" | "codex-cli" => {
            selected.model.backend = "codex-cli".into();
            if !selected.model.model.starts_with("gpt-") {
                selected.model.model = "gpt-5.6-luna".into();
            }
            selected.model.timeout_ms = selected.model.timeout_ms.max(120_000);
        }
        "openai" | "openai-compatible" => {
            if !selected.model.model.starts_with("gpt-") {
                bail!(
                    "unsupported model `{}`; RepoTracer currently supports GPT models",
                    selected.model.model
                );
            }
            selected.model.backend = "openai-compatible".into();
        }
        _ => {
            selected.model.backend = "codex-cli".into();
            selected.model.executable = None;
            selected.model.model = "gpt-5.6-luna".into();
            selected.model.api_key = None;
            selected.model.timeout_ms = selected.model.timeout_ms.max(120_000);
        }
    }
    Ok(selected)
}

fn verify_codex(cfg: &RepoTracerConfig, dry_run: bool) -> Result<()> {
    let scout = CliScout::from_config(cfg)?;
    if dry_run {
        plan(&format!(
            "would use {} with `{}` and its existing login/provider",
            cfg.model.model,
            scout.executable().display()
        ));
        return Ok(());
    }
    if !scout.executable().is_file() && which::which(scout.executable()).is_err() {
        bail!(
            "Codex CLI is required for zero-config GPT scouting; install Codex, sign in, then rerun `repotracer setup`"
        );
    }
    let output = Command::new(scout.executable())
        .args(["login", "status"])
        .output()
        .context("could not check Codex login")?;
    if !output.status.success() {
        bail!("Codex is not signed in; run `codex login`, then rerun `repotracer setup`");
    }
    item(
        true,
        &format!("{} ready ({})", scout.label(), cfg.model.model),
    );
    Ok(())
}

pub fn uninstall(root: &Path, yes: bool) -> Result<()> {
    if !yes {
        println!("This removes RepoTracer MCP entries, skills, and project routing instructions.");
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
    println!("Uninstall complete. Provider logins and the RepoTracer binary were left in place.");
    Ok(())
}

fn banner() {
    println!("{}", style("1;34", "REPOTRACER SETUP"));
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
    fn existing_install_choice_defaults_to_update() {
        // Anything unrecognised, including a bare Enter, must refresh rather than
        // silently uninstall.
        for answer in ["", "1", "x", "\n", "update"] {
            assert!(matches!(
                parse_existing_choice(answer),
                ExistingChoice::Update
            ));
        }
        for answer in ["2", "u", "uninstall"] {
            assert!(matches!(
                parse_existing_choice(answer),
                ExistingChoice::Uninstall
            ));
        }
        for answer in ["3", "c", "cancel", "q"] {
            assert!(matches!(
                parse_existing_choice(answer),
                ExistingChoice::Cancel
            ));
        }
    }

    #[test]
    fn setup_normalizes_to_luna_and_rejects_non_gpt_endpoints() {
        let mut legacy = RepoTracerConfig::default();
        legacy.model.backend = "ollama".into();
        legacy.model.model = "fastcontext".into();
        let selected = gpt_config(&legacy).unwrap();
        assert_eq!(selected.model.backend, "codex-cli");
        assert_eq!(selected.model.model, "gpt-5.6-luna");

        legacy.model.backend = "openai-compatible".into();
        assert!(gpt_config(&legacy).is_err());
    }
}
