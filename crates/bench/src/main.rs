//! Paired benchmark plan generator.
//! Measures complete-task economics, not middleware token counters.

use anyhow::bail;
use chrono::Utc;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "repotracer-bench",
    about = "Plan paired complete-task benchmarks without launching model calls."
)]
struct Args {
    /// Suite directory containing tasks/.
    #[arg(long, default_value = "benchmarks")]
    suite: PathBuf,

    /// Include only tasks carrying this suite label.
    #[arg(long)]
    task_suite: Option<String>,

    /// Include only these task IDs. Repeat the flag or pass comma-separated IDs.
    #[arg(long, value_delimiter = ',')]
    task: Vec<String>,

    /// Trials per arm and task. Defaults to 1 for pilots and 3 for release plans.
    #[arg(long)]
    repeats: Option<usize>,

    /// Pilot permits small or unblinded suites; release enforces the publication gate.
    #[arg(long, value_enum, default_value_t = Profile::Pilot)]
    profile: Profile,

    /// Stable identifier used by the result directory and trial randomization.
    #[arg(long, default_value = "pilot")]
    study_id: String,

    /// Write the plan JSON. Parent directories are created.
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Profile {
    Pilot,
    Release,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchRound {
    id: String,
    /// Natural user turn shown verbatim to the solver in both arms.
    prompt: String,
    /// Evaluator-only routing label. Never included in the solver prompt.
    #[serde(default)]
    expected_scout: bool,
    /// Evaluator-only localization signal. Never included in the solver prompt.
    #[serde(default)]
    expected_paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceSource {
    Verifier,
    Patch,
    Trajectory,
    FinalResponse,
    RepositoryState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RubricItem {
    id: String,
    criterion: String,
    weight: u8,
    #[serde(default)]
    required: bool,
    evidence: Vec<EvidenceSource>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Verification {
    #[serde(default)]
    commands: Vec<String>,
    #[serde(default)]
    manual_checks: Vec<String>,
}

impl Verification {
    fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.manual_checks.is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchTask {
    #[serde(default = "schema_version_one")]
    schema_version: u8,
    #[serde(default = "default_suite")]
    suite: String,
    id: String,
    /// Legacy one-turn task. Version 3 tasks use rounds instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    /// Ordered turns sent to one persistent solver session and worktree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rounds: Vec<BenchRound>,
    /// Legacy task-level routing label.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    expected_scout: bool,
    /// Legacy task-level localization signal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    expected_paths: Vec<String>,
    /// Hidden weighted quality rubric. Never included in solver prompts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rubric: Vec<RubricItem>,
    /// Hidden behavioral verification. Never included in solver prompts.
    #[serde(default, skip_serializing_if = "Verification::is_empty")]
    verification: Verification,
    /// Evaluator-only provenance. Never included in the solver prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<serde_json::Value>,
    /// False until provenance, verifier, and blind review are release-ready.
    #[serde(default)]
    headline_eligible: bool,
}

impl BenchTask {
    fn prompts(&self) -> Vec<&str> {
        if self.rounds.is_empty() {
            self.prompt.iter().map(String::as_str).collect()
        } else {
            self.rounds
                .iter()
                .map(|round| round.prompt.as_str())
                .collect()
        }
    }

    fn validate(&self, path: &Path) -> anyhow::Result<()> {
        if self.id.trim().is_empty() || self.suite.trim().is_empty() {
            bail!("{} has an empty id or suite", path.display());
        }

        match self.schema_version {
            1 | 2 => {
                if self
                    .prompt
                    .as_deref()
                    .is_none_or(|prompt| prompt.trim().is_empty())
                    || !self.rounds.is_empty()
                {
                    bail!(
                        "{} legacy task must contain one prompt and no rounds",
                        path.display()
                    );
                }
            }
            3 => {
                if self.prompt.is_some() || self.rounds.is_empty() {
                    bail!(
                        "{} schema 3 task must contain rounds instead of prompt",
                        path.display()
                    );
                }
                if self.suite == "mah-swe" && self.rounds.len() < 2 {
                    bail!(
                        "{} MAH-SWE task must have at least two rounds",
                        path.display()
                    );
                }
                if self.source.is_none() {
                    bail!("{} schema 3 task is missing provenance", path.display());
                }
                if self.rubric.is_empty() || self.verification.is_empty() {
                    bail!(
                        "{} schema 3 task needs a rubric and behavioral verification",
                        path.display()
                    );
                }
            }
            version => bail!("{} uses unsupported task schema {version}", path.display()),
        }

        let mut round_ids = HashSet::new();
        for round in &self.rounds {
            if round.id.trim().is_empty() || round.prompt.trim().is_empty() {
                bail!("{} has an empty round id or prompt", path.display());
            }
            if !round_ids.insert(round.id.as_str()) {
                bail!("{} has duplicate round id: {}", path.display(), round.id);
            }
        }

        let mut rubric_ids = HashSet::new();
        let mut total_weight = 0_u16;
        for item in &self.rubric {
            if item.id.trim().is_empty()
                || item.criterion.trim().is_empty()
                || item.weight == 0
                || item.evidence.is_empty()
            {
                bail!("{} has an incomplete rubric item", path.display());
            }
            if !rubric_ids.insert(item.id.as_str()) {
                bail!("{} has duplicate rubric id: {}", path.display(), item.id);
            }
            total_weight += u16::from(item.weight);
        }
        if !self.rubric.is_empty() && total_weight != 100 {
            bail!(
                "{} rubric weights total {total_weight}, expected 100",
                path.display()
            );
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Arm {
    Baseline,
    Repotracer,
}

#[derive(Debug, Serialize)]
struct PlannedTrial {
    pair_id: String,
    task_id: String,
    repeat: usize,
    arm: Arm,
    order: usize,
    prompt_sha256: String,
    review_id: String,
}

#[derive(Debug, Serialize)]
struct ReviewScore {
    rubric_id: String,
    criterion: String,
    points_available: u8,
    required: bool,
    points_awarded: Option<u8>,
    passed: Option<bool>,
    evidence: Vec<String>,
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReviewForm {
    schema_version: u8,
    review_id: String,
    task_id: String,
    artifact_bundle: String,
    blinded_fields: [&'static str; 5],
    scores: Vec<ReviewScore>,
    overall_score_100: Option<u8>,
    required_items_passed: Option<bool>,
    quality_defects: Vec<String>,
    reviewer_confidence: Option<String>,
    locked_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReviewPolicy {
    blinding: &'static str,
    quality: &'static str,
    invalid_pair: &'static str,
    uncertainty: &'static str,
}

#[derive(Debug, Serialize)]
struct DecisionGate {
    priority: [&'static str; 3],
    minimum_independent_tasks: usize,
    minimum_repeats_per_arm: usize,
    maximum_quality_regression_points: f32,
    target_median_cost_reduction_percent: f32,
    maximum_median_wall_time_regression_percent: f32,
    hard_failure_policy: &'static str,
    confidence_rule: &'static str,
}

#[derive(Debug, Serialize)]
struct Plan {
    schema_version: u8,
    plan_schema: &'static str,
    review_schema: &'static str,
    study_id: String,
    generated_at: String,
    status: &'static str,
    profile: Profile,
    suite_root: PathBuf,
    task_suite: Option<String>,
    arms: [Arm; 2],
    repeats_per_arm: usize,
    task_count: usize,
    trial_count: usize,
    planned_round_count: usize,
    tasks: Vec<BenchTask>,
    trials: Vec<PlannedTrial>,
    result_layout: &'static str,
    methodology: &'static str,
    review_policy: ReviewPolicy,
    decision_gate: DecisionGate,
}

fn schema_version_one() -> u8 {
    1
}

fn default_suite() -> String {
    "legacy-routing".into()
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

fn prompt_sha256(task: &BenchTask) -> String {
    let mut digest = Sha256::new();
    for prompt in task.prompts() {
        digest.update((prompt.len() as u64).to_be_bytes());
        digest.update(prompt.as_bytes());
    }
    hex(&digest.finalize())
}

fn arm_name(arm: Arm) -> &'static str {
    match arm {
        Arm::Baseline => "baseline",
        Arm::Repotracer => "repotracer",
    }
}

fn review_id(study_id: &str, task_id: &str, repeat: usize, arm: Arm) -> String {
    let digest =
        Sha256::digest(format!("{study_id}\0{task_id}\0{repeat}\0{}", arm_name(arm)).as_bytes());
    format!("review-{}", hex(&digest[..8]))
}

fn first_arm(study_id: &str, task_id: &str, repeat: usize) -> Arm {
    let seed = Sha256::digest(format!("{study_id}\0{task_id}").as_bytes());
    if (usize::from(seed[0]) + repeat - 1).is_multiple_of(2) {
        Arm::Baseline
    } else {
        Arm::Repotracer
    }
}

fn build_trials(tasks: &[BenchTask], repeats: usize, study_id: &str) -> Vec<PlannedTrial> {
    let mut pairs: Vec<_> = tasks
        .iter()
        .flat_map(|task| {
            (1..=repeats).map(move |repeat| {
                let key = Sha256::digest(format!("{study_id}\0{}\0{repeat}", task.id).as_bytes());
                (key.to_vec(), task, repeat)
            })
        })
        .collect();
    pairs.sort_by(|left, right| left.0.cmp(&right.0));

    let mut trials = Vec::with_capacity(pairs.len() * 2);
    for (_, task, repeat) in pairs {
        let first = first_arm(study_id, &task.id, repeat);
        let second = if first == Arm::Baseline {
            Arm::Repotracer
        } else {
            Arm::Baseline
        };
        let pair_id = format!("{}-r{repeat}", task.id);
        let prompt_sha256 = prompt_sha256(task);
        for arm in [first, second] {
            trials.push(PlannedTrial {
                pair_id: pair_id.clone(),
                task_id: task.id.clone(),
                repeat,
                arm,
                order: trials.len() + 1,
                prompt_sha256: prompt_sha256.clone(),
                review_id: review_id(study_id, &task.id, repeat, arm),
            });
        }
    }
    trials
}
fn review_form(task: &BenchTask, trial: &PlannedTrial) -> ReviewForm {
    ReviewForm {
        schema_version: 1,
        review_id: trial.review_id.clone(),
        task_id: trial.task_id.clone(),
        artifact_bundle: format!("private/blinded/{}", trial.review_id),
        blinded_fields: ["arm", "routing", "cost", "latency", "repeat"],
        scores: task
            .rubric
            .iter()
            .map(|item| ReviewScore {
                rubric_id: item.id.clone(),
                criterion: item.criterion.clone(),
                points_available: item.weight,
                required: item.required,
                points_awarded: None,
                passed: None,
                evidence: Vec::new(),
                note: None,
            })
            .collect(),
        overall_score_100: None,
        required_items_passed: None,
        quality_defects: Vec::new(),
        reviewer_confidence: None,
        locked_at: None,
    }
}

fn write_review_forms(out: &Path, plan: &Plan) -> anyhow::Result<()> {
    let Some(parent) = out.parent().filter(|path| !path.as_os_str().is_empty()) else {
        return Ok(());
    };
    let reviews = parent.join("reviews");

    for trial in &plan.trials {
        let task = plan
            .tasks
            .iter()
            .find(|task| task.id == trial.task_id)
            .expect("planned trial references a selected task");
        if task.rubric.is_empty() {
            continue;
        }
        std::fs::create_dir_all(&reviews)?;
        let form = review_form(task, trial);
        std::fs::write(
            reviews.join(format!("{}.json", trial.review_id)),
            format!("{}\n", serde_json::to_string_pretty(&form)?),
        )?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let repeats = args.repeats.unwrap_or(match args.profile {
        Profile::Pilot => 1,
        Profile::Release => 3,
    });
    if repeats == 0 {
        bail!("--repeats must be at least 1");
    }

    let tasks_dir = args.suite.join("tasks");
    let mut tasks = Vec::new();
    if tasks_dir.is_dir() {
        for entry in std::fs::read_dir(&tasks_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                let task: BenchTask = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
                task.validate(&path)?;
                tasks.push(task);
            }
        }
    }
    tasks.sort_by(|left, right| left.id.cmp(&right.id));

    let mut ids = HashSet::new();
    if let Some(duplicate) = tasks.iter().find(|task| !ids.insert(task.id.as_str())) {
        bail!("duplicate task id: {}", duplicate.id);
    }

    if let Some(task_suite) = &args.task_suite {
        tasks.retain(|task| &task.suite == task_suite);
    }
    if !args.task.is_empty() {
        let available: HashSet<_> = tasks.iter().map(|task| task.id.as_str()).collect();
        if let Some(missing) = args.task.iter().find(|id| !available.contains(id.as_str())) {
            bail!("unknown task id in selected suite: {missing}");
        }
        let requested: HashSet<_> = args.task.iter().map(String::as_str).collect();
        tasks.retain(|task| requested.contains(task.id.as_str()));
    }
    if tasks.is_empty() {
        bail!("no benchmark tasks selected");
    }

    if args.profile == Profile::Release {
        if repeats < 3 {
            bail!("release plans require at least 3 repeats per arm");
        }
        if tasks.len() < 30 {
            bail!("release plans require at least 30 independent tasks");
        }
        if let Some(task) = tasks.iter().find(|task| !task.headline_eligible) {
            bail!("release task is not headline eligible: {}", task.id);
        }
    }

    let trials = build_trials(&tasks, repeats, &args.study_id);
    let planned_round_count = tasks
        .iter()
        .map(BenchTask::prompts)
        .map(|prompts| prompts.len())
        .sum::<usize>()
        * repeats
        * 2;
    let plan = Plan {
        schema_version: 3,
        plan_schema: "benchmarks/results/plan-v3.schema.json",
        review_schema: "benchmarks/results/review-v1.schema.json",
        study_id: args.study_id,
        generated_at: Utc::now().to_rfc3339(),
        status: "planned",
        profile: args.profile,
        suite_root: args.suite,
        task_suite: args.task_suite,
        arms: [Arm::Baseline, Arm::Repotracer],
        repeats_per_arm: repeats,
        task_count: tasks.len(),
        trial_count: trials.len(),
        planned_round_count,
        tasks,
        trials,
        result_layout: "benchmarks/results/runs/<study-id>/{manifest.json,trials/,reviews/,summary.json,private/}",
        methodology: "Each pair uses the same repository state and ordered user turns. One isolated solver session and worktree persist across a trial's rounds; only RepoTracer availability changes between arms.",
        review_policy: ReviewPolicy {
            blinding: "Mask arm labels, routing events, cost, and latency until rubric scoring is locked.",
            quality: "Run behavioral verifiers first, then score each rubric item from patch, repository state, trajectory, and final response evidence.",
            invalid_pair: "Provider, harness, or verifier infrastructure failures invalidate and resample both arms; agent timeouts and context exhaustion count as failures.",
            uncertainty: "Report paired deltas with a 95% task-cluster bootstrap that keeps every task's repeats together.",
        },
        decision_gate: DecisionGate {
            priority: ["quality", "complete_provider_cost_usd", "wall_seconds"],
            minimum_independent_tasks: 30,
            minimum_repeats_per_arm: 3,
            maximum_quality_regression_points: 5.0,
            target_median_cost_reduction_percent: 60.0,
            maximum_median_wall_time_regression_percent: 20.0,
            hard_failure_policy: "No new security, data-loss, or required-behavior failure is acceptable.",
            confidence_rule: "Using the 95% task-cluster bootstrap: quality lower bound must be at least -5 points, cost-reduction lower bound must exceed 0%, and wall-time upper bound must not exceed +20%.",
        },
    };
    let text = serde_json::to_string_pretty(&plan)?;

    if let Some(out) = args.out {
        if let Some(parent) = out.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        write_review_forms(&out, &plan)?;
        std::fs::write(out, format!("{text}\n"))?;
    } else {
        println!("{text}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multi_round_task() -> BenchTask {
        serde_json::from_str(
            r#"{
                "schema_version": 3,
                "suite": "mah-swe",
                "id": "session",
                "rounds": [
                    {"id":"implement","prompt":"Fix it","expected_scout":true},
                    {"id":"review","prompt":"Review the real path","expected_scout":true}
                ],
                "rubric": [
                    {"id":"behavior","criterion":"The fix works","weight":100,"required":true,"evidence":["verifier"]}
                ],
                "verification": {"commands":["cargo test -p crate"]},
                "source": {"kind":"real-task"},
                "headline_eligible": false
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn legacy_tasks_keep_safe_defaults() {
        let task: BenchTask = serde_json::from_str(
            r#"{"id":"legacy","prompt":"Fix it","expected_scout":true,"expected_paths":[]}"#,
        )
        .unwrap();

        assert_eq!(task.schema_version, 1);
        assert_eq!(task.suite, "legacy-routing");
        assert!(!task.headline_eligible);
        task.validate(Path::new("legacy.json")).unwrap();
    }

    #[test]
    fn paired_multi_round_trials_share_prompts_and_alternate_order() {
        let task = multi_round_task();
        task.validate(Path::new("session.json")).unwrap();
        let trials = build_trials(std::slice::from_ref(&task), 3, "study");

        assert_eq!(trials.len(), 6);
        assert_eq!(task.prompts(), ["Fix it", "Review the real path"]);
        for repeat in 1..=3 {
            let pair: Vec<_> = trials
                .iter()
                .filter(|trial| trial.repeat == repeat)
                .collect();
            assert_eq!(pair.len(), 2);
            assert_ne!(pair[0].arm, pair[1].arm);
            assert_eq!(pair[0].prompt_sha256, pair[1].prompt_sha256);
        }
        assert_ne!(
            first_arm("study", "session", 1),
            first_arm("study", "session", 2)
        );
    }

    #[test]
    fn multi_round_rubric_must_total_one_hundred() {
        let mut task = multi_round_task();
        task.rubric[0].weight = 99;
        let error = task.validate(Path::new("session.json")).unwrap_err();

        assert!(error.to_string().contains("weights total 99"));
    }

    #[test]
    fn review_form_excludes_arm_and_economics() {
        let task = multi_round_task();
        let trial = build_trials(std::slice::from_ref(&task), 1, "study")
            .into_iter()
            .next()
            .unwrap();
        let form = serde_json::to_value(review_form(&task, &trial)).unwrap();

        for hidden in ["arm", "routing", "cost", "latency", "repeat"] {
            assert!(form.get(hidden).is_none());
        }
        assert_eq!(form["review_id"], trial.review_id);
        assert_eq!(form["scores"][0]["points_available"], 100);
        assert!(form["scores"][0]["points_awarded"].is_null());
    }
}
