use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MANAGED_START: &str = "<!-- repotracer:start -->";
const MANAGED_END: &str = "<!-- repotracer:end -->";

pub const ROUTING_SKILL: &str = r#"---
name: repotracer-scout
description: Delegate unfamiliar or multi-file repository exploration to Repotracer. Use for tracing behavior across modules, locating an implementation and its tests, understanding an unfamiliar subsystem, or finding change impact. Skip for trivial known-file reads or when the relevant code is already in context.
---

# Repotracer repository scout

Use the `repo_scout` MCP tool before manual Read/Grep/Glob chains when a task requires repository exploration across unknown or multiple files.

Give `repo_scout` one precise semantic question naming the behavior or flow to trace. Repotracer selects its configured read-only local, subscription, or custom-endpoint backend internally. Use its validated file:line citations to read only the returned locations, then solve the task with normal coding tools.

Do not invoke Repotracer's backend CLI directly. Do not use Repotracer when the exact relevant file or symbol is already known and one targeted read is enough, or when no repository exploration is needed.
"#;

const ROUTING_INSTRUCTIONS: &str = "Use the Repotracer `repo_scout` MCP tool before manual Read/Grep/Glob chains when a coding task requires understanding unfamiliar code, tracing behavior across multiple files, locating implementation plus tests, or assessing multi-module change impact. Ask one precise semantic repository question, then read only the validated file:line citations it returns. Skip Repotracer for trivial known-file reads or when the relevant code is already in context.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub detected: bool,
    pub configured: bool,
    pub path: Option<String>,
}

pub fn detect(root: &Path) -> Vec<AgentInfo> {
    vec![
        detect_json_agent("Claude Code", "claude", claude_config_path()),
        detect_codex(),
        detect_json_agent("Cursor", "cursor", cursor_mcp_path()),
        detect_json_agent("GitHub Copilot", "copilot", Some(copilot_mcp_path(root))),
        AgentInfo {
            name: "MCP (generic)".into(),
            detected: true,
            configured: true,
            path: Some("repotracer serve".into()),
        },
    ]
}

fn detect_json_agent(name: &str, command: &str, path: Option<PathBuf>) -> AgentInfo {
    let detected = which::which(command).is_ok()
        || path
            .as_ref()
            .and_then(|p| p.parent())
            .is_some_and(Path::exists);
    let configured = path
        .as_ref()
        .is_some_and(|p| p.exists() && file_contains_repotracer(p));
    AgentInfo {
        name: name.into(),
        detected,
        configured,
        path: path.map(|p| p.display().to_string()),
    }
}

fn detect_codex() -> AgentInfo {
    let path = codex_config_path();
    let detected = which::which("codex").is_ok()
        || path
            .as_ref()
            .and_then(|p| p.parent())
            .is_some_and(Path::exists);
    let configured = path
        .as_ref()
        .is_some_and(|p| p.exists() && file_contains_repotracer(p));
    AgentInfo {
        name: "Codex".into(),
        detected,
        configured,
        path: path.map(|p| p.display().to_string()),
    }
}

fn file_contains_repotracer(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|s| s.contains("repotracer"))
        .unwrap_or(false)
}

pub fn claude_config_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude.json"))
}

pub fn codex_config_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".codex").join("config.toml"))
}

pub fn cursor_mcp_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".cursor").join("mcp.json"))
}

pub fn copilot_mcp_path(root: &Path) -> PathBuf {
    root.join(".github").join("copilot").join("mcp.json")
}

fn claude_skill_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .join(".claude")
        .join("skills")
        .join("repotracer-scout")
        .join("SKILL.md"))
}

fn codex_skill_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .join(".codex")
        .join("skills")
        .join("repotracer-scout")
        .join("SKILL.md"))
}

fn cursor_rule_path(root: &Path) -> PathBuf {
    root.join(".cursor")
        .join("rules")
        .join("repotracer-scout.mdc")
}

fn copilot_instructions_path(root: &Path) -> PathBuf {
    root.join(".github").join("copilot-instructions.md")
}

pub fn repotracer_mcp_entry(binary: &Path) -> Value {
    json!({
        "command": binary.display().to_string(),
        "args": ["serve"],
    })
}

pub fn install_claude(binary: &Path, dry_run: bool) -> anyhow::Result<String> {
    let config = claude_config_path().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let skill = claude_skill_path()?;
    if !dry_run {
        install_json_mcp(&config, "mcpServers", binary)?;
        write_managed_file(&skill, ROUTING_SKILL)?;
    }
    Ok(format!(
        "{} Claude Code MCP + skill ({})",
        action(dry_run),
        config.display()
    ))
}

