use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub configured: bool,
    pub path: Option<String>,
}

pub fn detect() -> Vec<AgentInfo> {
    vec![
        detect_claude(),
        detect_codex(),
        detect_cursor(),
        AgentInfo {
            name: "MCP (generic)".into(),
            configured: true,
            path: Some("grephound serve".into()),
        },
    ]
}

fn detect_claude() -> AgentInfo {
    let path = claude_config_path();
    let configured = path
        .as_ref()
        .map(|p| p.exists() && file_contains_grephound(p))
        .unwrap_or(false);
    AgentInfo {
        name: "Claude Code".into(),
        configured,
        path: path.map(|p| p.display().to_string()),
    }
}

fn detect_codex() -> AgentInfo {
    let path = codex_config_path();
    let configured = path
        .as_ref()
        .map(|p| p.exists() && file_contains_grephound(p))
        .unwrap_or(false);
    AgentInfo {
        name: "Codex".into(),
        configured,
        path: path.map(|p| p.display().to_string()),
    }
}

fn detect_cursor() -> AgentInfo {
    let path = cursor_mcp_path();
    let configured = path
        .as_ref()
        .map(|p| p.exists() && file_contains_grephound(p))
        .unwrap_or(false);
    AgentInfo {
        name: "Cursor".into(),
        configured,
        path: path.map(|p| p.display().to_string()),
    }
}

fn file_contains_grephound(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|s| s.contains("grephound"))
        .unwrap_or(false)
}

pub fn claude_config_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    // Claude Code MCP config
    let p = home.join(".claude.json");
    if p.exists() {
        return Some(p);
    }
    Some(home.join(".claude.json"))
}

pub fn codex_config_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let p = home.join(".codex").join("config.toml");
    Some(p)
}

pub fn cursor_mcp_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(".cursor").join("mcp.json"))
}

/// Idempotent MCP server entry for grephound.
pub fn grephound_mcp_entry(binary: &Path) -> Value {
    json!({
        "command": binary,
        "args": ["serve"],
    })
}

pub fn install_claude(binary: &Path, dry_run: bool) -> anyhow::Result<String> {
    let path = claude_config_path().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    if dry_run {
        return Ok(format!("would configure Claude Code at {}", path.display()));
    }
    backup_file(&path)?;
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?)?
    } else {
        json!({})
    };
    let map = root.as_object_mut().unwrap();
    let mcp = map
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();
    mcp.insert("grephound".into(), grephound_mcp_entry(binary));
    fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(format!("configured Claude Code ({})", path.display()))
}

pub fn install_codex(binary: &Path, dry_run: bool) -> anyhow::Result<String> {
    let path = codex_config_path().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    if dry_run {
        return Ok(format!("would configure Codex at {}", path.display()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    backup_file(&path)?;
    let mut text = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };

    let block = format!(
        "\n[mcp_servers.grephound]\ncommand = \"{}\"\nargs = [\"serve\"]\n",
        binary.display()
    );

    if text.contains("[mcp_servers.grephound]") || text.contains("mcp_servers.grephound") {
        // Replace existing section crudely.
        if let Some(start) = text.find("[mcp_servers.grephound]") {
            let rest = &text[start..];
            let end = rest.find("\n[").map(|i| start + i).unwrap_or(text.len());
            text.replace_range(start..end, block.trim_start());
        }
    } else {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&block);
    }
    fs::write(&path, text)?;
    Ok(format!("configured Codex ({})", path.display()))
}

pub fn install_cursor(binary: &Path, dry_run: bool) -> anyhow::Result<String> {
    let path = cursor_mcp_path().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    if dry_run {
        return Ok(format!("would configure Cursor at {}", path.display()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    backup_file(&path)?;
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|_| "{}".into()))?
    } else {
        json!({})
    };
    let map = root.as_object_mut().unwrap();
    let mcp = map
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();
    mcp.insert("grephound".into(), grephound_mcp_entry(binary));
    fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(format!("configured Cursor ({})", path.display()))
}

pub fn uninstall_all() -> anyhow::Result<Vec<String>> {
    let mut msgs = Vec::new();
    if let Some(path) = claude_config_path() {
        if path.exists() {
            backup_file(&path)?;
            if let Ok(mut root) = serde_json::from_str::<Value>(&fs::read_to_string(&path)?) {
                if let Some(mcp) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
                    mcp.remove("grephound");
                    fs::write(&path, serde_json::to_string_pretty(&root)?)?;
                    msgs.push(format!("removed grephound from {}", path.display()));
                }
            }
        }
    }
    if let Some(path) = codex_config_path() {
        if path.exists() {
            backup_file(&path)?;
            let text = fs::read_to_string(&path)?;
            if text.contains("grephound") {
                // Strip grephound section
                let mut out = String::new();
                let mut skip = false;
                for line in text.lines() {
                    if line.trim() == "[mcp_servers.grephound]" {
                        skip = true;
                        continue;
                    }
                    if skip {
                        if line.starts_with('[') {
                            skip = false;
                        } else {
                            continue;
                        }
                    }
                    if !skip {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                fs::write(&path, out)?;
                msgs.push(format!("removed grephound from {}", path.display()));
            }
        }
    }
    if let Some(path) = cursor_mcp_path() {
        if path.exists() {
            backup_file(&path)?;
            if let Ok(mut root) = serde_json::from_str::<Value>(&fs::read_to_string(&path)?) {
                if let Some(mcp) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
                    mcp.remove("grephound");
                    fs::write(&path, serde_json::to_string_pretty(&root)?)?;
                    msgs.push(format!("removed grephound from {}", path.display()));
                }
            }
        }
    }
    Ok(msgs)
}

fn backup_file(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        let bak = PathBuf::from(format!("{}.bak", path.to_string_lossy()));
        fs::copy(path, &bak)?;
    }
    Ok(())
}

pub fn current_binary() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("grephound"))
}
