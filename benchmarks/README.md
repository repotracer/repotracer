# benchmarks/

- `tasks/` — task definitions (query + quality expectations)
- `runners/` — agent runners (Claude/Codex) — wire when keys available
- `results/` — dated paired run artifacts

```bash
cargo run -p grephound-bench -- --suite benchmarks
```
