# Contributing

```bash
git clone https://github.com/repotracer/repotracer
cd repotracer
cargo test --workspace
cargo run -p repotracer -- doctor
cargo run -p repotracer -- scout "where is config loaded?" --mock
```

CI must pass without a GPU, a local model runtime, or API keys — use the mock backend.

## Before publishing

```bash
scripts/verify-release.sh              # local build
scripts/verify-release.sh --published  # what npx actually serves
```

Installs into a throwaway HOME, drives the MCP server over stdio the way Codex
does, makes one live scout call, checks every returned citation resolves, then
uninstalls. Your real `~/.codex` is never touched. Costs about a cent of Codex
quota for the one live call.

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
