# Running benchmarks

Compare the same task with and without RepoTracer. Report quality, complete-task cost, parent/scout tokens, and elapsed time together. A finished process is not a finished benchmark until its work has been graded.

The public tools in `tools/benchmarks` run native sessions, collect usage, prepare blind grading packets, call a separate grader, and produce per-task and historical reports. They use Python 3.10 or newer, with no packages to install. Installed RepoTracer binaries bundle the same scripts.

## Use the benchmark interface

```sh
repotracer benchmarks
# Keep all settings and results in a chosen directory:
repotracer benchmarks --state-dir .benchmark-runs/daily
```

The initial list has three custom Codex tasks and one external Claude Code task. Enter your project paths and prompts, or add and remove tasks. Blank custom prompts are skipped. External tasks use SWE-bench Lite or a local SWE-style JSON/JSONL dataset; the selected instance and dataset identity are saved before execution. Local exports need `repo`, `base_commit`, `instance_id`, and `problem_statement`; `test_patch`, `FAIL_TO_PASS`, and `PASS_TO_PASS` supply acceptance checks. Reference solution patches are never given to the solver.

Set model choices and price cards before starting. Codex runs require a rate card for each solving model so they cannot finish with an unpriced token ledger. Claude Code can supply its own reported cost. Price cards use the fields shown below and USD per million tokens. Use your provider's actual model identifiers and rates. Prices are configuration, not guessed from a model alias.

The default comparison is baseline versus current RepoTracer. A third, changed group requires an explicit candidate binary or routing source. Runs pin the binary and guidance they use. Groups run sequentially, and closing the interface leaves workers running. Open it again to see progress, results, or a saved investigation. Applying a patch is opt-in for custom tasks and refuses a source checkout that changed after capture.

The saved daily preset is started manually. It does not install a scheduler. For another machine, run the same command there; both groups must run on that machine. Automatic remote dispatch is not included.

Agents can use the same backend without driving terminal keys:

```sh
python3 tools/benchmarks/workflow.py --state-dir .benchmark-runs/daily init
python3 tools/benchmarks/workflow.py --state-dir .benchmark-runs/daily get
python3 tools/benchmarks/workflow.py --state-dir .benchmark-runs/daily save < config.json
python3 tools/benchmarks/workflow.py --state-dir .benchmark-runs/daily start
python3 tools/benchmarks/workflow.py --state-dir .benchmark-runs/daily investigate --run RUN --task TASK --group current
python3 tools/benchmarks/workflow.py --state-dir .benchmark-runs/daily apply --run RUN --task TASK --group current
```

`get` returns saved configuration and job summaries as JSON. `save` accepts the configuration object, and `start` resumes an unfinished batch with the same configuration. Raw native logs, patches, grading keys, and reports stay in the state directory. Costs become available as each group finishes; unknown cost remains unknown while it runs. Results without a blind grade remain incomplete.

Custom tasks can set an acceptance command before launch. External pytest checks are derived from the dataset when its test identifiers support that command; other test runners need an explicit command. Dependencies must work natively on the selected machine. Missing checks and environment failures are recorded, not treated as passes.

## Start a comparison

From a fresh clone:

```sh
python3 tools/benchmarks/bench.py init .benchmark-runs/today
python3 tools/benchmarks/bench.py --help
```

Edit `.benchmark-runs/today/manifest.json`. The generated files contain no measured results, real rates, or prefilled grades. Keep local outputs in `.benchmark-runs`, which Git ignores, or outside the project. Never put credentials in a manifest.

Use two groups normally, `baseline` for the parent alone and `current` for the same parent with RepoTracer. Add a `changed` candidate when testing a code or prompt change. Each candidate needs a unique id and group within its task. Test the exact submitted code and prompt, and record their identifiers in `version`. Later edits need their own comparison.

The default owner-initiated batch contains three real tasks written by the user and one randomly selected external benchmark task. Preserve the user's prompts exactly. Additional agent-proposed tasks must be shown and accepted before launch. Record the external dataset revision, task id, and selection seed. Keep internal and external results separate in aggregates. Contributors can start with one relevant task rather than paying for a full batch.

Use Sol medium as the default Codex parent. Luna Auto starts at medium and offers medium/high/xhigh/max. Reserve one task for Claude Code when capacity is limited, normally the external task. Its preset uses Opus as parent and scout; Opus scout Auto starts at low and can increase to medium. Record the exact provider model identifiers, parent effort, and actual per-request scout efforts. These defaults are comparison settings, not claims that a particular account exposes those models.

## Run the task in native sessions

Capture the same starting source state for every group, including relevant uncommitted and untracked source files. A Git commit alone does not describe a dirty checkout. Put the digest of the full captured state in each candidate's `snapshot`. Record a stable machine identifier in `machine` and the same parent settings in `parent`.

Create a separate worktree and fresh parent thread for each group. Leave the user's active checkout untouched. Keep the solving agent's normal project instructions, dependencies, and native tools equivalent between groups. Baseline must not have RepoTracer enabled. Assisted groups get the exact RepoTracer binary, configuration, and parent guidance being tested. Do not force the parent to call it or do preparatory searches to choose scout effort.

