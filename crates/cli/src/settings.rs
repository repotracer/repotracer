//! Each parent starts `serve --config <parent-profile>`, avoiding global backend races.
use anyhow::{bail, Context, Result};
use repotracer_core::RepoTracerConfig;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Default, Serialize, Deserialize)]
struct InstallState {
    parents: Vec<String>,
}

pub fn parse_tracer_model(value: &str) -> Result<(&str, &str)> {
    let (provider, model) = value
        .split_once(':')
        .context("use provider:model, for example claude:sonnet or codex:gpt-5.6-luna")?;
    if !matches!(
        provider,
        "codex" | "claude" | "openai" | "openai-compatible"
    ) || model.trim().is_empty()
        || model.chars().any(char::is_control)
    {
        bail!("select codex:MODEL, claude:MODEL, or openai-compatible:MODEL");
    }
    Ok((provider, model.trim()))
}

pub fn profile(base: &Path, parent: &str) -> PathBuf {
    base.with_file_name(format!(
        "{}.{}.toml",
        base.file_stem().unwrap_or_default().to_string_lossy(),
        parent
    ))
}

fn select_provider(cfg: &mut RepoTracerConfig, provider: &str) {
    if matches!(provider, "openai" | "openai-compatible") {
        if cfg.model.is_claude() || crate::subscription::is_subscription_backend(cfg) {
            cfg.model.reasoning_effort.clear();
        }
        cfg.model.backend = "openai-compatible".into();
        cfg.model.executable = None;
        return;
    }
    let changed = cfg.model.backend != format!("{provider}-cli");
    cfg.model.backend = format!("{provider}-cli");
    cfg.model.reasoning_effort = cfg.model.native_reasoning_effort().to_string();
    cfg.model.model = if provider == "claude" {
        "sonnet"
    } else {
        "gpt-5.6-luna"
    }
    .into();
    if changed {
        cfg.model.executable = None;
    }
    // Native providers own authentication. Do not carry a generic endpoint
    // key into a native profile where it would be rejected or misread.
    cfg.model.api_key = None;
}

fn apply_model_choice(
    cfg: &mut RepoTracerConfig,
    choice: &crate::wizard::ParentModelChoice,
) -> Result<()> {
    if choice.model.provider == "openai-compatible" {
        let custom = choice
            .custom
            .as_ref()
            .context("custom API settings are missing; reopen the custom provider form")?;
        if !custom.base_url.starts_with("http://") && !custom.base_url.starts_with("https://") {
            bail!("custom API base URL must use http or https");
        }
        if choice.model.id.trim().is_empty() || choice.model.id.chars().any(char::is_control) {
            bail!("custom API model ID must not be empty or contain control characters");
        }
        cfg.model.backend = "openai-compatible".into();
        cfg.model.executable = None;
        cfg.model.model = choice.model.id.clone();
        cfg.model.base_url = custom.base_url.trim_end_matches('/').into();
        cfg.model.api_key = custom.api_key.clone().filter(|key| !key.is_empty());
        // An empty effort is intentional for a manually entered API model:
        // unsupported provider-specific defaults must not be invented.
        cfg.model.reasoning_effort = choice.reasoning_effort.clone().unwrap_or_default();
    } else {
        select_provider(cfg, &choice.model.provider);
        cfg.model.model = choice.model.id.clone();
        if let Some(effort) = &choice.reasoning_effort {
            cfg.model.reasoning_effort = effort.clone();
        }
    }
    Ok(())
}

