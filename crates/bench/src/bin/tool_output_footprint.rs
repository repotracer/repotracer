use grephound_repo_tools::RepoTools;
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
struct Measurement {
    tool: &'static str,
    output_bytes: usize,
    output_lines: usize,
    duration_ms: u64,
    truncated: bool,
    actionable_continuation: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = fixture()?;
    let tools = RepoTools::new(&root);
    let cases = [
        (
            "Read",
            json!({"path": "big.txt", "offset": 1, "limit": 2000}),
        ),
        (
            "Grep",
            json!({"pattern": "NEEDLE", "path": "matches", "output_mode": "content"}),
        ),
        ("Glob", json!({"pattern": "**/*.txt", "directory": "deep"})),
    ];
    let mut measurements = Vec::new();
    for (name, arguments) in cases {
        let result = tools.call_one(name, &arguments.to_string()).await;
        anyhow::ensure!(!result.failed, "{name} failed: {}", result.output);
        let output_lower = result.output.to_ascii_lowercase();
        measurements.push(Measurement {
            tool: name,
            output_bytes: result.output.len(),
            output_lines: result.output.lines().count(),
            duration_ms: result.duration_ms,
            truncated: result.output.contains("truncated"),
            actionable_continuation: output_lower.contains("offset")
                || output_lower.contains("more specific")
                || output_lower.contains("narrow"),
        });
    }
    println!("{}", serde_json::to_string_pretty(&measurements)?);
    std::fs::remove_dir_all(root)?;
    Ok(())
}

fn fixture() -> anyhow::Result<PathBuf> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!("grephound-output-footprint-{nonce}"));
    std::fs::create_dir_all(root.join("matches"))?;
    let big = (1..=2000)
        .map(|line| format!("line-{line:04} {}", "x".repeat(1980)))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(root.join("big.txt"), big)?;
    for index in 0..100 {
        std::fs::write(
            root.join("matches").join(format!("match-{index:03}.txt")),
            format!("NEEDLE-{index:03}-{}\n", "y".repeat(1980)),
        )?;
        let branch = format!("branch-{index:03}-{}", "a".repeat(180));
        let leaf = format!("leaf-{index:03}-{}", "b".repeat(180));
        let dir = root.join("deep").join(branch).join(leaf);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("result-{index:03}.txt")), "evidence\n")?;
    }
    Ok(root)
}
