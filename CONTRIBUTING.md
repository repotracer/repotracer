# Contributing

```bash
git clone https://github.com/grephound/grephound
cd grephound
cargo test --workspace
cargo run -p grephound -- doctor
cargo run -p grephound -- scout "where is config loaded?" --mock
```

CI must pass without GPU, Ollama, or API keys (mock backend).

## Good first areas

- Agent integrations
- Windows path edge cases
- Benchmark tasks + verifiers
- Doctor diagnostics
- Docs clarity

## Style

- Prefer boring Rust
- No new crates for one struct
- Tests for path security and concurrency regressions