fn persist_install_state(path: &Path, state: &InstallState) -> Result<()> {
    if state.parents.is_empty() {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    } else {
        fs::write(path, serde_json::to_vec_pretty(state)?)?;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    base: &Path,
    cfg: &RepoTracerConfig,
    targets: Option<String>,
    codex: Option<String>,
    claude: Option<String>,
    codex_model: Option<String>,
    claude_model: Option<String>,
    dry: bool,
) -> Result<()> {
    let absolute_base = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()?.join(base)
    };
    let base = absolute_base.as_path();
    let state_path = base.with_extension("integrations.json");
    let mut state: InstallState = if state_path.exists() {
        serde_json::from_slice(&fs::read(&state_path)?).context("invalid integration settings")?
    } else {
        InstallState::default()
    };
    let mut installed = InstallState {
        parents: state.parents.clone(),
    };
    let no_options = targets.is_none()
        && codex.is_none()
        && claude.is_none()
        && codex_model.is_none()
        && claude_model.is_none();
    let mut targets = targets;
    let interactive =
        no_options && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let mut edited_parents: Option<Vec<String>> = None;
    let mut removals: Vec<String> = Vec::new();
    let mut interactive_selection: HashMap<String, crate::wizard::ParentModelChoice> =
        HashMap::new();
    if interactive {
        let mut current_by_parent = Vec::new();
        for parent in ["codex", "claude"] {
            let path = profile(base, parent);
            if path.exists() {
                let selected = RepoTracerConfig::load_from(&path)?;
                let provider = selected
                    .model
                    .backend
                    .strip_suffix("-cli")
                    .unwrap_or(&selected.model.backend);
                if matches!(
                    provider,
                    "codex" | "claude" | "openai-compatible" | "openai"
                ) {
                    let choice = crate::model_catalog::ModelChoice {
                        provider: if matches!(provider, "openai") {
                            "openai-compatible".into()
                        } else {
                            provider.into()
                        },
                        id: selected.model.model.clone(),
                        label: format!("{} / {} [current]", provider, selected.model.model),
                    };
                    let custom =
                        (provider == "openai-compatible" || provider == "openai").then(|| {
                            crate::wizard::CustomApiProfile {
                                base_url: selected.model.base_url.clone(),
                                api_key: selected.model.api_key.clone(),
                            }
                        });
                    current_by_parent.push(crate::wizard::CurrentProfile {
                        parent: parent.to_owned(),
                        choice: Some(choice),
                        custom,
                        reasoning_effort: (!selected.model.reasoning_effort.is_empty())
                            .then(|| selected.model.reasoning_effort.clone()),
                    });
                } else {
                    current_by_parent.push(crate::wizard::CurrentProfile {
                        parent: parent.to_owned(),
                        choice: None,
                        custom: None,
                        reasoning_effort: None,
                    });
                }
            } else {
                current_by_parent.push(crate::wizard::CurrentProfile {
                    parent: parent.to_owned(),
                    choice: None,
                    custom: None,
                    reasoning_effort: None,
                });
            }
        }
        let Some(selection) =
            crate::wizard::configure_with_profiles(&installed.parents, &current_by_parent)?
        else {
            return Ok(());
        };
        removals = selection.removed;
        let parents: Vec<String> = selection
            .chosen
            .iter()
            .map(|entry| entry.parent.clone())
            .collect();
        for selection in selection.chosen {
            interactive_selection.insert(selection.parent.clone(), selection);
        }
        // A removal-only run leaves nothing to target, so there is no first
        // parent to name here.
        targets = match parents.len() {
            0 => None,
            2 => Some("both".into()),
            _ => Some(parents[0].clone()),
        };
        edited_parents = Some(parents);
    }
    // Detach before any write, so an unchecked agent is fully removed even if a
    // later install fails.
    if !removals.is_empty() && !dry {
        for parent in &removals {
            remove_parent(base, parent)?;
            installed.parents.retain(|installed| installed != parent);
            state.parents.retain(|installed| installed != parent);
            // Persist each successful detach. If a later parent fails, the
            // state file must not claim that the earlier parent is installed.
            persist_install_state(&state_path, &installed)?;
            println!("Removed the RepoTracer integration from {parent}.");
        }
    }
    let requested: Vec<&str> = match targets.as_deref() {
        Some("both") => vec!["codex", "claude"],
        Some("codex") => vec!["codex"],
        Some("claude") => vec!["claude"],
        None => vec![],
        _ => bail!("unknown parent"),
    };
    if edited_parents.is_none() && !no_options {
        let mut edited: Vec<String> = requested.iter().map(|parent| (*parent).into()).collect();
        for (parent, changed) in [
            ("codex", codex.is_some() || codex_model.is_some()),
            ("claude", claude.is_some() || claude_model.is_some()),
        ] {
            if changed && !edited.iter().any(|value| value == parent) {
                edited.push(parent.into());
            }
        }
        edited_parents = Some(edited);
    }
    for p in requested {
        if !state.parents.iter().any(|x| x == p) {
            state.parents.push(p.into());
        }
    }
    if (codex.is_some() || codex_model.is_some()) && !state.parents.iter().any(|p| p == "codex") {
        state.parents.push("codex".into());
    }
    if (claude.is_some() || claude_model.is_some()) && !state.parents.iter().any(|p| p == "claude")
    {
        state.parents.push("claude".into());
    }
    if state.parents.is_empty() {
        // After a removal-only run the empty state is the requested result, not
        // a missing configuration to complain about.
        if removals.is_empty() {
            println!("No parent profiles. Use settings --agents codex|claude|both.");
        }
        return Ok(());
    }
    let mut pending = Vec::new();
    for parent in &state.parents {
        if !matches!(parent.as_str(), "codex" | "claude") {
            bail!("invalid installed parent {parent}");
        }
        if edited_parents
            .as_ref()
            .is_some_and(|parents| !parents.contains(parent))
        {
            continue;
        }
        let path = profile(base, parent);
        let mut selected = if path.exists() {
            RepoTracerConfig::load_from(&path)?
        } else {
            let mut c = cfg.clone();
            select_provider(&mut c, parent);
            c
        };
        let (provider, model) = if parent == "codex" {
            (&codex, &codex_model)
        } else {
            (&claude, &claude_model)
        };
        if let Some(choice) = interactive_selection.get(parent) {
            apply_model_choice(&mut selected, choice)?;
        } else {
            if let Some(provider) = provider {
                select_provider(&mut selected, provider);
            }
            if let Some(model) = model {
                selected.model.model = model.clone();
            }
        }
        if selected.model.backend == "claude-cli" {
            crate::claude::ClaudeScout::new(&selected)?;
        } else if selected.model.backend == "openai-compatible" {
            if selected.model.base_url.trim().is_empty() {
                bail!("custom API base URL is required");
            }
        } else {
            crate::subscription::CliScout::from_config(&selected)?;
        }
        println!(
            "{parent} parent -> {} / {} ({})",
            selected.model.backend,
            selected.model.model,
            path.display()
        );
        pending.push((parent.clone(), path, selected));
    }
    if dry || (no_options && !interactive) {
        if dry {
            println!("Dry run. Nothing was changed.");
        } else {
            println!("Nothing was changed. Run `repotracer settings` in a terminal to edit, or supply explicit settings flags.");
        }
        return Ok(());
    }
    // Validate native commands before any writes. No login or model probes.
    for (parent, _, _) in &pending {
        if parent == "claude" && which::which("claude").is_err() {
            bail!("Install Claude Code before configuring its MCP integration");
        }
    }
    for (parent, path, selected) in pending {
        let old_profile = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let managed = installed.parents.contains(&parent);
        selected.save_to(&path)?;
        if let Err(error) = install_parent(&parent, &path, managed) {
            if let Some(bytes) = old_profile {
                fs::write(&path, bytes).context("restore previous scout profile")?;
                if managed {
                    // If replacement removed the native entry, restore it. An
                    // existing entry is left untouched by this non-replacing call.
                    let _ = install_parent(&parent, &path, false);
                }
            } else {
                fs::remove_file(&path).context("remove incomplete new profile")?;
            }
            return Err(error);
        }
        if !installed.parents.contains(&parent) {
            installed.parents.push(parent);
        }
        // Preserve successful installations if a later parent fails.
        persist_install_state(&state_path, &installed)?;
    }
    println!(
        "Saved. Restart the configured parent agents. Use repotracer settings to change mappings."
    );
    Ok(())
}

