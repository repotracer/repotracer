use crate::agents;
use crate::config;
use crate::subscription::{is_subscription_backend, CliScout};
use anyhow::{bail, Result};
use repotracer_core::RepoTracerConfig;
use std::io::{self, IsTerminal};
use std::path::Path;

pub async fn run(
    root: &Path,
    cfg_path: &Path,
    cfg: &RepoTracerConfig,
    dry_run: bool,
) -> Result<()> {
    // Setup is a global install: it writes to the Codex home and ~/.repotracer.
    // Nothing here depends on the current directory, so nothing here inspects it.
    let detected = agents::detect(root);
    let already_installed = detected.iter().any(|agent| agent.configured);

    if already_installed && !dry_run {
        match prompt_existing_install()? {
            ExistingChoice::Update => {}
            ExistingChoice::Uninstall => return uninstall(root, true),
            ExistingChoice::Cancel => {
                println!("Cancelled. Nothing was changed.");
                return Ok(());
            }
        }
    }

    let selected_cfg = gpt_config(cfg)?;
    if is_subscription_backend(&selected_cfg) {
        verify_codex_available(&selected_cfg, dry_run)?;
    }

    let binary = agents::current_binary();
    let codex_message = agents::install_codex(&binary, dry_run)
        .map_err(|error| anyhow::anyhow!("Codex configuration failed: {error}"))?;

    if dry_run {
        // A preview's whole job is showing what would be configured.
        println!("{}", style("1;34", "Dry run. Nothing was changed."));
        item(
            true,
            &format!(
                "scout: {} via {}",
                selected_cfg.model.model, selected_cfg.model.base_url
            ),
        );
        item(true, &codex_message);
        item(true, &format!("would write {}", cfg_path.display()));
        return Ok(());
    }

    if let Some(parent) = cfg_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    selected_cfg.save_to(cfg_path)?;

    item(true, "Codex found");
    item(true, &codex_message);
    item(true, &format!("installed at {}", binary.display()));
    if selected_cfg.updates.automatic {
        item(true, "automatic updates on");
    } else {
        item(true, "automatic updates off");
    }
    println!();
    println!(
        "{}",
        style("1;32", "Ready. Restart Codex and keep prompting normally.")
    );
    if selected_cfg.updates.automatic {
        println!("RepoTracer will update automatically. Restart Codex after an update for it to take effect.");
        println!("To disable it, set `updates.automatic = false` in ~/.repotracer/config.toml or `REPOTRACER_NO_UPDATE=1`.");
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
        }
    }
    Ok(selected)
}

fn verify_codex_available(cfg: &RepoTracerConfig, dry_run: bool) -> Result<()> {
    let scout = CliScout::from_config(cfg)?;
    if dry_run {
        plan(&format!(
            "would use {} with `{}` and its login/provider at scout time",
            cfg.model.model,
            scout.executable().display()
        ));
        return Ok(());
    }
    if !scout.executable().is_file() && which::which(scout.executable()).is_err() {
        bail!(
            "Codex CLI is required for zero-config GPT scouting; install Codex, then rerun `repotracer setup`"
        );
    }
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
        assert_eq!(selected.model.service_tier, "fast");

        legacy.updates.automatic = false;
        assert!(!gpt_config(&legacy).unwrap().updates.automatic);

        legacy.updates.automatic = false;
        assert!(!gpt_config(&legacy).unwrap().updates.automatic);

        legacy.model.backend = "openai-compatible".into();
        assert!(gpt_config(&legacy).is_err());
    }
}
