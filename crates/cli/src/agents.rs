use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use toml_edit::{value, DocumentMut, InlineTable, Item, Table, TableLike, Value};

const MANAGED_START: &str = "<!-- repotracer:start -->";
const MANAGED_END: &str = "<!-- repotracer:end -->";

pub(crate) const ROUTING_INSTRUCTIONS: &str = concat!(
    "RepoTracer investigates repository tasks using a separately configured model. Delegate when finding, understanding or checking relevant behavior would otherwise take your time; a known small lookup may be simpler locally. ",
    "Give repo_scout the objective, requirements, corrections and relevant context you already know. Do not investigate first to prepare a delegation. Query alone is enough; search terms and file names are optional. Example: 'We are adding layered configuration. Find the loader and investigate what affects precedence. Requirements: later files win; preserve the old API.' ",
    "The investigator can follow useful related leads and use available scripts, tests and other tools. It returns an explanation with selected source and experiment evidence. Source attachments come from files; conclusions are model judgments. Use the answer to continue the task. Check a specific missing fact, conflicting evidence or changed source when it matters, rather than repeating the investigation. Review your own edits and run relevant checks as usual. ",
    "You may select investigation.reasoning_effort when the assignment already makes the need clear: medium for straightforward questions, high for difficult diagnosis or interacting behavior. Supported higher levels remain available. Omit it to use configured effort and native adaptive behavior. Do not search merely to decide effort. ",
    "Set repository to the current target when different from the server startup directory. It is a starting location, not a restriction on useful related evidence. Reuse conversation.id in investigation.conversation_id whenever its existing context helps, including a related assignment in another repository; supply the new target explicitly. Start fresh when relevance is unclear. No extra investigation or elapsed-time rule is needed to decide. ",
    "The response identifies actual source locations and whether native history resumed, started fresh or is unknown. Give necessary current context when history is unavailable. Independent calls can run in parallel; calls on the same conversation run in order. ",
    "Prefer directly exposed repo_scout when available. structuredContent contains the answer and evidence; content[].text is a readable alternative. Use either representation, not both. If direct exposure is unavailable, use the host's supported tool discovery and wait mechanism."
);

const REPOTRACER_NAMESPACE: &str = "mcp__repotracer";
const MIN_TESTED_CODEX_VERSION: (u64, u64, u64) = (0, 153, 4);
const CODEX_VERSION_TIMEOUT: Duration = Duration::from_secs(2);
// A caller allowance, not a scout turn limit. The prior 120-second benchmark
// setting cut off live investigations and caused the parent to repeat them.
// This operational default is configurable; preserve any explicit user value.
const SCOUT_TOOL_TIMEOUT_SECS: i64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub detected: bool,
    pub configured: bool,
    pub path: Option<String>,
}

pub fn detect(_root: &Path) -> Vec<AgentInfo> {
    let path = claude_instructions_path();
    let claude = AgentInfo {
        name: "Claude Code".into(),
        detected: which::which("claude").is_ok(),
        configured: path
            .as_ref()
            .is_some_and(|p| fs::read_to_string(p).is_ok_and(|s| s.contains(MANAGED_START))),
        path: path.map(|p| p.display().to_string()),
    };
    vec![detect_codex(), claude]
}

