//! Parent-visible handles and ordering, separate from native process retention.
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

// Metadata only. Eviction never interrupts active requests or claims that a
// native thread survived. Unknown generated handles require an explicit root.
const MAX_HANDLES: usize = 1024;

struct Entry {
    root: Option<PathBuf>,
    gate: Arc<AsyncMutex<()>>,
    touched: Instant,
}

#[derive(Default)]
pub(crate) struct Conversations {
    entries: Mutex<HashMap<String, Entry>>,
}

impl Conversations {
    pub fn root(&self, id: &str) -> Option<PathBuf> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .and_then(|entry| entry.root.clone())
    }

    /// Join the conversation queue before any asynchronous repository work.
    /// The transport polls each request once in arrival order to reserve it.
    pub async fn enter(&self, id: &str) -> Result<OwnedMutexGuard<()>> {
        let gate = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = entries.get_mut(id) {
                entry.touched = Instant::now();
                entry.gate.clone()
            } else {
                if entries.len() >= MAX_HANDLES {
                    let oldest = entries
                        .iter()
                        .filter(|(_, e)| Arc::strong_count(&e.gate) == 1)
                        .min_by_key(|(_, e)| e.touched)
                        .map(|(id, _)| id.clone());
                    if let Some(oldest) = oldest {
                        entries.remove(&oldest);
                    } else {
                        bail!("all conversation handles are active; retry after an investigation finishes");
                    }
                }
                let gate = Arc::new(AsyncMutex::new(()));
                entries.insert(
                    id.to_owned(),
                    Entry {
                        root: None,
                        gate: gate.clone(),
                        touched: Instant::now(),
                    },
                );
                gate
            }
        };
        Ok(gate.lock_owned().await)
    }

    /// Record the current repository while holding the conversation guard.
    /// A conversation may move between related checkouts; the current target
    /// is refreshed for each turn while the gate still preserves ordering.
    pub fn bind(&self, id: &str, root: &Path) -> Result<()> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries
            .get_mut(id)
            .context("conversation was not reserved")?;
        entry.root = Some(root.to_owned());
        Ok(())
    }
}

pub(crate) fn select_repository(
    default: &Path,
    explicit: Option<&str>,
    focus: Option<&Path>,
    remembered: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(explicit) = explicit {
        if explicit.trim().is_empty() {
            bail!("repository must not be empty");
        }
        let path = Path::new(explicit);
        return directory(if path.is_absolute() {
            path.to_owned()
        } else {
            default.join(path)
        });
    }
    if let Some(remembered) = remembered {
        return directory(remembered.to_owned());
    }
    let original_default = default;
    let default = directory(default.to_owned())?;
    if let Some(focus) = focus.filter(|path| path.is_absolute()) {
        // Do not turn an in-root symlink escape into permission to change roots.
        if !(focus.starts_with(&default)
            || original_default.is_absolute() && focus.starts_with(original_default))
        {
            let resolved = focus
                .canonicalize()
                .context("absolute focus does not exist; set repository explicitly")?;
            if !resolved.starts_with(&default) {
                let start = if resolved.is_dir() {
                    resolved.as_path()
                } else {
                    resolved.parent().context("focus has no parent directory")?
                };
                // Git knows linked-worktree .git files and ignores stray .git
                // directories. Do not infer a broad root from marker existence.
                if let Some(root) = git_root(start) {
                    if resolved.starts_with(&root) {
                        return Ok(root);
                    }
                }
                bail!("absolute focus is outside the default repository and has no enclosing Git checkout; set repository explicitly");
            }
        }
    }
    Ok(default)
}

fn git_root(start: &Path) -> Option<PathBuf> {
    let mut command = std::process::Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    let output = command
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = std::str::from_utf8(&output.stdout)
        .ok()?
        .trim_end_matches(['\r', '\n']);
    directory(PathBuf::from(path)).ok()
}

