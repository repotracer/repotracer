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
    let project_hint = if work_dir.join("Cargo.toml").is_file() {
        "Rust; search .rs files"
    } else if work_dir.join("go.mod").is_file() {
        "Go; search .go files"
    } else if work_dir.join("pyproject.toml").is_file() {
        "Python; search .py files"
    } else if work_dir.join("package.json").is_file() {
        "JavaScript/TypeScript; search .js, .jsx, .ts, and .tsx files"
    } else {
        "Unknown; infer it from the authoritative directory listing"
    };

    SYSTEM_TEMPLATE
        .replace("${OS_KIND}", os_kind)
        .replace("${SHELL_NAME}", &shell)
        .replace("${WORK_DIR}", &work)
        .replace("${WORK_DIR_LS}", &listing)
        .replace("${PROJECT_HINT}", project_hint)
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
        if entries.len() >= limit {
            break;
        }
    }
    entries.sort();
    if entries.is_empty() {
        "(empty)".into()
    } else {
        entries.join("\n")
    }
}

pub fn user_query_prompt(query: &str) -> String {
    format!("<query>\n{query}\n</query>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_requests_a_ranked_bounded_handoff() {
        let prompt = build_system_prompt(Path::new("."));
        assert!(prompt.contains("normally 3-6"));
        assert!(prompt.contains("never more than 8"));
        assert!(prompt.contains("Order files to modify and tests first"));
        assert!(prompt.contains("Prefer tight ranges"));
        assert!(prompt.contains("split separate regions"));
        assert!(prompt.contains("omit low-value documentation"));
        assert!(prompt.contains("A failed search"));
    }

    #[test]
    fn system_prompt_identifies_rust_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        assert!(build_system_prompt(dir.path()).contains("Rust; search .rs files"));
    }
}