pub(crate) fn claude_instructions_path() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".claude")))
        .map(|p| p.join("CLAUDE.md"))
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
    // Look the key up in the parsed document instead of matching a header line.
    // `configure_mcp_server` also writes the server as an inline entry when the
    // user keeps `mcp_servers` inline, and that form never emits a
    // `[mcp_servers.repotracer]` header. Detection has to mirror the writer or a
    // successful install reports `configured: false`.
    // This runs on a user-controlled file, so an unreadable or malformed config
    // is simply "not configured" rather than an error.
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(document) = parse_codex_config(&text) else {
        return false;
    };
    document
        .get("mcp_servers")
        .and_then(|servers| servers.get(REPOTRACER_SERVER_NAME))
        .is_some()
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
    // Refreshes must retain the profile selected through `settings`.
    // That command passes replacement args explicitly when the user changes it.
    let config = codex_config_path().ok_or_else(|| anyhow::anyhow!("no Codex home"))?;
    let text = match fs::read_to_string(config) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let document = parse_codex_config(&text)?;
    let args = match document
        .get("mcp_servers")
        .and_then(|servers| servers.get(REPOTRACER_SERVER_NAME))
        .and_then(|server| server.get("args"))
    {
        Some(args) => args
            .as_array()
            .and_then(|args| {
                args.iter()
                    .map(|arg| arg.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Codex config `mcp_servers.repotracer.args` must be an array of strings"
                )
            })?,
        None => vec!["serve".to_owned()],
    };
    install_codex_with_args(binary, &args, dry_run)
}

pub(crate) fn install_codex_with_args(
    binary: &Path,
    args: &[String],
    dry_run: bool,
) -> anyhow::Result<String> {
    let config = codex_config_path().ok_or_else(|| anyhow::anyhow!("no Codex home"))?;
    let instructions = codex_instructions_path()?;
    let direct_status = if dry_run {
        None
    } else {
        Some(install_codex_config(&config, binary, args)?)
    };
    if !dry_run {
        upsert_managed_block(&instructions, ROUTING_INSTRUCTIONS)?;
        if let Ok(skill) = codex_skill_path() {
            if skill.exists() {
                fs::remove_file(skill)?;
            }
        }
    }
    if let Some(DirectExposureStatus::Skipped(reason)) = &direct_status {
        eprintln!("RepoTracer: warning: {reason}");
    }
    let direct_summary = match direct_status {
        Some(DirectExposureStatus::Enabled) => "; direct RepoTracer tool exposure enabled",
        Some(DirectExposureStatus::Skipped(_)) => "; direct RepoTracer tool exposure skipped",
        None => "; direct RepoTracer tool exposure enabled when supported",
    };
    Ok(format!(
        "{} Codex MCP + automatic routing{} ({})",
        action(dry_run),
        direct_summary,
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

fn install_codex_config(
    path: &Path,
    binary: &Path,
    args: &[String],
) -> anyhow::Result<DirectExposureStatus> {
    ensure_parent(path)?;
    let text = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    // Parse and edit the complete document before making a backup or writing it.
    // A malformed user config must never be replaced by a partial installation.
    let mut document = parse_codex_config(&text)?;
    let direct_status = codex_direct_exposure_status();
    configure_mcp_server(&mut document, binary, args)?;
    if matches!(direct_status, DirectExposureStatus::Enabled) {
        configure_direct_namespace(&mut document)?;
    }
    let updated = document.to_string();
    backup_file(path)?;
    fs::write(path, updated)?;
    Ok(direct_status)
}

fn parse_codex_config(text: &str) -> anyhow::Result<DocumentMut> {
    text.parse::<DocumentMut>()
        .map_err(|error| anyhow::anyhow!("invalid Codex config TOML: {error}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DirectExposureStatus {
    Enabled,
    Skipped(String),
}

fn configure_mcp_server(
    document: &mut DocumentMut,
    binary: &Path,
    args: &[String],
) -> anyhow::Result<()> {
    let mcp_servers = document
        .as_table_mut()
        .entry("mcp_servers")
        .or_insert(Item::Table(Table::new()));
    let server = match mcp_servers {
        Item::Table(mcp_servers) => mcp_servers
            .entry(REPOTRACER_SERVER_NAME)
            .or_insert(Item::Table(Table::new())),
        Item::Value(Value::InlineTable(mcp_servers)) => {
            TableLike::entry(mcp_servers, REPOTRACER_SERVER_NAME)
                .or_insert(Item::Value(Value::InlineTable(InlineTable::new())))
        }
        other => {
            anyhow::bail!(
                "Codex config `mcp_servers` must be a table, found {}",
                other.type_name()
            )
        }
    };
    let server = table_like_mut(server, "mcp_servers.repotracer")?;
    server.insert("command", value(binary.display().to_string()));
    let args_value: Value = args.iter().cloned().collect();
    server.insert("args", Item::Value(args_value));
    server
        .entry("tool_timeout_sec")
        .or_insert(value(SCOUT_TOOL_TIMEOUT_SECS));
    Ok(())
}

const REPOTRACER_SERVER_NAME: &str = "repotracer";

fn table_like_mut<'a>(item: &'a mut Item, path: &str) -> anyhow::Result<&'a mut dyn TableLike> {
    match item {
        Item::Table(table) => Ok(table),
        Item::Value(Value::InlineTable(table)) => Ok(table),
        other => anyhow::bail!(
            "Codex config `{path}` must be a table, found {}",
            other.type_name()
        ),
    }
}

fn configure_direct_namespace(document: &mut DocumentMut) -> anyhow::Result<()> {
    let features = document
        .as_table_mut()
        .entry("features")
        .or_insert(Item::Table(Table::new()));
    let (code_mode, inline_parent) = match features {
        Item::Table(features) => (
            features
                .entry("code_mode")
                .or_insert(Item::Table(Table::new())),
            false,
        ),
        Item::Value(Value::InlineTable(features)) => (
            TableLike::entry(features, "code_mode")
                .or_insert(Item::Value(Value::InlineTable(InlineTable::new()))),
            true,
        ),
        other => {
            anyhow::bail!(
                "Codex config `features` must be a table, found {}",
                other.type_name()
            )
        }
    };
    let code_mode = ensure_code_mode_table(code_mode, inline_parent)?;
    let namespace_item = code_mode
        .entry("direct_only_tool_namespaces")
        .or_insert(Item::Value(std::iter::empty::<String>().collect()));
    let namespace_list = namespace_item
        .as_value_mut()
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Codex config `features.code_mode.direct_only_tool_namespaces` must be an array"
            )
        })?;
    let known = namespace_list
        .iter()
        .map(|entry| entry.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Codex config `features.code_mode.direct_only_tool_namespaces` must contain strings"
            )
        })?;
    if !known
        .iter()
        .any(|namespace| namespace == REPOTRACER_NAMESPACE)
    {
        namespace_list.push(REPOTRACER_NAMESPACE);
    }
    Ok(())
}

