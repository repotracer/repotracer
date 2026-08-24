//! Paired benchmark plan generator.
//! Measures complete-task economics, not middleware token counters.

use anyhow::bail;
use chrono::Utc;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "repotracer-bench",
    about = "Plan paired complete-task benchmarks without launching model calls."
)]
struct Args {
    /// Suite directory under benchmarks/
    #[arg(long, default_value = "benchmarks")]
    suite: PathBuf,

    /// Include only these task IDs. Repeat the flag or pass comma-separated IDs.
    #[arg(long, value_delimiter = ',')]
    task: Vec<String>,

    /// Trials per arm and task.
    #[arg(long, default_value_t = 1)]
    repeats: usize,

    /// Stable identifier used by the result directory.
    #[arg(long, default_value = "pilot")]
    study_id: String,

    /// Write the plan JSON. Parent directories are created.
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BenchTask {
    #[serde(default = "schema_version_one")]
    schema_version: u8,
    #[serde(default = "default_suite")]
    suite: String,
    id: String,
    /// Natural user request shown verbatim to the solver in both arms.
    prompt: String,
    /// Predeclared evaluator label. Never included in the solver prompt.
    #[serde(default)]
    expected_scout: bool,
    /// Hidden localization signal for evaluation. Never included in the solver prompt.
    #[serde(default)]
    expected_paths: Vec<String>,
    /// Evaluator-only provenance. Never included in the solver prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<serde_json::Value>,
    /// Public-claim eligibility is false unless explicitly enabled.
    #[serde(default)]
    headline_eligible: bool,
}

#[derive(Debug, Serialize)]
struct Plan {
    schema_version: u8,
    study_id: String,
    generated_at: String,
    status: &'static str,
    suite_root: PathBuf,
    arms: [&'static str; 2],
    repeats_per_arm: usize,
    trial_count: usize,
    tasks: Vec<BenchTask>,
    result_layout: &'static str,
    methodology: &'static str,
}

fn schema_version_one() -> u8 {
    1
}

fn default_suite() -> String {
    "legacy-routing".into()
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.repeats == 0 {
        bail!("--repeats must be at least 1");
    }

    let tasks_dir = args.suite.join("tasks");
    let mut tasks = Vec::new();
    if tasks_dir.is_dir() {
        for entry in std::fs::read_dir(&tasks_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                let task: BenchTask = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
                if task.id.trim().is_empty() || task.prompt.trim().is_empty() {
                    bail!("{} has an empty id or prompt", path.display());
                }
                if !matches!(task.schema_version, 1 | 2) {
                    bail!(
                        "{} uses unsupported task schema {}",
                        path.display(),
                        task.schema_version
                    );
                }
                tasks.push(task);
            }
        }
    }
    tasks.sort_by(|left, right| left.id.cmp(&right.id));

    let mut ids = HashSet::new();
    if let Some(duplicate) = tasks.iter().find(|task| !ids.insert(task.id.as_str())) {
        bail!("duplicate task id: {}", duplicate.id);
    }

    if !args.task.is_empty() {
        let requested: HashSet<_> = args.task.iter().map(String::as_str).collect();
        if let Some(missing) = requested.iter().find(|id| !ids.contains(**id)) {
            bail!("unknown task id: {missing}");
        }
        tasks.retain(|task| requested.contains(task.id.as_str()));
    }

    let plan = Plan {
        schema_version: 2,
        study_id: args.study_id,
        generated_at: Utc::now().to_rfc3339(),
        status: "planned",
        suite_root: args.suite,
        arms: ["baseline", "repotracer"],
        repeats_per_arm: args.repeats,
        trial_count: tasks.len() * args.repeats * 2,
        tasks,
        result_layout: "benchmarks/results/runs/<study-id>/{manifest.json,trials/,summary.json}",
        methodology: "The solver receives the byte-identical task prompt in both arms. Only RepoTracer availability changes.",
    };
    let text = serde_json::to_string_pretty(&plan)?;

    if let Some(out) = args.out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(out, format!("{text}\n"))?;
    } else {
        println!("{text}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_tasks_keep_safe_defaults() {
        let task: BenchTask = serde_json::from_str(
            r#"{"id":"legacy","prompt":"Fix it","expected_scout":true,"expected_paths":[]}"#,
        )
        .unwrap();

        assert_eq!(task.schema_version, 1);
        assert_eq!(task.suite, "legacy-routing");
        assert!(!task.headline_eligible);
    }
}
