# Integration status

RepoTracer supports Claude Code and Codex as parent coding agents, with independent investigator profiles for each.

This page tracks integration behavior and verification. It does not make benchmark claims; see [BENCHMARKS.md](../BENCHMARKS.md) for those.

## Supported

| Area | Status |
|---|---|
| Codex parent | Supported |
| Claude Code parent | Supported |
| Configure both together | Supported |
| Independent parent profiles | Supported |
| Native model discovery | Supported |
| Custom OpenAI-compatible investigator | Supported |
| Investigation continuation | Supported |
| Independent parallel investigations | Supported |
| Structured source attachments | Supported |
| Text fallback | Supported |

The native parent CLI is still required for native investigations.

## Current setup defaults

| Parent | Investigator |
|---|---|
| Codex | `gpt-5.6-luna` |
| Claude Code | `opus` with automatic reasoning |

These are defaults, not a promise that they are available to every account or that they are optimal for every repository.

## Verification

The release pipeline covers the Rust workspace and npm launcher on the supported CI platforms.

Codex native integration checks run in CI where the Codex CLI is available.

Claude Code end-to-end checks require a signed-in `claude` session and therefore remain an opt-in local smoke test:

```bash
cargo build -p repotracer

python3 scripts/subscription-smoke.py \
  --binary target/debug/repotracer \
  --output /tmp/repotracer-smoke \
  --backend claude-cli
```

See [CONTRIBUTING.md](../CONTRIBUTING.md) for the full smoke-test workflow.

## Native investigator behavior

Native investigations run through the user's Claude Code or Codex CLI environment.

RepoTracer does not enforce a read-only sandbox around those CLIs. The investigator is instructed to investigate, but it may use capabilities available in the underlying environment, including useful checks or experiments.

The repository is a starting location rather than a hard filesystem boundary. Related repositories or external paths can be part of an investigation when needed.

Temporary artifacts created during an investigation may persist.

## Result handling

A result can be complete, partial, not found, or failed.

Partial and uncertain investigations should preserve useful work: what was found, what was checked, source or experiment results, and what remains unresolved.

Structured source ranges are checked for location validity when they are attached. That check does not prove the model-written conclusion.

Source attachment failures can be reported separately from the investigation itself so a useful report is not thrown away.

## Timeouts and sessions

`session.idle_secs` controls idle retention of a warm native process.

`model.timeout_ms` controls native stream inactivity when configured.

`explorer.timeout_seconds` applies only to the generic OpenAI-compatible investigation loop.

These settings do not describe the native model's internal turn or tool limits.

## External dependencies

RepoTracer cannot guarantee behavior controlled by the provider or parent CLI, including:

- model availability
- account or subscription limits
- native CLI flags
- upstream permission behavior
- provider usage reporting
- parent-agent routing decisions

When a native CLI changes, RepoTracer may need an integration update even when the MCP protocol itself has not changed.