fn ensure_code_mode_table(
    item: &mut Item,
    inline_parent: bool,
) -> anyhow::Result<&mut dyn TableLike> {
    if let Item::Value(existing_value) = item {
        if let Some(enabled) = existing_value.as_bool() {
            let suffix = existing_value.decor().suffix().cloned();
            let mut enabled_value = Value::from(enabled);
            if let Some(suffix) = suffix {
                enabled_value.decor_mut().set_suffix(suffix);
            }
            if inline_parent {
                let mut table = InlineTable::new();
                table.insert("enabled", enabled_value);
                *item = Item::Value(Value::InlineTable(table));
            } else {
                let mut table = Table::new();
                table.insert("enabled", Item::Value(enabled_value));
                *item = Item::Table(table);
            }
        }
    }
    table_like_mut(item, "features.code_mode")
}

pub(crate) fn upsert_managed_block(path: &Path, content: &str) -> anyhow::Result<()> {
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
        let text = fs::read_to_string(&path)?;
        let updated = remove_codex_entries(&text)?;
        if updated != text {
            backup_file(&path)?;
            fs::write(&path, updated)?;
            messages.push(format!("removed RepoTracer from {}", path.display()));
        }
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

fn remove_codex_entries(text: &str) -> anyhow::Result<String> {
    let mut document = parse_codex_config(text)?;
    let mut changed = false;
    let remove_mcp_servers = {
        let root = document.as_table_mut();
        if let Some(mcp_servers) = root.get_mut("mcp_servers") {
            let mcp_servers = table_like_mut(mcp_servers, "mcp_servers")?;
            if mcp_servers.remove(REPOTRACER_SERVER_NAME).is_some() {
                changed = true;
            }
            mcp_servers.is_empty()
        } else {
            false
        }
    };
    if remove_mcp_servers {
        document.as_table_mut().remove("mcp_servers");
    }

    let remove_features = {
        let root = document.as_table_mut();
        if let Some(features) = root.get_mut("features") {
            let features = table_like_mut(features, "features")?;
            let remove_code_mode = if let Some(code_mode) = features.get_mut("code_mode") {
                match code_mode {
                    Item::Table(code_mode) => remove_direct_namespace(code_mode, &mut changed)?,
                    Item::Value(Value::InlineTable(code_mode)) => {
                        remove_direct_namespace(code_mode, &mut changed)?
                    }
                    Item::Value(Value::Boolean(_)) => {
                        // A boolean is a user's untouched code-mode setting. It has
                        // no RepoTracer namespace to remove.
                        false
                    }
                    other => anyhow::bail!(
                        "Codex config `features.code_mode` must be a table or boolean, found {}",
                        other.type_name()
                    ),
                }
            } else {
                false
            };
            if remove_code_mode {
                features.remove("code_mode");
            }
            features.is_empty()
        } else {
            false
        }
    };
    if remove_features {
        document.as_table_mut().remove("features");
    }
    Ok(if changed {
        document.to_string()
    } else {
        text.to_string()
    })
}

fn remove_direct_namespace(
    code_mode: &mut dyn TableLike,
    changed: &mut bool,
) -> anyhow::Result<bool> {
    if let Some(namespace_item) = code_mode.get_mut("direct_only_tool_namespaces") {
        let namespace_list = namespace_item
            .as_value_mut()
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Codex config `features.code_mode.direct_only_tool_namespaces` must be an array"
                )
            })?;
        let indices = namespace_list
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                (entry.as_str() == Some(REPOTRACER_NAMESPACE)).then_some(index)
            })
            .collect::<Vec<_>>();
        for index in indices.into_iter().rev() {
            namespace_list.remove(index);
            *changed = true;
        }
        if namespace_list.is_empty() {
            code_mode.remove("direct_only_tool_namespaces");
        }
    }
    Ok(code_mode.is_empty())
}

