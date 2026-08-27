# benchmarks/

- `tasks/` — versioned task prompts plus evaluator-only provenance, routing labels, rubrics, and verification
- `runners/` — paired agent runners; the treatment changes tool availability, never the prompt
- `results/` — the canonical home for every benchmark result
- `results/runs/<study-id>/manifest.json` — preregistered task selection, randomized pairs, and trial inventory
- `results/runs/<study-id>/trials/` — one immutable artifact set per task, repeat, and arm
- `results/runs/<study-id>/reviews/` — generated blind review forms keyed by opaque review ID
- `results/runs/<study-id>/summary.json` — derived aggregates; never a substitute for trial artifacts
- `results/plan-v3.schema.json` — current repeated and multi-round plan schema
- `results/review-v1.schema.json` — blind manual quality review schema
- `results/run.schema.json` — legacy version 2 completed-run schema
- `results/index.json` and `results/SHA256SUMS` — legacy artifact inventory and checksum ledger

Keep private session exports, arm mappings given to the coordinator, and raw trajectories under each run's ignored `private/` directory. Commit sanitized manifests, locked review output, summaries, and SHA-256 digests. Every run declares task provenance, complete main-plus-scout cost, wall time, quality evidence, decision, and limitations. Never delete a losing run or rewrite a completed trial.

Use provider billing records for cost. Luna subscription comparisons use $0.20/M uncached input, $0.02/M cached reads, and $1.20/M output including reasoning. Cache writes are not counted for subscription benchmarks.

## Rust build storage

Never run plain `cargo build` inside every isolated benchmark checkout: each checkout creates another large `target/` tree. Build every source arm through the shared wrapper; it reuses one target directory and snapshots only the requested binary per arm.

```bash
./benchmarks/runners/build-rust-arm.sh baseline repotracer -p repotracer
./benchmarks/runners/build-rust-arm.sh candidate repotracer -p repotracer
```

The default shared cache is `$XDG_CACHE_HOME/repotracer/benchmarks` or `~/.cache/repotracer/benchmarks`. Override it with `REPOTRACER_BENCH_CACHE_DIR`. Set `REPOTRACER_BENCH_PROFILE=release` when passing `--release`.

```bash
# Validate every task and print a one-repeat paired pilot plan.
cargo run -p repotracer-bench -- --suite benchmarks

# Plan three repeats of every MAH-SWE task. Ordered rounds stay in one solver
# session and worktree. This also writes blinded forms under reviews/.
cargo run -p repotracer-bench -- \
  --task-suite mah-swe \
  --repeats 3 \
  --study-id 2026-08-27-mah-swe \
  --out benchmarks/results/runs/2026-08-27-mah-swe/manifest.json

# The release profile refuses fewer than 30 independent tasks, fewer than three
# repeats per arm, or any task not explicitly marked headline-eligible.
cargo run -p repotracer-bench -- \
  --profile release \
  --study-id release-candidate \
  --out benchmarks/results/runs/release-candidate/manifest.json

(cd benchmarks/results && sha256sum -c SHA256SUMS)
```

## Evaluation lanes

1. **Formal repository tasks:** run a contamination-resistant public suite such as DeepSWE for comparable pass/fail capability evidence. Use SWE-Interact when the research question is session continuity. Do not mix those scores with MAH-SWE.
2. **MAH-SWE:** original or private real-work tasks with two or more ordered user turns, hidden behavioral verification, and a 100-point task rubric. One solver session and worktree persist through every round.

Both lanes compare paired baseline and RepoTracer arms. Randomization changes task-pair order and which arm runs first. Behavioral verification runs before quality review. Reviewers receive only opaque review bundles and forms; arm, routing, cost, latency, and repeat stay hidden until scores lock. Infrastructure failures invalidate and resample both arms. Agent timeouts and context exhaustion count as failures.

The release gate is quality first, complete provider cost second, and wall time third: no new hard failure, at most a 5-point quality regression, a 60% median cost-reduction target, and at most a 20% median wall-time regression. Report paired deltas and a 95% task-cluster bootstrap; keep all repeats from one task in the same resampled cluster.
