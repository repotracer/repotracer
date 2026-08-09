use crate::agents;
use crate::config;
use anyhow::Result;
use grephound_core::GrephoundConfig;
use std::path::Path;
use std::process::Command;

pub async fn run(
    root: &Path,
    cfg_path: &Path,
    cfg: &GrephoundConfig,
    yes: bool,
    dry_run: bool,
) -> Result<()> {
    println!("grephound\n");
    println!("Detecting your setup...\n");

    let is_git = root.join(".git").exists();
    let has_rg = which::which("rg").is_ok();
    let ollama = ollama_reachable(&cfg.model.base_url).await;
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;

    println!("✓ {}", os_label(os, arch));
    if is_git {
        println!("✓ Git repository");
    } else {
        println!("○ Not a git repository (ok)");
    }
    if has_rg {
        println!("✓ ripgrep (rg)");
    } else {
        println!("✗ ripgrep not found — install: https://github.com/BurntSushi/ripgrep");
    }
    if ollama {
        println!("✓ Ollama reachable");
    } else {
        println!("○ Ollama not reachable at {}", cfg.model.base_url);
    }

    let agents = agents::detect();
    for a in &agents {
        if a.name.contains("Claude") || a.name.contains("Codex") || a.name.contains("Cursor") {
            let mark = if a
                .path
                .as_ref()
                .map(|p| Path::new(p).exists())
                .unwrap_or(false)
            {
                "✓"
            } else {
                "○"
            };
            println!("{mark} {} detected", a.name);
        }
    }

    println!("\nInstalling local repository scout...");

    if !dry_run {
        let c = cfg.clone();
        if let Some(parent) = cfg_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        c.save_to(cfg_path)?;
        println!("✓ Config {}", cfg_path.display());
    } else {
        println!("· would write {}", cfg_path.display());
    }

    let bin = agents::current_binary();
    println!("✓ Binary {}", bin.display());

    // Configure agents
    match agents::install_claude(&bin, dry_run) {
        Ok(m) => println!("✓ {m}"),
        Err(e) => println!("○ Claude Code: {e}"),
    }
    match agents::install_codex(&bin, dry_run) {
        Ok(m) => println!("✓ {m}"),
        Err(e) => println!("○ Codex: {e}"),
    }
    match agents::install_cursor(&bin, dry_run) {
        Ok(m) => println!("✓ {m}"),
        Err(e) => println!("○ Cursor: {e}"),
    }

    if ollama {
        let model = &cfg.model.model;
        if model_present(model) {
            println!("✓ Model {model} available");
        } else {
            println!("○ Model `{model}` not found in Ollama");
            if yes || dry_run {
                if dry_run {
                    println!("· would run: ollama pull {model}");
                } else {
                    println!("  Pulling {model} (this may take a while)...");
                    let status = Command::new("ollama").args(["pull", model]).status();
                    match status {
                        Ok(s) if s.success() => println!("✓ Model ready"),
                        _ => println!(
                            "✗ Could not pull model. Try:\n    ollama pull {model}\n  or point grephound at another OpenAI-compatible endpoint."
                        ),
                    }
                }
            } else {
                println!(
                    "  Download required for explorer model `{model}`.\n  Run with --yes to pull, or:\n    ollama pull {model}"
                );
            }
        }
    } else {
        println!(
            "\nLocal inference not detected.\nStart Ollama:\n  ollama serve\n  ollama pull {}\n\nOr set a custom endpoint:\n  grephound config --init\n  # edit base_url / model",
            cfg.model.model
        );
    }

    println!("\nDone.\n");
    println!("Your coding agents can now delegate repository exploration.\n");
    println!("Try:\n  grephound scout \"where is authentication handled?\"\n  grephound doctor\n");
    let _ = root;
    let _ = config::default_config_path;
    Ok(())
}

pub fn uninstall(yes: bool) -> Result<()> {
    if !yes {
        println!("This removes grephound MCP entries from Claude Code / Codex / Cursor.");
        println!("Re-run with --yes to confirm.");
        return Ok(());
    }
    for m in agents::uninstall_all()? {
        println!("✓ {m}");
    }
    let cfg = config::default_config_path();
    if cfg.exists() {
        std::fs::remove_file(&cfg)?;
        println!("✓ removed {}", cfg.display());
    }
    println!("Uninstall complete. Binary left in place — remove manually if desired.");
    Ok(())
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

async fn ollama_reachable(base_url: &str) -> bool {
    let base = base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_string();
    let url = format!("{base}/api/tags");
    let client = match reqwest_client() {
        Some(c) => c,
        None => return false,
    };
    client
        .get(&url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn reqwest_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()
}

fn model_present(model: &str) -> bool {
    let out = Command::new("ollama").args(["list"]).output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.lines()
                .any(|l| l.split_whitespace().next() == Some(model) || l.contains(model))
        }
        _ => false,
    }
}