pub fn install_codex(binary: &Path, dry_run: bool) -> anyhow::Result<String> {
    let config = codex_config_path().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let skill = codex_skill_path()?;
    if !dry_run {
        install_codex_config(&config, binary)?;
        write_managed_file(&skill, ROUTING_SKILL)?;
    }
    Ok(format!(
        "{} Codex MCP + skill ({})",
        action(dry_run),
        config.display()
    ))
}

pub fn install_cursor(binary: &Path, root: &Path, dry_run: bool) -> anyhow::Result<String> {
    let config = cursor_mcp_path().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let rule = cursor_rule_path(root);
    if !dry_run {
        install_json_mcp(&config, "mcpServers", binary)?;
        write_managed_file(
            &rule,
            &format!(
                "---\ndescription: Route multi-file repository exploration through Repotracer\nalwaysApply: true\n---\n\n{ROUTING_INSTRUCTIONS}\n"
            ),
        )?;
    }
    Ok(format!(
        "{} Cursor MCP + project rule ({})",
        action(dry_run),
        config.display()
    ))
}

pub fn install_copilot(binary: &Path, root: &Path, dry_run: bool) -> anyhow::Result<String> {
    let config = copilot_mcp_path(root);
    let instructions = copilot_instructions_path(root);
    if !dry_run {
        install_json_mcp(&config, "servers", binary)?;
        upsert_managed_block(&instructions, ROUTING_INSTRUCTIONS)?;
    }
    Ok(format!(
        "{} GitHub Copilot MCP + project instructions ({})",
        action(dry_run),
        config.display()
    ))
}

fn action(dry_run: bool) -> &'static str {
    if dry_run {
        "would configure"
    } else {
        "configured"
    }
}

fn install_json_mcp(path: &Path, key: &str, binary: &Path) -> anyhow::Result<()> {
    ensure_parent(path)?;
    backup_file(path)?;
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(path)?)?
    } else {
        json!({})
    };
    let map = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))?;
    let servers = map
        .entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("`{key}` in {} must be an object", path.display()))?;
    servers.insert("repotracer".into(), repotracer_mcp_entry(binary));
    fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(())
}

fn install_codex_config(path: &Path, binary: &Path) -> anyhow::Result<()> {
    ensure_parent(path)?;
    backup_file(path)?;
    let text = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    let command = serde_json::to_string(&binary.display().to_string())?;
    let block = format!("[mcp_servers.repotracer]\ncommand = {command}\nargs = [\"serve\"]\n");
    fs::write(
        path,
        replace_toml_section(&text, "[mcp_servers.repotracer]", &block),
    )?;
    Ok(())
}

fn replace_toml_section(text: &str, header: &str, block: &str) -> String {
    if let Some(start) = text.find(header) {
        let rest = &text[start + header.len()..];
        let end = rest
            .find("\n[")
            .map(|offset| start + header.len() + offset + 1)
            .unwrap_or(text.len());
        let mut out = String::with_capacity(text.len() + block.len());
        out.push_str(&text[..start]);
        out.push_str(block);
        out.push_str(&text[end..]);
        out
    } else {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(block);
        out
    }
}

fn remove_toml_section(text: &str, header: &str) -> String {
    if !text.contains(header) {
        return text.to_string();
    }
    replace_toml_section(text, header, "")
}

fn write_managed_file(path: &Path, content: &str) -> anyhow::Result<()> {
    ensure_parent(path)?;
    backup_file(path)?;
    fs::write(path, content)?;
    Ok(())
}

fn upsert_managed_block(path: &Path, content: &str) -> anyhow::Result<()> {
    ensure_parent(path)?;
    backup_file(path)?;
    let existing = fs::read_to_string(path).unwrap_or_default();
    let block = format!("{MANAGED_START}\n{content}\n{MANAGED_END}");
    let updated = replace_managed_block(&existing, &block);
    fs::write(path, updated)?;
    Ok(())
}

fn replace_managed_block(existing: &str, block: &str) -> String {
    if let Some(start) = existing.find(MANAGED_START) {
        if let Some(relative_end) = existing[start..].find(MANAGED_END) {
            let end = start + relative_end + MANAGED_END.len();
            return format!("{}{}{}", &existing[..start], block, &existing[end..]);
        }
    }
    if existing.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{}\n\n{block}\n", existing.trim_end())
    }
}

