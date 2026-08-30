use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const MANAGED_START: &str = "<!-- repotracer:start -->";
const MANAGED_END: &str = "<!-- repotracer:end -->";

const ROUTING_INSTRUCTIONS: &str = concat!(
    "Choose the repository route from the user's requested change surface and current context before planning the first repository operation. ",
    "Scope follows ownership boundaries and explicit exhaustive inventory. ",
    "Start with one targeted local lookup for a request about one command, function, symbol, file, or localized behavior, even when its exact source and test paths are unknown. ",
    "Locating that implementation and its ordinary regression coverage stays in the local route. ",
    "Call repo_scout first when the request itself requires an exhaustive relationship, inventory, or propagation across ownership boundaries: ",
    "locate every caller or implementation; trace exported configuration or API blast radius; map an unfamiliar repository's ownership or change surface; ",
    "inventory tests and fixtures across behaviors or components; trace dependency and module relationships; compare plausible implementation owners; ",
    "or investigate cross-owner behavior before edits. ",
    "Examples that stay local: change one named CLI command's output when its source path is unknown; fix one function and add its regression coverage. ",
    "Examples that use Scout: find every caller of an exported timeout setting and trace its API blast radius; inventory setup tests and fixtures across install, update, preservation, and removal. ",
    "If the targeted local lookup fails to identify a precise surface or reveals multiple owners, call repo_scout. ",
    "After a successful handoff, use its summary and citations instead of repeating broad searches. Read one narrow cited range only for an unresolved implementation fact. ",
    "RepoTracer cannot inspect Git history; after a regression handoff, use one targeted history lookup before selecting the fix."
);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub detected: bool,
    pub configured: bool,
    pub path: Option<String>,
}

pub fn detect(_root: &Path) -> Vec<AgentInfo> {
    vec![detect_codex()]
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

fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
}

pub fn codex_config_path() -> Option<PathBuf> {
    Some(codex_home()?.join("config.toml"))
}

fn codex_skill_path() -> anyhow::Result<PathBuf> {
    Ok(codex_home()
        .ok_or_else(|| anyhow::anyhow!("no Codex home"))?
        .join("skills")
        .join("repotracer-scout")
        .join("SKILL.md"))
}

fn codex_instructions_path() -> anyhow::Result<PathBuf> {
    Ok(codex_home()
        .ok_or_else(|| anyhow::anyhow!("no Codex home"))?
        .join("AGENTS.md"))
}

pub fn install_codex(binary: &Path, dry_run: bool) -> anyhow::Result<String> {
    let config = codex_config_path().ok_or_else(|| anyhow::anyhow!("no Codex home"))?;
    let instructions = codex_instructions_path()?;
    if !dry_run {
        install_codex_config(&config, binary)?;
        upsert_managed_block(&instructions, ROUTING_INSTRUCTIONS)?;
        if let Ok(skill) = codex_skill_path() {
            if skill.exists() {
                fs::remove_file(skill)?;
            }
        }
    }
    Ok(format!(
        "{} Codex MCP + automatic routing ({})",
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

fn upsert_managed_block(path: &Path, content: &str) -> anyhow::Result<()> {
    ensure_parent(path)?;
    backup_file(path)?;
    let existing = fs::read_to_string(path).unwrap_or_default();
    let block = format!("{MANAGED_START}\n{content}\n{MANAGED_END}");
    fs::write(path, replace_managed_block(&existing, &block))?;
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

pub fn uninstall_all(_root: &Path) -> anyhow::Result<Vec<String>> {
    let mut messages = Vec::new();
    if let Some(path) = codex_config_path().filter(|p| p.exists()) {
        backup_file(&path)?;
        let updated = remove_toml_section(&fs::read_to_string(&path)?, "[mcp_servers.repotracer]");
        fs::write(&path, updated)?;
        messages.push(format!("removed RepoTracer from {}", path.display()));
    }
    if let Ok(path) = codex_skill_path() {
        if path.exists() {
            fs::remove_file(&path)?;
            messages.push(format!("removed {}", path.display()));
        }
    }
    remove_managed_instructions(&codex_instructions_path()?, &mut messages)?;
    Ok(messages)
}

fn remove_managed_instructions(path: &Path, messages: &mut Vec<String>) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let updated = remove_managed_block(&fs::read_to_string(path)?);
    if updated.is_empty() {
        fs::remove_file(path)?;
    } else {
        fs::write(path, updated + "\n")?;
    }
    messages.push(format!(
        "removed RepoTracer instructions from {}",
        path.display()
    ));
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
    fn codex_routing_uses_ownership_and_contrasting_examples() {
        for delegated_work in [
            "locate every caller or implementation",
            "exported configuration or API blast radius",
            "map an unfamiliar repository",
            "inventory tests and fixtures",
            "dependency and module relationships",
            "compare plausible implementation owners",
            "cross-owner behavior before edits",
        ] {
            assert!(ROUTING_INSTRUCTIONS.contains(delegated_work));
        }
        for local_boundary in [
            "requested change surface",
            "source and test paths are unknown",
            "ordinary regression coverage stays in the local route",
            "Examples that stay local",
            "Examples that use Scout",
        ] {
            assert!(ROUTING_INSTRUCTIONS.contains(local_boundary));
        }
        assert!(ROUTING_INSTRUCTIONS.contains("before planning the first repository operation"));
        assert!(ROUTING_INSTRUCTIONS.contains("instead of repeating broad searches"));
        assert!(ROUTING_INSTRUCTIONS.contains("one targeted history lookup"));
    }
}