fn codex_direct_exposure_status() -> DirectExposureStatus {
    let executable = match which::which("codex") {
        Ok(path) => path,
        Err(_) => {
            return DirectExposureStatus::Skipped(
                "could not verify the Codex version because `codex` was not found on PATH; leaving direct tool exposure unchanged".into(),
            )
        }
    };
    let output = match codex_version_output(&executable) {
        Ok(output) => output,
        Err(reason) => {
            return DirectExposureStatus::Skipped(format!(
            "could not verify the Codex version ({reason}); leaving direct tool exposure unchanged"
        ))
        }
    };
    codex_direct_exposure_for_version(parse_codex_version(&output))
}

fn codex_direct_exposure_for_version(version: Option<(u64, u64, u64)>) -> DirectExposureStatus {
    let Some(version) = version else {
        return DirectExposureStatus::Skipped(
            "could not parse the Codex version; leaving direct tool exposure unchanged".into(),
        );
    };
    if version >= MIN_TESTED_CODEX_VERSION {
        DirectExposureStatus::Enabled
    } else {
        DirectExposureStatus::Skipped(format!(
            "Codex {}.{}.{} predates the tested direct-tool configuration (0.153.4); leaving direct tool exposure unchanged",
            version.0, version.1, version.2
        ))
    }
}

