//! Paired benchmark harness scaffold.
//! Measures complete-task economics, not middleware token counters.

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "grephound-bench",
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
    /// Natural user request shown verbatim to the solver in both arms.
    prompt: String,
    /// Predeclared evaluator label. Never included in the solver prompt.
    expected_scout: bool,
    /// Hidden localization signal for evaluation. Never included in the solver prompt.
    expected_paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Aggregate {
    note: String,
    tasks: usize,
    expected_scout_tasks: usize,
    expected_skip_tasks: usize,
    expected_path_assertions: usize,
    methodology: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    println!("grephound benchmark\n");
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
    for task in &tasks {
        println!("  - {} — {}", task.id, task.prompt);
    }

    let expected_scout_tasks = tasks.iter().filter(|task| task.expected_scout).count();
    let expected_path_assertions = tasks.iter().map(|task| task.expected_paths.len()).sum();
    let agg = Aggregate {
        note: "Runner scaffold only. Recorded paired artifacts live under benchmarks/results/."
            .into(),
        tasks: tasks.len(),
        expected_scout_tasks,
        expected_skip_tasks: tasks.len() - expected_scout_tasks,
        expected_path_assertions,
        methodology: "Natural user prompts are identical in both arms; routing labels and expected paths stay evaluator-only. See BENCHMARKS.md".into(),
    };

    let text = serde_json::to_string_pretty(&agg)?;
    if let Some(out) = args.out {
        std::fs::write(out, &text)?;
    } else {
        println!("\n{text}");
    }
    Ok(())
}
