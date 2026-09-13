use anyhow::{Context, Result};
use repotracer_core::RepoTracerConfig;
use std::{
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Formatted, Item, Value};

const LEGACY_MAX_TURNS: i64 = 6;
const UNLIMITED_MAX_TURNS: i64 = 0;

pub fn normalize_legacy_max_turns(config: &mut RepoTracerConfig) -> bool {
    if config.explorer.max_turns != LEGACY_MAX_TURNS as u32 {
        return false;
    }
    config.explorer.max_turns = UNLIMITED_MAX_TURNS as u32;
    true
}

pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("REPOTRACER_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".repotracer")
        .join("config.toml")
}

pub fn load_or_default(path: &Path) -> RepoTracerConfig {
    if path.exists() {
        match RepoTracerConfig::load_from(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: failed to load {}: {e:#}", path.display());
                RepoTracerConfig::default()
            }
        }
    } else {
        RepoTracerConfig::default()
    }
}

/// Change the old generated six-turn ceiling to the current unlimited default.
///
/// This edits only the matching integer. A user-selected ceiling, comments,
/// and keys this version does not know about stay byte-for-byte unchanged.
pub fn migrate_legacy_max_turns(path: &Path) -> Result<bool> {
    let original = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let mut document = original
        .parse::<DocumentMut>()
        .with_context(|| format!("invalid RepoTracer config TOML in {}", path.display()))?;
    if !replace_legacy_max_turns(&mut document) {
        return Ok(false);
    }

    backup_file(path)?;
    fs::write(path, document.to_string())
        .with_context(|| format!("write migrated RepoTracer config {}", path.display()))?;
    Ok(true)
}

fn replace_legacy_max_turns(document: &mut DocumentMut) -> bool {
    let Some(explorer) = document.get_mut("explorer") else {
        return false;
    };
    let max_turns = match explorer {
        Item::Table(table) => table.get_mut("max_turns").and_then(Item::as_value_mut),
        Item::Value(Value::InlineTable(table)) => table.get_mut("max_turns"),
        _ => None,
    };
    let Some(Value::Integer(integer)) = max_turns else {
        return false;
    };
    if *integer.value() != LEGACY_MAX_TURNS {
        return false;
    }

    let mut replacement = Formatted::new(UNLIMITED_MAX_TURNS);
    std::mem::swap(replacement.decor_mut(), integer.decor_mut());
    *integer = replacement;
    true
}

fn backup_file(path: &Path) -> Result<()> {
    let backup = PathBuf::from(format!("{}.bak", path.to_string_lossy()));
    if !backup.exists() {
        fs::copy(path, &backup)
            .with_context(|| format!("back up {} before max-turn migration", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_changes_only_the_legacy_generated_limit_and_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "# keep this comment\ncustom = 'keep'\n\n[explorer]\nmax_turns = 6 # old generated default\nmax_tool_calls = 17\n\n[unknown]\nvalue = true\n";
        fs::write(&path, original).unwrap();

        assert!(migrate_legacy_max_turns(&path).unwrap());
        let once = fs::read_to_string(&path).unwrap();
        assert_eq!(
            once,
            original.replacen(
                "max_turns = 6 # old generated default",
                "max_turns = 0 # old generated default",
                1
            )
        );
        assert_eq!(
            fs::read_to_string(path.with_extension("toml.bak")).unwrap(),
            original
        );

        assert!(!migrate_legacy_max_turns(&path).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), once);
        assert_eq!(
            fs::read_to_string(path.with_extension("toml.bak")).unwrap(),
            original
        );
    }

    #[test]
    fn migration_supports_inline_tables_and_preserves_custom_limits() {
        for (limit, changed) in [(5, false), (6, true), (7, false), (0, false)] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let original =
                format!("explorer = {{ max_turns = {limit}, concurrency = 3 }} # keep\n");
            fs::write(&path, &original).unwrap();

            assert_eq!(migrate_legacy_max_turns(&path).unwrap(), changed);
            let updated = fs::read_to_string(&path).unwrap();
            let parsed: toml::Value = updated.parse().unwrap();
            assert_eq!(
                parsed["explorer"]["max_turns"].as_integer(),
                Some(if changed { 0 } else { limit })
            );
            assert!(updated.contains("concurrency = 3"));
            assert!(updated.contains("# keep"));
            assert_eq!(path.with_extension("toml.bak").exists(), changed);
        }
    }
}