fn codex_version_output(executable: &Path) -> Result<String, String> {
    let mut child = Command::new(executable)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + CODEX_VERSION_TIMEOUT;
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) => {
                let output = child
                    .wait_with_output()
                    .map_err(|error| error.to_string())?;
                if !status.success() {
                    return Err(format!("`codex --version` exited with {status}"));
                }
                return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("`codex --version` timed out".into());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn parse_codex_version(text: &str) -> Option<(u64, u64, u64)> {
    for line in text.lines() {
        let mut tokens = line.split_whitespace();
        let Some(product) = tokens.next().map(str::to_ascii_lowercase) else {
            continue;
        };
        if product != "codex" && !product.starts_with("codex-") && !product.starts_with("codex_") {
            continue;
        }
        for token in tokens {
            let token = token.trim_matches(|character: char| {
                !character.is_ascii_digit() && !matches!(character, '.' | '-' | '+')
            });
            let core = token.split(['-', '+']).next().unwrap_or_default();
            let mut parts = core.split('.');
            let version = match (parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some(major), Some(minor), Some(patch), None) => {
                    match (major.parse(), minor.parse(), patch.parse()) {
                        (Ok(major), Ok(minor), Ok(patch)) => Some((major, minor, patch)),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(version) = version {
                return Some(version);
            }
        }
    }
    None
}

pub(crate) fn remove_managed_instructions(
    path: &Path,
    messages: &mut Vec<String>,
) -> anyhow::Result<()> {
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
    // Never clobber an existing backup. `selfupdate` re-runs the install path
    // through `__refresh-integration` after every auto-update, so overwriting
    // would replace the user's pre-RepoTracer snapshot with our own output and
    // lose the pristine copy for good. The first backup is the one worth keeping.
    let backup = PathBuf::from(format!("{}.bak", path.to_string_lossy()));
    if path.exists() && !backup.exists() {
        fs::copy(path, backup)?;
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
    fn codex_config_update_is_idempotent_and_preserves_existing_fields() {
        let original = r#"
model = "gpt"
[mcp_servers]
repotracer = { command = "old", args = ["serve"], startup_timeout_sec = 9 }
other = { command = "keep" }
[features]
code_mode = { enabled = false, other_setting = "keep" }
[unrelated]
value = 1
"#;
        let once = update_codex_config(original, &["serve".into()]);
        let twice = update_codex_config(&once, &["serve".into()]);
        assert_eq!(once, twice);
        assert_eq!(once.matches(REPOTRACER_NAMESPACE).count(), 1);
        assert!(once.contains("startup_timeout_sec = 9"));
        assert!(once.contains("other_setting = \"keep\""));
        assert!(once.contains("other = { command = \"keep\" }"));
        assert!(once.contains(REPOTRACER_NAMESPACE));
        assert!(once.parse::<DocumentMut>().is_ok());
    }

    #[test]
    fn uninstall_residue_is_not_a_configured_install() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let installed =
            "model_provider = \"repotracer\"\n[mcp_servers.repotracer]\ncommand = \"repotracer\"\n";
        fs::write(&path, remove_codex_entries(installed).unwrap()).unwrap();

        assert!(!file_contains_repotracer(&path));
    }

    #[test]
    fn detection_matches_every_form_the_installer_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for configured in [
            "[mcp_servers.repotracer]\ncommand = \"repotracer\"\n",
            "[mcp_servers]\nrepotracer = { command = \"repotracer\" }\n",
            "mcp_servers = { repotracer = { command = \"repotracer\" } }\n",
        ] {
            fs::write(&path, configured).unwrap();
            assert!(file_contains_repotracer(&path), "config: {configured}");
        }
        for unconfigured in [
            "[mcp_servers]\nother = { command = \"keep\" }\n",
            "mcp_servers = { other = { command = \"keep\" } }\n",
            "model = \"gpt\"\n",
        ] {
            fs::write(&path, unconfigured).unwrap();
            assert!(!file_contains_repotracer(&path), "config: {unconfigured}");
        }
    }

    #[test]
    fn detection_is_infallible_for_unreadable_or_malformed_configs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(!file_contains_repotracer(&path));
        fs::write(&path, "[mcp_servers.repotracer\ncommand = \"repotracer\"\n").unwrap();
        assert!(!file_contains_repotracer(&path));
    }

    #[test]
    fn backup_keeps_the_pre_repotracer_snapshot_across_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let backup = dir.path().join("config.toml.bak");

        // Nothing to snapshot before the user has a config.
        backup_file(&path).unwrap();
        assert!(!backup.exists());

        fs::write(&path, "pristine").unwrap();
        backup_file(&path).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), "pristine");

        // A later auto-update refresh must not overwrite the first snapshot.
        fs::write(&path, "installed").unwrap();
        backup_file(&path).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), "pristine");
    }

    #[test]
    fn boolean_and_dotted_code_mode_forms_retain_enabled() {
        for (input, expected) in [
            ("[features]\ncode_mode = true\n", "enabled = true"),
            ("[features]\ncode_mode.enabled = false\n", "enabled = false"),
        ] {
            let updated = update_codex_config(input, &["serve".into()]);
            assert!(updated.contains(expected), "updated config: {updated}");
            assert!(updated.contains(REPOTRACER_NAMESPACE));
            assert!(updated.parse::<DocumentMut>().is_ok());
        }
    }

    #[test]
    fn empty_and_boolean_inline_tables_round_trip_with_profile_args() {
        for input in [
            "features = {}\nmcp_servers = {}\n",
            "features = { code_mode = false }\nmcp_servers = {}\n",
            "features = { code_mode = true }\nmcp_servers = {}\n",
        ] {
            let args = vec![
                "serve".into(),
                "--config".into(),
                "a path/profile.toml".into(),
            ];
            let updated = update_codex_config(input, &args);
            let parsed: toml::Value = updated.parse().unwrap();
            assert_eq!(
                parsed["mcp_servers"]["repotracer"]["args"]
                    .as_array()
                    .unwrap()
                    .len(),
                3
            );
            assert_eq!(
                parsed["mcp_servers"]["repotracer"]["args"][2].as_str(),
                Some("a path/profile.toml")
            );
            assert_eq!(
                parsed["features"]["code_mode"]["direct_only_tool_namespaces"][0].as_str(),
                Some(REPOTRACER_NAMESPACE)
            );
            if input.contains("false") || input.contains("true") {
                assert_eq!(
                    parsed["features"]["code_mode"]["enabled"].as_bool(),
                    Some(input.contains("true"))
                );
            }
            assert_eq!(updated, update_codex_config(&updated, &args));
            let removed: toml::Value = remove_codex_entries(&updated).unwrap().parse().unwrap();
            assert!(removed.get("mcp_servers").is_none());
            if input.contains("false") || input.contains("true") {
                assert_eq!(
                    removed["features"]["code_mode"]["enabled"].as_bool(),
                    Some(input.contains("true"))
                );
            } else {
                assert!(removed.get("features").is_none());
            }
        }
    }

    #[test]
    fn comments_other_namespaces_and_explicit_disable_survive_upgrade() {
        let input = "# user header\n[features] # features note\ncode_mode = false # keep disabled\nother = true # unrelated note\n";
        let updated = update_codex_config(input, &["serve".into()]);
        for comment in [
            "# user header",
            "# features note",
            "# keep disabled",
            "# unrelated note",
        ] {
            assert!(updated.contains(comment), "{updated}");
        }
        let parsed: toml::Value = updated.parse().unwrap();
        assert_eq!(
            parsed["features"]["code_mode"]["enabled"].as_bool(),
            Some(false)
        );

        let input = "[features.code_mode]\ndirect_only_tool_namespaces = [\"keep\"] # namespaces\n";
        let updated = update_codex_config(input, &["serve".into()]);
        assert!(updated.contains("# namespaces"));
        let parsed: toml::Value = updated.parse().unwrap();
        let namespaces = parsed["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array()
            .unwrap();
        assert_eq!(namespaces.len(), 2);
        assert_eq!(namespaces[0].as_str(), Some("keep"));
        assert_eq!(namespaces[1].as_str(), Some(REPOTRACER_NAMESPACE));
    }

    #[test]
    fn inline_tables_are_updated_and_uninstall_keeps_unrelated_values() {
        let input = r#"features = { code_mode = { enabled = false, direct_only_tool_namespaces = ["keep", "mcp__repotracer", "mcp__repotracer"], other = "value" } }
mcp_servers = { repotracer = { command = "old" }, other = { command = "keep" } }
"#;
        let updated = update_codex_config(input, &["serve".into()]);
        let removed = remove_codex_entries(&updated).unwrap();
        assert!(removed.contains("enabled = false"));
        assert!(removed.contains("direct_only_tool_namespaces = [\"keep\"]"));
        assert!(removed.contains("other = \"value\""));
        assert!(removed.contains("other = { command = \"keep\" }"));
        assert!(!removed.contains(REPOTRACER_NAMESPACE));
        assert!(!removed.contains("repotracer ="));
        assert!(removed.parse::<DocumentMut>().is_ok());
    }

    #[test]
    fn invalid_config_is_rejected_before_an_update() {
        let invalid = "[features\ncode_mode = true\n";
        assert!(parse_codex_config(invalid).is_err());
    }

    #[test]
    fn scout_caller_timeout_defaults_long_and_preserves_explicit_values() {
        for explicit in [None, Some(42), Some(1800)] {
            let input = explicit
                .map(|seconds| format!("[mcp_servers.repotracer]\ntool_timeout_sec = {seconds}\n"))
                .unwrap_or_default();
            let updated: toml::Value = update_codex_config(&input, &["serve".into()])
                .parse()
                .unwrap();
            assert_eq!(
                updated["mcp_servers"]["repotracer"]["tool_timeout_sec"].as_integer(),
                Some(explicit.unwrap_or(SCOUT_TOOL_TIMEOUT_SECS))
            );
        }
    }

    #[test]
    fn direct_exposure_uses_only_the_tested_version_floor() {
        assert_eq!(
            codex_direct_exposure_for_version(Some((0, 153, 4))),
            DirectExposureStatus::Enabled
        );
        assert!(matches!(
            codex_direct_exposure_for_version(Some((0, 153, 3))),
            DirectExposureStatus::Skipped(_)
        ));
        assert!(matches!(
            codex_direct_exposure_for_version(None),
            DirectExposureStatus::Skipped(_)
        ));
    }

    #[test]
    fn version_parser_ignores_non_codex_numeric_lines() {
        assert_eq!(
            parse_codex_version("node 22.0.0\ncodex-cli 0.153.4\n"),
            Some((0, 153, 4))
        );
        assert_eq!(parse_codex_version("node 22.0.0\n"), None);
        assert_eq!(parse_codex_version("codex-cli unknown\n"), None);
    }

    fn update_codex_config(text: &str, args: &[String]) -> String {
        let mut document = parse_codex_config(text).unwrap();
        configure_mcp_server(&mut document, Path::new("repotracer"), args).unwrap();
        configure_direct_namespace(&mut document).unwrap();
        document.to_string()
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
    fn parent_delegates_existing_context_without_preliminary_investigation() {
        assert!(ROUTING_INSTRUCTIONS.contains("context you already know"));
        assert!(ROUTING_INSTRUCTIONS.contains("Requirements: later files win"));
        for capability in [
            "Do not investigate first",
            "Query alone is enough",
            "scripts, tests and other tools",
            "selected source and experiment evidence",
            "including a related assignment in another repository",
            "not a restriction on useful related evidence",
        ] {
            assert!(
                ROUTING_INSTRUCTIONS.contains(capability),
                "missing capability: {capability}"
            );
        }
        for removed_rule in [
            "Call repo_scout first",
            "before planning the first repository operation",
            "repeating broad searches",
            "one targeted history lookup",
        ] {
            assert!(!ROUTING_INSTRUCTIONS.contains(removed_rule));
        }
    }
}