fn remove_managed_block(existing: &str) -> String {
    if let Some(start) = existing.find(MANAGED_START) {
        if let Some(relative_end) = existing[start..].find(MANAGED_END) {
            let end = start + relative_end + MANAGED_END.len();
            return format!("{}{}", &existing[..start], &existing[end..])
                .trim()
                .to_string();
        }
    }
    existing.to_string()
}

pub fn uninstall_all(root: &Path) -> anyhow::Result<Vec<String>> {
    let mut messages = Vec::new();
    remove_json_mcp(claude_config_path().as_deref(), "mcpServers", &mut messages)?;
    remove_json_mcp(cursor_mcp_path().as_deref(), "mcpServers", &mut messages)?;
    remove_json_mcp(Some(&copilot_mcp_path(root)), "servers", &mut messages)?;

    if let Some(path) = codex_config_path().filter(|p| p.exists()) {
        backup_file(&path)?;
        let updated = remove_toml_section(&fs::read_to_string(&path)?, "[mcp_servers.repotracer]");
        fs::write(&path, updated)?;
        messages.push(format!("removed Repotracer from {}", path.display()));
    }

    for path in [
        claude_skill_path().ok(),
        codex_skill_path().ok(),
        Some(cursor_rule_path(root)),
    ]
    .into_iter()
    .flatten()
    {
        if path.exists() {
            fs::remove_file(&path)?;
            messages.push(format!("removed {}", path.display()));
        }
    }

    let instructions = copilot_instructions_path(root);
    if instructions.exists() {
        let updated = remove_managed_block(&fs::read_to_string(&instructions)?);
        if updated.is_empty() {
            fs::remove_file(&instructions)?;
        } else {
            fs::write(&instructions, updated + "\n")?;
        }
        messages.push(format!(
            "removed Repotracer instructions from {}",
            instructions.display()
        ));
    }

    Ok(messages)
}

fn remove_json_mcp(
    path: Option<&Path>,
    key: &str,
    messages: &mut Vec<String>,
) -> anyhow::Result<()> {
    let Some(path) = path.filter(|p| p.exists()) else {
        return Ok(());
    };
    let mut root: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let removed = root
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.remove("repotracer"))
        .is_some();
    if removed {
        backup_file(path)?;
        fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
        messages.push(format!("removed Repotracer from {}", path.display()));
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn backup_file(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        fs::copy(
            path,
            PathBuf::from(format!("{}.bak", path.to_string_lossy())),
        )?;
    }
    Ok(())
}

pub fn current_binary() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("repotracer"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn json_install_preserves_other_servers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        fs::write(&path, r#"{"mcpServers":{"other":{"command":"other"}}}"#).unwrap();
        install_json_mcp(
            &path,
            "mcpServers",
            Path::new("C:\\Repotracer\\repotracer.exe"),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["mcpServers"]["other"]["command"], "other");
        assert_eq!(
            value["mcpServers"]["repotracer"]["command"],
            "C:\\Repotracer\\repotracer.exe"
        );
    }

    #[test]
    fn codex_section_update_is_idempotent() {
        let original = "model = \"gpt\"\n\n[mcp_servers.repotracer]\ncommand = \"old\"\nargs = [\"serve\"]\n\n[other]\nvalue = 1\n";
        let block = "[mcp_servers.repotracer]\ncommand = \"new\"\nargs = [\"serve\"]\n";
        let once = replace_toml_section(original, "[mcp_servers.repotracer]", block);
        let twice = replace_toml_section(&once, "[mcp_servers.repotracer]", block);
        assert_eq!(once, twice);
        assert_eq!(once.matches("[mcp_servers.repotracer]").count(), 1);
        assert!(once.contains("[other]"));
    }

    #[test]
    fn managed_instructions_preserve_user_content() {
        let first = replace_managed_block(
            "# Existing\n",
            "<!-- repotracer:start -->\none\n<!-- repotracer:end -->",
        );
        let second = replace_managed_block(
            &first,
            "<!-- repotracer:start -->\ntwo\n<!-- repotracer:end -->",
        );
        assert!(second.contains("# Existing"));
        assert!(!second.contains("\none\n"));
        assert_eq!(second.matches(MANAGED_START).count(), 1);
        assert_eq!(remove_managed_block(&second), "# Existing");
    }

    #[test]
    fn skill_has_positive_and_negative_routing_rules() {
        assert!(ROUTING_SKILL.contains("multi-file"));
        assert!(ROUTING_SKILL.contains("Do not use Repotracer"));
        assert!(ROUTING_SKILL.contains("file:line citations"));
    }
}