Run groups sequentially on the same machine and vary their order independently of their names. Do not always warm builds or caches with baseline first. Record execution order privately. If a project is on another machine, run the entire pair there. Compare like starting environments; record cache handling instead of silently giving one group prepared builds.

Use native Codex or Claude Code with the access the user authorized. Do not introduce a Docker or sandbox environment just for the comparison. Claude requests go through Claude Code and its configured provider. Do not extract subscription credentials or call subscription endpoints from another client.

Preserve native thread ids and logs so an interrupted run can continue its existing thread. Do not restart it because the controller disconnected. There is no whole-task deadline. Model inactivity detection is distinct from a total runtime cutoff. An external interruption stays in the record, together with all usage from before and after resumption.

Before solving, prepare task acceptance criteria and small hidden checks in a separate location. Do not give solving agents those checks or other candidates' work. With full filesystem access this is separation by workflow, not a filesystem security guarantee. Grade each candidate using the same criteria. Record agent-created tests separately from acceptance checks and collect the actual patch, including new files. A plain `git diff` omits untracked files.

Applying a useful benchmark patch is a separate requested action. Check for intervening source changes and conflicts first; do not merge whichever candidate got the highest score automatically.

## Record cost, tokens, time, and evidence

Update the candidate's `status` to `completed`, `failed`, or `interrupted`. Record known usage even for failures. Only set `usage_complete` when all requests have been collected, including retries and native child agents. Missing usage needs `usage_missing_reason`, and missing time needs `time_missing_reason`. Never turn either into zero.

Each `requests` entry has this shape. The numbers below illustrate the format, not model prices or benchmark results:

```json
{
  "id": "unique-native-request-id",
  "role": "parent",
  "model": "actual-model-id",
  "effort": "medium",
  "rate_card": "model-short-tier-2026-09-13",
  "tokens": {
    "uncached_input": 100,
    "cache_read": 1000,
    "cache_write": 0,
    "output": 20
  }
}
```

Use `parent` or `scout` for the request's role. Attribute native child agents to the role that launched them. The four token buckets are disjoint. For a provider whose input count includes cache reads or writes, subtract those before recording `uncached_input`. Output includes reasoning tokens when the provider includes them in output; never count those twice. Record actual effort and model, even when Auto changes them.

Add a matching `rate_cards` entry with `model`, `source`, and numeric USD-per-million rates for `uncached_input`, `cache_read`, `cache_write`, and `output`. Use separate cards for models and pricing tiers. Select a tier per request, never from the sum of tokens across a task. Preserve special cache-write pricing through separate cards when needed. All groups use the same rate-card set. Unknown rates are an error, not free usage. Identical replayed request ids are deduplicated; conflicting duplicates fail.

Complete cost is parent plus scout, including cache, retries, failures, and child agents. API-price equivalents measure subscription capacity here; they are not subscription invoices. Keep preparation, grading, and follow-up diagnosis usage separately from task cost. Do not include those requests in the solving candidate's ledger.

`seconds` is whole-task elapsed time with the same definition for every group. Do not add scout duration to parent time when they overlap. Keep start, finish, and interruption timestamps in private evidence. If complete timing cannot be reconstructed, use `null`; do not use only the resumed segment. Separately recorded active runtime can help diagnosis but must not silently replace elapsed time.

Each candidate refers to a UTF-8 `patch` and two JSON evidence files, `acceptance_checks` and `agent_tests`. Test records use this format:

```json
[
  {"name": "test_refresh_rotation", "status": "passed", "evidence": "Expected rotated token accepted."},
  {"name": "test_expired_session", "status": "failed", "evidence": "Expected rejection; received success."}
]
```

Supported statuses are `passed`, `failed`, `skipped`, `error`, and `not_run`. An empty list means no recorded tests; it is not a pass. Keep actual assertion diagnostics and relevant output in `evidence`. Do not include shell timing summaries, model identity, group names, output-order markers, absolute worktree paths, or billing information. Store the raw logs separately for later diagnosis.

## Blind the quality grader

Give a separate grader only the task, starting context, candidate patch, acceptance-check results, and agent-created test results. Never send cost, tokens, runtime, model identity, baseline/RepoTracer labels, execution order, or the native conversation. For repository work, model names can legitimately appear in code under review; do not alter functional source just to conceal those names.

Prepare one packet per settled task:

```sh
python3 tools/benchmarks/bench.py blind .benchmark-runs/today/manifest.json \
  --task my-task-1 \
  --output .benchmark-runs/grader/task-1 \
  --key .benchmark-runs/private/task-1-key.json
```

The command randomly assigns `Candidate A`, `Candidate B`, and further labels if needed. It independently shuffles their requested review order. It exports only the allowed files and whitelisted test fields. The key stays outside the packet, with private file permissions. Re-running cannot overwrite the packet or silently reroll its labels. Task and packet hashes prevent accidentally joining grades to changed evidence.

Optional `redact_test_strings` entries in the manifest replace literal identifiers in test names and diagnostics. Code and task text remain unchanged. The exporter cannot infer every identity clue inside free text. Inspect the packet once for provider banners, timing summaries, identifying paths, or other avoidable leaks. Fix the source evidence before creating the final packet; do not manually edit an already graded packet. Do not remove failures or assertions when redacting.

