//! Paired benchmark harness scaffold.
//! Measures complete-task economics, not middleware token counters.

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "repotracer-bench",
    about = "Benchmark the bill, not imaginary token counters."
)]
struct Args {
    /// Suite directory under benchmarks/
    #[arg(long, default_value = "benchmarks")]
    suite: PathBuf,

    /// Write results JSON
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BenchTask {
    id: String,
    query: String,
    /// Paths expected to appear in citations (quality signal).
    expected_paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Aggregate {
    note: String,
    tasks: usize,
    methodology: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    println!("repotracer benchmark\n");
    println!("We benchmark complete coding-task economics.");
    println!("Not characters filtered. Not middleware counters.\n");
    println!("Suite root: {}", args.suite.display());

    let tasks_dir = args.suite.join("tasks");
    let mut tasks = Vec::new();
    if tasks_dir.is_dir() {
        for ent in std::fs::read_dir(&tasks_dir)?.flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                let t: BenchTask = serde_json::from_str(&std::fs::read_to_string(&p)?)?;
                tasks.push(t);
            }
        }
    }

    println!("Tasks discovered: {}", tasks.len());
    for t in &tasks {
        println!("  - {} — {}", t.id, t.query);
    }

    let agg = Aggregate {
        note: "No paired agent runs in this offline scaffold. Wire Claude/Codex runners under benchmarks/runners/.".into(),
        tasks: tasks.len(),
        methodology: "See docs/benchmarks/why-token-counters-lie.md and BENCHMARKS.md".into(),
    };

    let text = serde_json::to_string_pretty(&agg)?;
    if let Some(out) = args.out {
        std::fs::write(out, &text)?;
    } else {
        println!("\n{text}");
    }
    Ok(())
}