fn directory(path: PathBuf) -> Result<PathBuf> {
    let root = path
        .canonicalize()
        .with_context(|| format!("repository does not exist: {}", path.display()))?;
    if !root.is_dir() {
        bail!("repository must be a directory: {}", root.display());
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_focus_selects_other_checkout() {
        let base = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(other.path())
            .status()
            .unwrap()
            .success());
        std::fs::create_dir(other.path().join("src")).unwrap();
        std::fs::write(other.path().join("src/lib.rs"), "fn start() {}\n").unwrap();
        assert_eq!(
            select_repository(
                base.path(),
                None,
                Some(&other.path().join("src/lib.rs")),
                None
            )
            .unwrap(),
            other.path().canonicalize().unwrap()
        );
        assert_eq!(
            select_repository(
                base.path(),
                Some(other.path().to_str().unwrap()),
                None,
                None
            )
            .unwrap(),
            other.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn absolute_focus_resolves_a_linked_worktree_not_its_main_checkout() {
        let parent = tempfile::tempdir().unwrap();
        let main = parent.path().join("main");
        let linked = parent.path().join("linked");
        let git = |args: &[&str]| {
            let result = std::process::Command::new("git")
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["init", "-q", main.to_str().unwrap()]);
        git(&[
            "-C",
            main.to_str().unwrap(),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ]);
        git(&[
            "-C",
            main.to_str().unwrap(),
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
        ]);
        assert!(linked.join(".git").is_file());
        assert_eq!(
            select_repository(&main, None, Some(&linked), None).unwrap(),
            linked.canonicalize().unwrap()
        );
    }

    #[test]
    fn outside_non_git_focus_needs_explicit_repository() {
        let base = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let error = select_repository(base.path(), None, Some(other.path()), None).unwrap_err();
        assert!(error.to_string().contains("set repository explicitly"));
        assert!(select_repository(base.path(), Some(""), None, None).is_err());
        assert!(select_repository(base.path(), Some("does-not-exist"), None, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn in_root_symlink_does_not_implicitly_select_outside_checkout() {
        let base = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join(".git")).unwrap();
        std::os::unix::fs::symlink(other.path(), base.path().join("escape")).unwrap();
        assert_eq!(
            select_repository(base.path(), None, Some(&base.path().join("escape")), None).unwrap(),
            base.path().canonicalize().unwrap()
        );
        // Core normalize_paths subsequently rejects this escaping focus.
    }

    #[cfg(unix)]
    #[test]
    fn aliased_root_keeps_symlink_escape_in_original_repository() {
        let base = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let aliases = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(other.path())
            .status()
            .unwrap()
            .success());
        let alias = aliases.path().join("root");
        std::os::unix::fs::symlink(base.path(), &alias).unwrap();
        std::os::unix::fs::symlink(other.path(), base.path().join("escape")).unwrap();
        assert_eq!(
            select_repository(&alias, None, Some(&alias.join("escape")), None).unwrap(),
            base.path().canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn same_handle_waits_but_independent_handle_can_enter() {
        let registry = Conversations::default();
        let first = registry.enter("a").await.unwrap();
        let second = registry.enter("a");
        tokio::pin!(second);
        tokio::select! {
            biased;
            _ = &mut second => panic!("same conversation must wait"),
            independent = registry.enter("b") => { drop(independent.unwrap()); }
        }
        drop(first);
        assert!(second.await.is_ok());
    }

    #[tokio::test]
    async fn metadata_eviction_never_evicts_active_handle() {
        let registry = Conversations::default();
        let root = tempfile::tempdir().unwrap();
        let _active = registry.enter("active").await.unwrap();
        registry.bind("active", root.path()).unwrap();
        for n in 0..MAX_HANDLES + 2 {
            let id = format!("id-{n}");
            let _turn = registry.enter(&id).await.unwrap();
            registry.bind(&id, root.path()).unwrap();
        }
        assert!(registry.root("active").is_some());
        assert!(registry.root("id-0").is_none());
        assert_eq!(registry.entries.lock().unwrap().len(), MAX_HANDLES);
    }
}
