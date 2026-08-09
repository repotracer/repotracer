# Benchmarks

## Rule

**If the invoice didn't shrink, don't call it token savings.**

We benchmark complete coding tasks:

| Metric | Why |
|--------|-----|
| Total provider cost | The bill |
| Main input / cache / output tokens | Frontier usage |
| Explorer cost / latency | Scout overhead (local or paid) |
| Turns | Control-loop waste |
| Wall-clock | UX |
| Task success | Quality gate |

We do **not** publish:

- “tokens avoided” by truncating tool output
- unpaired single runs as headlines
- quality-blind cost wins

## Methodology

See [docs/benchmarks/why-token-counters-lie.md](./docs/benchmarks/why-token-counters-lie.md).

Paired arms:

- **A** — coding agent alone
- **B** — same agent + grephound

Hold constant: model, reasoning setting, repo commit, task, agent version, timeout.

Modes (never mixed):

- **Natural** — tool available; agent decides
- **Forced** — exploration tasks instructed to use `repo_scout`

## Reproduce

```bash
cargo run -p grephound-bench -- --suite benchmarks
```

Raw results live under `benchmarks/results/`.

## Status

Harness scaffold ships in 0.1.0. Headline Δ numbers appear only after repeated paired runs.
