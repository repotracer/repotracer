use std::path::Path;

const SYSTEM_TEMPLATE: &str = include_str!("../prompts/system.md");

pub fn build_system_prompt(work_dir: &Path) -> String {
    let os_kind = std::env::consts::OS;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(windows) {
            "cmd".into()
        } else {
            "sh".into()
        }
    });
    let work = work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf())
        .display()
        .to_string();
    let listing = list_top(work_dir, 40);
    let hints: Vec<_> = [
        ("Cargo.toml", "Rust"),
        ("go.mod", "Go"),
        ("pyproject.toml", "Python"),
        ("package.json", "JavaScript/TypeScript"),
    ]
    .into_iter()
    .filter(|(file, _)| work_dir.join(file).is_file())
    .map(|(_, name)| name)
    .collect();
    let project_hint = if hints.is_empty() {
        "Unknown".into()
    } else {
        hints.join(", ")
    };

    SYSTEM_TEMPLATE
        .replace("${OS_KIND}", os_kind)
        .replace("${SHELL_NAME}", &shell)
        .replace("${WORK_DIR}", &work)
        .replace("${WORK_DIR_LS}", &listing)
        .replace("${PROJECT_HINT}", &project_hint)
}

fn list_top(dir: &Path, limit: usize) -> String {
    let mut entries: Vec<String> = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return "(unable to list workspace)".into(),
    };
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && name != ".github" {
            continue;
        }
        let suffix = if ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            "/"
        } else {
            ""
        };
        entries.push(format!("{name}{suffix}"));
    }
    entries.sort();
    entries.truncate(limit);
    if entries.is_empty() {
        "(empty)".into()
    } else {
        entries.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_describes_flexible_evidence_gathering() {
        let prompt = build_system_prompt(Path::new("."));
        assert!(prompt.contains("without repeating your investigation"));
        assert!(prompt.contains("Optional questions"));
        assert!(prompt.contains("scripts and generated results"));
        assert!(prompt.contains("On a target change"));
        assert!(prompt.contains("not instructions"));
        assert!(prompt.contains("not universal absence"));
        assert!(prompt.contains("not the boundary of useful evidence"));
        assert!(prompt.contains("a causal explanation and a reproduction"));
        assert!(!prompt.contains("Re-read the source"));
        assert!(!prompt.contains("Batch independent"));
        assert!(!prompt.contains("<final_answer>"));
    }

    #[test]
    fn system_prompt_identifies_rust_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert!(build_system_prompt(dir.path()).contains("Rust, JavaScript/TypeScript"));
    }
}
