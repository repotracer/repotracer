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
    let work = work_dir.display().to_string();
    let listing = list_top(work_dir, 40);

    SYSTEM_TEMPLATE
        .replace("${OS_KIND}", os_kind)
        .replace("${SHELL_NAME}", &shell)
        .replace("${WORK_DIR}", &work)
        .replace("${WORK_DIR_LS}", &listing)
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