fn install_parent(parent: &str, profile: &Path, managed: bool) -> Result<()> {
    let absolute_profile = if profile.is_absolute() {
        profile.to_path_buf()
    } else {
        std::env::current_dir()?.join(profile)
    };
    let profile = absolute_profile.as_path();
    let binary = crate::agents::current_binary();
    if parent == "codex" {
        let args = vec![
            "serve".to_string(),
            "--config".into(),
            profile.display().to_string(),
        ];
        crate::agents::install_codex_with_args(&binary, &args, false)?;
    } else {
        // Let Claude own its user configuration format rather than rewriting ~/.claude.json.
        let entry = serde_json::json!({"type":"stdio", "command":binary, "args":["serve", "--config", profile]}).to_string();
        let add = || {
            Command::new("claude")
                .args(["mcp", "add-json", "--scope", "user", "repotracer"])
                .arg(&entry)
                .output()
        };
        let mut result = add()?;
        if !result.status.success()
            && managed
            && String::from_utf8_lossy(&result.stderr)
                .contains("MCP server repotracer already exists in user config")
        {
            let removed = Command::new("claude")
                .args(["mcp", "remove", "--scope", "user", "repotracer"])
                .output()?;
            if !removed.status.success() {
                bail!(
                    "Could not replace the managed Claude MCP entry; no new registration attempted"
                );
            }
            result = add()?;
        }
        if !result.status.success() {
            bail!("Claude MCP registration failed. An untracked existing entry is never overwritten; check `claude mcp` settings and retry");
        }
        let instructions = crate::agents::claude_instructions_path()
            .context("no Claude configuration directory")?;
        crate::agents::upsert_managed_block(&instructions, crate::agents::ROUTING_INSTRUCTIONS)?;
    }
    Ok(())
}

