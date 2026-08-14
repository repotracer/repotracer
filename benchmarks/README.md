# benchmarks/

- `tasks/` — natural user prompts plus evaluator-only routing labels and expected paths
- `runners/` — paired agent runners; the natural arm changes tool availability, never the user prompt
- `results/` — dated raw evidence, including rejected and failed candidates
- `results/index.json` — standardized artifact inventory, pricing profile, headline gate, decisions, and SHA-256 digests
- `results/SHA256SUMS` — checksum ledger for proving artifacts were not silently rewritten
- `results/synthesis.json` — stable presentation-ready tables derived from the raw artifacts

Every new result must preserve raw runs and declare `schema_version`, `date`, `status`, protocol/model settings, quality method, complete cost inputs, wall time, decision, and limitations. Never delete a losing run. Only `index.json` decides whether evidence is headline-eligible.

Luna subscription comparisons use $0.20/M uncached input, $0.02/M cached reads, and $1.20/M output including reasoning. Cache writes are not counted for subscription benchmarks.

## Rust build storage

Never run plain `cargo build` inside every isolated benchmark checkout: each checkout creates another large `target/` tree. Build every source arm through the shared wrapper; it reuses one target directory and snapshots only the requested binary per arm.

```bash
./benchmarks/runners/build-rust-arm.sh baseline grephound -p grephound
./benchmarks/runners/build-rust-arm.sh candidate grephound -p grephound
```

The default shared cache is `$XDG_CACHE_HOME/grephound/benchmarks` or `~/.cache/grephound/benchmarks`. Override it with `GREPHOUND_BENCH_CACHE_DIR`. Set `GREPHOUND_BENCH_PROFILE=release` when passing `--release`.

```bash
cargo run -p grephound-bench -- --suite benchmarks
(cd benchmarks/results && sha256sum -c SHA256SUMS)
```
