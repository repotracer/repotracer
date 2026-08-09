use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("path escapes repository root: {0}")]
    Escape(String),
    #[error("path does not exist: {0}")]
    NotFound(String),
    #[error("not a file: {0}")]
    NotFile(String),
    #[error("not a directory: {0}")]
    NotDir(String),
    #[error("symlink escape: {0}")]
    SymlinkEscape(String),
    #[error("binary file rejected: {0}")]
    Binary(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// True if `path` is inside `root` after lexical normalization (no symlink resolve).
pub fn is_within_root(root: &Path, path: &Path) -> bool {
    let root = normalize_lexically(root);
    let path = normalize_lexically(path);
    path.starts_with(&root)
}

/// Resolve a user-supplied path against the repo root and enforce containment.
/// Accepts absolute paths only if they remain under root; otherwise joins relative.
pub fn resolve_in_root(root: &Path, input: &str) -> Result<PathBuf, PathError> {
    let root = root
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexically(root));
    let candidate = if Path::new(input).is_absolute() {
        PathBuf::from(input)
    } else {
        root.join(input)
    };

    // Lexical check first (blocks obvious `..` escapes before touching FS).
    let lexical = normalize_lexically(&candidate);
    let root_lex = normalize_lexically(&root);
    if !lexical.starts_with(&root_lex) {
        return Err(PathError::Escape(input.to_string()));
    }

    // If it exists, canonicalize and re-check (symlink escape).
    if lexical.exists() {
        let canon = lexical.canonicalize().map_err(PathError::Io)?;
        let root_canon = root.canonicalize().unwrap_or(root_lex);
        if !canon.starts_with(&root_canon) {
            return Err(PathError::SymlinkEscape(input.to_string()));
        }
        return Ok(canon);
    }

    // Non-existent path: allow if parent is inside root (for better errors later).
    Ok(lexical)
}

pub fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    // NUL in first 8KiB => binary
    let n = bytes.len().min(8192);
    bytes[..n].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn blocks_parent_escape() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert!(resolve_in_root(root, "../outside").is_err());
    }

    #[test]
    fn allows_relative_inside() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.txt"), "hi").unwrap();
        let p = resolve_in_root(root, "a.txt").unwrap();
        assert!(p.ends_with("a.txt"));
    }

    #[test]
    fn blocks_symlink_escape() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        let outside = dir.path().join("secret");
        fs::write(&outside, "nope").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
            assert!(matches!(
                resolve_in_root(&root, "link"),
                Err(PathError::SymlinkEscape(_))
            ));
        }
    }
}