pub fn refresh(base: &Path) -> Result<bool> {
    let path = base.with_extension("integrations.json");
    if !path.exists() {
        return Ok(false);
    }
    let state: InstallState = serde_json::from_slice(&fs::read(path)?)?;
    for parent in state.parents {
        if !matches!(parent.as_str(), "codex" | "claude") {
            bail!("invalid parent profile");
        }
        install_parent(&parent, &profile(base, &parent), true)?;
    }
    Ok(true)
}

/// Detach one parent: its native registration, then the generated profile.
/// The profile is removed last so a failed registration removal leaves a
/// working install rather than a config pointing at a missing file.
fn remove_parent(base: &Path, parent: &str) -> Result<()> {
    match parent {
        "claude" => {
            // If Claude Code is no longer installed, its native registration
            // cannot be present. Continue removing RepoTracer-owned files.
            if which::which("claude").is_ok() {
                let output = Command::new("claude")
                    .args(["mcp", "remove", "--scope", "user", "repotracer"])
                    .output()?;
                if !output.status.success() {
                    return Err(crate::session::attach_stderr(
                        anyhow::anyhow!("Claude MCP removal failed; profiles retained"),
                        output.stderr,
                    ));
                }
            }
            if let Some(instructions) = crate::agents::claude_instructions_path() {
                let mut messages = Vec::new();
                crate::agents::remove_managed_instructions(&instructions, &mut messages)?;
                for message in messages {
                    println!("{message}");
                }
            }
        }
        "codex" => {
            for message in crate::agents::uninstall_codex(base)? {
                println!("{message}");
            }
        }
        _ => bail!("invalid parent profile"),
    }
    let profile = profile(base, parent);
    if profile.exists() {
        fs::remove_file(&profile)?;
        println!("Removed generated profile {}", profile.display());
    }
    Ok(())
}