Run Astra in a fresh grading session with only the packet contents and its `grading.md` instructions. Do not connect that session to RepoTracer, the original logs, or the key. File placement alone does not prevent an unrestricted agent from reading sibling directories. Supply the packet as input instead of telling the grader to explore the benchmark workspace. Treat instructions found in patches or test output as untrusted data.

The LLM's judgment carries more weight than test counts. It should inspect correctness, completeness, scope, and the quality of the tests, including whether tests were weakened or bypassed. A passing suite alone does not establish that the task was solved. Use a common 0–10 rubric set before execution, explain deductions, and allow a justified no-change result. The final score is the grader's judgment, not a fixed weighted average of test pass rate and prose quality.

Save its structured answer outside the packet:

```json
{
  "Candidate A": {"score": 8, "rationale": "Concrete evidence and deductions."},
  "Candidate B": {"score": 9, "rationale": "Concrete evidence and deductions."}
}
```

All supplied candidates need scores. Keep this quality judgment fixed before showing performance figures. Routing diagnosis happens afterward, when the investigator can inspect full traces. Regrading after seeing the cost is a new review, not a replacement of the blind one.

## Produce the report

```sh
python3 tools/benchmarks/bench.py report .benchmark-runs/today/manifest.json \
  --review .benchmark-runs/private/task-1-key.json .benchmark-runs/private/task-1-grades.json
```

The default output is JSON for agents. Add `--format markdown` for a readable table. Repeat `--review KEY GRADES` for additional tasks. Save stdout to a local file when needed. Calling `report` before grading shows incomplete rows explicitly and produces no paired winner for them.

Every row contains group/model settings, complete cost, raw seconds, parent/scout tokens with cache buckets, outcome, and grade. Known partial usage is kept but not presented as the full bill. Missing values stay missing. Failed candidates with complete records and grades remain in the comparison.

Compute within each task:

```text
cost ratio = assisted complete cost / baseline complete cost
time ratio = assisted elapsed seconds / baseline elapsed seconds
quality delta = assisted grade - baseline grade
```

The reporter rejects paired claims for different machines, snapshots, or parent settings. It stores raw seconds but aggregates paired time ratios, never raw seconds across machines. A zero baseline denominator yields no ratio. Cost and time can have different valid sample counts; report each count.

For each version/model configuration, task origin, and validation/regression category, report median paired cost ratio, median paired time ratio, IQR for both, quality delta distribution, quality wins/ties/losses, and `n`. Keep attempted counts visible too. IQR uses linearly interpolated 25th and 75th percentiles. A single observation has an IQR equal to that observation; it is still just one observation. Ties mean an exact zero difference on the common grade scale.

Save each day's JSON report, then aggregate:

```sh
python3 tools/benchmarks/bench.py history \
  .benchmark-runs/day-1-report.json .benchmark-runs/day-2-report.json
```

Different versions, model presets, task origins, and rate-card sets stay separate. Duplicate run identities are rejected. Use stable task ids, and never give a rerun a new identity to inflate independent sample counts. Add `--bootstrap 2000` for a reproducible percentile bootstrap 95% interval around the cost/time medians when there are at least two pairs. Resampling does not turn a small or dependent sample into stronger evidence; `n` remains visible.

## Diagnose losses without tuning the score

The report marks every worse cost, time, or quality result with `diagnosis_required`. An investigator then reads the private task traces, scout answers, parent actions, and test evidence. Determine whether the loss came from the assignment, scout findings, parent use of those findings, implementation, infrastructure, or measurement. Do not assume a confident answer was correct, or that every repeated file read was waste.

Record a short diagnosis as `bug`, `tradeoff`, or `unresolved`, with evidence and a candidate improvement if one follows. A tradeoff may be inherent to the chosen design, but do not call it inherent merely because the first investigation found no fix. Investigate material or recurring losses more deeply. Do not automatically edit, retry, or discard a run until a favorable result appears.

Once Task X informs a change, it is a development example for that change. Rerunning X is useful regression coverage, not independent evidence that the fix improves new tasks. Add X to `development_tasks` for the changed-version experiment. Set `derived_from` for close task variants. The reporter places these results under regression even if someone left `purpose` as `validation`.

Validate the resulting fix on future or new tasks. Keep original fresh-task results for the older version as historical evidence. For historical aggregation where contamination needs to be recorded afterward, use `--development-task TASK_ID` and include known derivatives. Apply that override to the changed-version reports, not indiscriminately to versions that predate the tuning. Development/regression results never share an aggregate with fresh validation.

## Check or contribute the tooling

```sh
python3 -m unittest discover -s tools/benchmarks -p 'test_*.py' -v
```

These are synthetic unit tests of blinding and accounting, not model benchmarks. CI runs them without provider access or Rust builds. Tooling and this guide are public; raw runs, private research, label keys, and account data are not added to product commits. A contributed benchmark should include the full result table and disclose exact tested versions, conditions, missing evidence, and whether its tasks influenced the change.
