use crate::agents;
use crate::config;
use crate::subscription::{is_subscription_backend, CliScout};
use anyhow::{bail, Context, Result};
use repotracer_core::RepoTracerConfig;
use std::io::{self, IsTerminal};
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
        return Ok(());
    }

    // Setup's own checkmarks say what was written, not whether it works, so run
    // the real verification. A failing check does not undo a successful setup —
    // doctor already prints what to fix — so its error must not fail this command.
    if crate::doctor::run(root, &selected_cfg, false)
        .await
        .is_err()
    {
        println!("\nSetup finished. Fix the items above, then: repotracer doctor");
        return Ok(());
    }
    println!("\nRestart Codex, then ask a question that spans several files.");
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
    let choice = crate::select::select(
        "RepoTracer is already configured.",
        &["Update the configuration", "Uninstall RepoTracer"],
        "Use arrow keys, then Enter. Esc to cancel.",
    )?;
    Ok(match choice {
        Some(0) => ExistingChoice::Update,
        Some(1) => ExistingChoice::Uninstall,
        _ => ExistingChoice::Cancel,
    })
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
