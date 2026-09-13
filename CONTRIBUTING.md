# Contributing

Thanks for contributing to RepoTracer.

For small fixes, feel free to open a PR directly. For a larger behavior change, new integration, or protocol change, opening an issue first can save both sides from implementing different designs.

## Setup

```bash
git clone https://github.com/repotracer/repotracer
cd repotracer

cargo test --workspace
cargo run -p repotracer -- doctor
cargo run -p repotracer -- scout "where is config loaded?" --mock
```

The normal test suite must run without a GPU, local model runtime, or API key. Use the deterministic mock backend for CI-safe tests.

## Before opening a PR

Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Add a regression test when a bug can reasonably be reproduced in the test suite.

If the change affects setup, native provider sessions, MCP results, continuation, or release packaging, also run the relevant checks below.

## Native CLI smoke tests

Mock tests cover RepoTracer's own control flow. They cannot prove that a current Claude Code or Codex CLI still behaves the way the integration expects.

### Codex app-server

`scripts/codex-app-server-smoke.py` exercises the Codex app-server protocol and can run in CI where the Codex CLI is available.

### End-to-end `repo_scout`

`scripts/subscription-smoke.py` starts the MCP server over stdio and puts a real native CLI behind `repo_scout`.

```bash
cargo build -p repotracer

python3 scripts/subscription-smoke.py \
  --binary target/debug/repotracer \
  --output /tmp/repotracer-smoke
```

For Claude Code:

```bash
python3 scripts/subscription-smoke.py \
  --binary target/debug/repotracer \
  --output /tmp/repotracer-smoke \
  --backend claude-cli
```

Useful options:

- `--model` selects a different native model.
- `--native-executable` points at a specific CLI or wrapper.
- `--impact-effort` changes the reasoning effort for the change-impact case.

Run the end-to-end smoke test after changes to native sessions, conversation continuation, MCP result formatting, source attachment handling, or either native adapter.

It uses real provider capacity, so it is intentionally not part of the default test path.

## Release checks

Before publishing:

```bash
scripts/verify-release.sh
```

To verify what the published package actually serves:

```bash
scripts/verify-release.sh --published
```

The verifier uses a throwaway home directory rather than modifying your real Claude Code or Codex configuration.

A passing release check proves packaging and integration behavior. It is not evidence for a cost, speed, or quality claim.

## Benchmark changes

Keep benchmark results inspectable.

A retained comparison should include the prompt, repository/environment, model settings, direct and RepoTracer runs, investigator usage, the quality check, raw outputs, and checksums.

Do not report a local token reduction as a RepoTracer saving. The assisted total includes the coding agent **and** investigator.

See [BENCHMARKS.md](./BENCHMARKS.md) and [Measure the complete task](./docs/benchmarks/why-token-counters-lie.md).

## Documentation

Describe current behavior, not an older architecture.

Native Claude Code / Codex investigations and the OpenAI-compatible backend have different capabilities. Keep that distinction intact and tie benchmark claims to published artifacts.

See [Architecture](./docs/ARCHITECTURE.md) for the current runtime model.

## Good areas to help

- Claude Code and Codex integrations
- Windows path and process behavior
- continuation and concurrency edge cases
- benchmark tasks and verifiers
- doctor diagnostics
- documentation

## Rust style

Prefer boring Rust.

Avoid adding a dependency for a small amount of code. Add focused tests for path handling, cancellation, result formatting, and concurrency when a change touches those areas.
