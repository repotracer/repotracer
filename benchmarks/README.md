# benchmarks/

- `tasks/` — natural prompts plus evaluator-only provenance, routing labels, and expected paths
- `runners/` — paired agent runners; the treatment changes tool availability, never the prompt
- `results/` — the canonical home for every benchmark result
- `results/runs/<study-id>/manifest.json` — protocol, task selection, arms, and trial inventory
- `results/runs/<study-id>/trials/` — one immutable artifact set per task, repeat, and arm
- `results/runs/<study-id>/summary.json` — derived aggregates; never a substitute for trial artifacts
- `results/run.schema.json` — required manifest and trial fields for new runs
- `results/index.json` and `results/SHA256SUMS` — legacy artifact inventory and checksum ledger

Keep private session exports and raw trajectories under each run's ignored `private/` directory. Commit sanitized manifests, grader output, summaries, and SHA-256 digests. Every new run declares its schema version, status, protocol/model settings, task provenance, quality method, complete main-plus-scout cost inputs, wall time, decision, and limitations. Never delete a losing run or rewrite a completed trial.

Luna subscription comparisons use $0.20/M uncached input, $0.02/M cached reads, and $1.20/M output including reasoning. Cache writes are not counted for subscription benchmarks.

## Rust build storage

Never run plain `cargo build` inside every isolated benchmark checkout: each checkout creates another large `target/` tree. Build every source arm through the shared wrapper; it reuses one target directory and snapshots only the requested binary per arm.

```bash
./benchmarks/runners/build-rust-arm.sh baseline repotracer -p repotracer
./benchmarks/runners/build-rust-arm.sh candidate repotracer -p repotracer
```

The default shared cache is `$XDG_CACHE_HOME/repotracer/benchmarks` or `~/.cache/repotracer/benchmarks`. Override it with `REPOTRACER_BENCH_CACHE_DIR`. Set `REPOTRACER_BENCH_PROFILE=release` when passing `--release`.

```bash
# Validate every task and print a one-repeat paired plan.
cargo run -p repotracer-bench -- --suite benchmarks

# Initialize a low-compute private diagnostic plan (two model sessions total).
cargo run -p repotracer-bench -- \
  --task session-recurring-problems \
  --study-id 2026-08-23-session-diagnostic \
  --out benchmarks/results/runs/2026-08-23-session-diagnostic/manifest.json

(cd benchmarks/results && sha256sum -c SHA256SUMS)
```