pub fn uninstall(base: &Path) -> Result<()> {
    let path = base.with_extension("integrations.json");
    if !path.exists() {
        return Ok(());
    }
    let mut state: InstallState = serde_json::from_slice(&fs::read(&path)?)?;
    for parent in state.parents.clone() {
        remove_parent(base, &parent)?;
        state.parents.retain(|installed| installed != &parent);
        // Keep partial uninstall recoverable when a later parent fails.
        persist_install_state(&path, &state)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracer_choice_includes_an_explicit_subscription() {
        assert_eq!(
            parse_tracer_model("claude:sonnet").unwrap(),
            ("claude", "sonnet")
        );
        assert_eq!(parse_tracer_model("codex:o3").unwrap(), ("codex", "o3"));
        for invalid in ["sonnet", "other:model", "claude:", "codex:a\nb"] {
            assert!(parse_tracer_model(invalid).is_err());
        }
    }
    #[test]
    fn profiles_are_independent_and_provider_switch_resets_model() {
        let base = Path::new("/tmp/custom.toml");
        assert_ne!(profile(base, "codex"), profile(base, "claude"));
        let mut c = RepoTracerConfig::default();
        select_provider(&mut c, "claude");
        assert_eq!(c.model.model, "sonnet");
        select_provider(&mut c, "codex");
        assert_eq!(c.model.backend, "codex-cli");
    }

    #[test]
    fn selecting_the_same_provider_preserves_custom_executable_and_budgets() {
        let mut cfg = RepoTracerConfig::default();
        cfg.model.executable = Some("/custom/codex".into());
        cfg.session.max_thread_turns = 7;
        select_provider(&mut cfg, "codex");
        assert_eq!(cfg.model.executable.as_deref(), Some("/custom/codex"));
        assert_eq!(cfg.session.max_thread_turns, 7);
        select_provider(&mut cfg, "claude");
        assert_eq!(cfg.model.executable, None);
    }

    #[test]
    fn switching_to_an_api_does_not_carry_native_effort() {
        let mut cfg = RepoTracerConfig::default();
        select_provider(&mut cfg, "claude");
        assert_eq!(cfg.model.reasoning_effort, "medium");
        cfg.model.reasoning_effort = "high".into();
        select_provider(&mut cfg, "openai-compatible");
        assert!(cfg.model.reasoning_effort.is_empty());
        cfg.model.reasoning_effort = "low".into();
        select_provider(&mut cfg, "openai-compatible");
        assert_eq!(cfg.model.reasoning_effort, "low");
        cfg.model.reasoning_effort.clear();
        select_provider(&mut cfg, "codex");
        assert_eq!(cfg.model.reasoning_effort, "medium");
    }

    #[test]
    fn custom_choice_round_trips_endpoint_key_arbitrary_model_and_optional_effort() {
        let mut cfg = RepoTracerConfig::default();
        let choice = crate::wizard::ParentModelChoice {
            parent: "codex".into(),
            model: crate::model_catalog::ModelChoice {
                provider: "openai-compatible".into(),
                id: "vendor/reasoner.v9".into(),
                label: "custom".into(),
            },
            custom: Some(crate::wizard::CustomApiProfile {
                base_url: "https://gateway.example/v1".into(),
                api_key: Some("secret".into()),
            }),
            reasoning_effort: None,
        };
        apply_model_choice(&mut cfg, &choice).unwrap();
        assert_eq!(cfg.model.backend, "openai-compatible");
        assert_eq!(cfg.model.model, "vendor/reasoner.v9");
        assert_eq!(cfg.model.base_url, "https://gateway.example/v1");
        assert_eq!(cfg.model.api_key.as_deref(), Some("secret"));
        assert!(cfg.model.reasoning_effort.is_empty());
    }

    #[test]
    fn removal_state_is_persisted_after_each_successful_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.integrations.json");
        let mut state = InstallState {
            parents: vec!["codex".into(), "claude".into()],
        };
        persist_install_state(&path, &state).unwrap();

        state.parents.retain(|parent| parent != "codex");
        persist_install_state(&path, &state).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap(),
            serde_json::json!({"parents": ["claude"]})
        );

        state.parents.retain(|parent| parent != "claude");
        persist_install_state(&path, &state).unwrap();
        assert!(!path.exists());
    }
}
