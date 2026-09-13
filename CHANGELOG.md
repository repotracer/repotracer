# Changelog

This file records behavior at the time each release shipped. Older entries can describe constraints that no longer apply to the current architecture.

## [Unreleased]

## [2.1.1] - 2026-09-13

### Added

- `repotracer benchmarks` configures native Codex and Claude Code comparisons, tracks detached jobs, and shows costs, tokens, time, and blind quality grades. Public Python commands support the same workflow for agents. ([#21](https://github.com/repotracer/repotracer/pull/21))
- Benchmark results can launch a separate investigation of worse cost, time, or quality. Custom-task patches can be applied explicitly when the original checkout is unchanged. ([#21](https://github.com/repotracer/repotracer/pull/21))

### Changed

- `repotracer update` and automatic updates follow npm's `latest` version, then verify and install that exact GitHub release. Unpublished GitHub releases no longer trigger updates. ([#21](https://github.com/repotracer/repotracer/pull/21))
- Codex's managed `mcp_servers.repotracer.tool_timeout_sec` now uses a finite
  compatibility allowance and upgrades the previous managed `600`-second value
  without changing other MCP entries or custom timeout values. ([#20](https://github.com/repotracer/repotracer/pull/20))
- `repo_scout` advertises the discovered reasoning effort levels for the
  configured native model. The recommended Luna model offers medium through
  max. The recommended Opus model starts Auto at low and can automatically
  escalate only to medium. ([#20](https://github.com/repotracer/repotracer/pull/20))

### Fixed

- Setup and update migrate the old `explorer.max_turns = 6` setting to `0`, including installed Codex and Claude profiles, so investigations can finish their reports. Other turn limits are preserved, and migrated files retain a backup. ([#21](https://github.com/repotracer/repotracer/pull/21))

[Unreleased]: https://github.com/repotracer/repotracer/compare/v2.1.1...HEAD
[2.1.1]: https://github.com/repotracer/repotracer/compare/v2.1.0...v2.1.1

## 2.1.0 — 2026-09-13

The first release since 1.0.1, and the largest so far. Version 2.0.0 was used during development but never published, so this entry covers everything since 1.0.1.

RepoTracer started this cycle as a Codex search helper. It ships as a repo investigator for Claude Code and Codex: a separate agent traces the repository, runs checks when they help, and reports back with source, results, and anything it could not settle.

### Highlights

- **Claude Code support.** Install RepoTracer for Claude Code, Codex, or both. Each keeps its own investigator profile.
- **Investigations instead of file lookups.** `repo_scout` can trace behavior across files and related repositories, run checks or experiments, and report what it found, what it checked, and what is still unresolved.
- **Follow-ups continue the same investigation.** A related call picks up the earlier work, even when it moves into another repository. Independent investigations run in parallel.
- **Warm native sessions.** Investigator processes stay up between related calls, so follow-ups skip CLI start-up.
- **A new setup and settings wizard.** Pick agents, models, reasoning effort, and the Codex fast tier from one full-screen terminal UI, and uninstall from the same place.
- **Bring any model.** Choose from the native model catalogs, enter a `provider:model-id`, or point at any OpenAI-compatible endpoint.
- **`repotracer symbols`.** Definitions, references, and outlines for Rust, Python, JavaScript, TypeScript/TSX, and Go, with no model call.

### Added

**Agents and models**

- Claude Code parent integration alongside Codex. `setup --agents codex|claude|both` selects them without the wizard.
- Independent investigator profiles per parent, so changing the Codex setup never changes Claude Code.
- Native model discovery from the installed CLIs, a searchable model catalog, `provider:model-id` entries, and a Custom model form for OpenAI-compatible endpoints (base URL, model ID, optional API key).
- Defaults: `codex:gpt-5.6-luna` on the fast tier for Codex, and `claude:opus` with automatic reasoning for Claude Code.
- `repotracer settings`, interactive or scripted with `--agents`, `--codex-scout`, `--codex-model`, `--claude-scout`, `--claude-model`, and `--dry-run`.
- A Codex fast-tier setting, on by default for `gpt-5.6-luna` and off for other Codex models.

**Investigations**

- A new investigation engine. Reports carry a written answer, selected source, experiment results, unresolved questions, limitations, usage, and timing.
- Investigations can span several repository targets and keep citations to paths outside the starting repository.
- Continuation through `investigation.conversation_id`. Calls on one conversation run in order; separate conversations run concurrently.
- Optional `repository`, `focus`, and `investigation` inputs on `repo_scout`, and investigation intents on the CLI (`scout --intent`).
- An investigator can ask for one follow-up pass at a higher reasoning effort when that would settle a specific gap.
- Native Claude Code investigations with subscription usage accounting.
- Warm native sessions. Retained Codex app-server and Claude Code processes are reused for related work and retired after `session.idle_secs`.
- Source attachments are checked against the file and delivered as bounded ranges. A failed attachment is reported next to the answer instead of discarding the report.
- The `repo_scout` result format is versioned as handoff v4, and failed investigations are returned as MCP tool errors.

**Setup wizard**

- A full-screen terminal wizard for setup and settings. The card sizes to the page, with a step rail, per-page titles, and a wordmark where there is room.
- Model, reasoning effort, and fast tier are each a row you arrow onto. Enter opens a setting and Space flips a toggle.
- `Custom model…` leads the model picker.
- A first install opens with Codex and Claude Code both selected.
- Uninstall from the wizard: uncheck an agent, or press Uninstall (`R`) to stage every installed agent. Rows read "will be removed" before anything changes.
- Catalog warnings name the command that fixes them.
- Works without color (`NO_COLOR`) and on terminals without UTF-8 (`REPOTRACER_ASCII=1`).

**Tooling and release**

- `doctor` checks Claude Code authentication in the same environment the Claude investigator uses.
- `scripts/verify-release.sh` checks packaging and integration in a throwaway home directory, including the published package with `--published`.
- `scripts/subscription-smoke.py` runs `repo_scout` end to end with a real Codex or Claude Code CLI behind it.
- CI on Linux, macOS, and Windows with rustfmt, strict Clippy, the native Codex app-server checks, and a repository hygiene check. Pre-commit hooks for contributors.
- The npm launcher installs the platform-native binary for macOS, Linux, and Windows.

### Changed

- Routing: when a task needs an investigation, the parent calls `repo_scout` straight away instead of searching on its own first.
- Timeouts are separate settings: `session.idle_secs` for warm-process idle time, `model.timeout_ms` for stream silence, and `explorer.timeout_seconds` for a whole investigation on the OpenAI-compatible engine only. A zero tool timeout disables the limit.
- OpenAI-compatible endpoints leave reasoning effort unset unless you choose one; the medium default applies only to native investigators. Reasoning requests use completion-token limits.
- `service_tier` is written only to Codex profiles. Claude Code and custom profiles no longer carry it, and profiles written before this change behave as they did.
- Building from source requires Rust 1.90 or newer.
- The docs describe source validation as a location check, not proof that a conclusion is correct.

### Fixed

- Cancelling or retiring an investigation kills the whole native process tree, including helper processes Claude Code starts. On Windows, a job object owns Claude Code's child processes.
- Setup keeps the first backup of your config instead of overwriting it on every auto-update.
- An inline `mcp_servers` entry is detected as installed.
- Profile saves are atomic on every platform, and saved API keys are tied to the provider origin they were entered for.
- A turn with unreported usage no longer disables warm reuse for its conversation. Missing usage is marked partial instead of estimated.
- Claude Code failures quote the end of its stderr, so a renamed flag is distinguishable from a stream that ended early.
- Repository boundaries hold through path aliases, generic tools stay bound to the requested repository, and nested reads and searches for dash-prefixed text work.
- Follow-ups on one conversation can no longer overtake each other while a repository is being selected.
- Citation source is read in bounded, streamed ranges instead of loading whole files.
- Symbols indexes are kept across requests, and paging can no longer stall on an empty page.
- Tool telemetry is kept when a native turn fails.
- Custom provider discovery settings are preserved, and verified discovery results appear in the wizard.
- Workspace context refreshes when a native investigation changes targets, and retained scratch directories no longer collide when processes are recycled.
- Successful integration removals persist even if a later removal fails, and uninstall state survives a rerun of the wizard.
- Self-update picks the correct upgrade target version.

### Upgrading from 1.0.1

- Run `npx repotracer@latest setup` to add Claude Code or change models, then restart the parent agent.
- Anything that parses raw `repo_scout` output should expect handoff v4.

### Verification

Workspace tests, Clippy, and rustfmt pass. Release tags gate published artifacts on GitHub CI across Linux, macOS, and Windows.

This verification note is not a benchmark claim.

## 1.0.1 — 2026-08-30

### Fixed

- Setup no longer treats unrelated `repotracer` text left in Codex configuration as an installed MCP integration after uninstall.

## 1.0.0 — 2026-08-30

### Changed

- GPT-5.6 Luna investigators use the fast service tier by default.
- Routing classifies the requested ownership surface before planning broad searches. Localized changes start with targeted work; exhaustive or cross-owner tasks can call RepoTracer first.

### Fixed

- Subscription investigations treat `model.timeout_ms` as a stream-silence limit.
- Luna accepts every reasoning level exposed by the native backend.

## 0.1.9 — 2026-08-26

### Fixed

- Setup no longer requires an active Codex login. It verifies that Codex is installed and defers authentication until an investigation runs.

## 0.1.8 — 2026-08-26

### Changed

- Automatic update checks now run whenever the RepoTracer MCP server starts instead of at most once every 24 hours.
- Codex investigations preserve active model-provider and authentication settings while avoiding unrelated inherited configuration from that release's isolated environment.

## 0.1.7 — 2026-08-26

### Fixed

- Codex subscription investigations restore the user's Windows `unelevated` sandbox setting when starting an app-server session.
- CI exercises the RepoTracer → Codex app-server → repository-read path on Linux, macOS, and Windows.

## 0.1.6 — 2026-08-26

### Added

- RepoTracer can update the binary installed under `~/.repotracer/bin`, verify the release checksum, and refresh managed integration files.
- Added `repotracer update`.
- Automatic updates default to on and can be disabled with configuration or `REPOTRACER_NO_UPDATE=1`.

### Changed

- Codex subscription investigations moved from `codex exec` to `codex app-server`.

### Removed

- Removed the old update notice from `repo_scout` results.

## 0.1.5 — 2026-08-25

### Fixed

- Tiny repositories and localized single-owner changes skip `repo_scout`; broad cross-component work can still call it first.
- Long-running investigations no longer use a total timeout by default.

## 0.1.4 — 2026-08-25

### Added

- Added release-update information to the old `repo_scout` handoff path.
- Release notes began using tag annotations rather than autogenerated change text.

## 0.1.3 — 2026-08-25

### Fixed

- `setup` no longer scans the working directory or runs a live repository diagnosis.
- Model path-escape detection handles POSIX and drive-letter paths independently of the host platform.

### Changed

- Setup output was shortened.

## 0.1.2 — 2026-08-24

### Added

- Expanded the routing benchmark suite.
- Added a tamper-evident benchmark index and SHA-256 ledger.

### Changed

- `setup` ran `doctor` itself instead of only printing the command.
- Product support was limited to Codex while other hosts lacked equivalent end-to-end evidence.

### Fixed

- Codex investigation sessions narrowed inherited capabilities in the v0.1 architecture.
- Luna reasoning became configurable and defaulted to medium.
- Benchmark prompts were ordinary user prompts; routing labels remained evaluator-only.
- Setup became zero-question and GPT-only for that release.

## 0.1.1 — 2026-08-24

### Added

- Existing installs gained an update/uninstall menu.

### Fixed

- Documentation stopped calling the managed Codex instructions a “routing skill.”
- The npm launcher stopped falling back to an arbitrary `repotracer` on `PATH`.

## 0.1.0 — 2026-08-08

### Added

- Rust investigation engine.
- Read / Glob / Grep repository tools.
- Concurrent tool execution.
- `rg --count-matches` support.
- Citation parsing and source-location validation.
- OpenAI-compatible model backend and deterministic mock backend.
- CLI commands for scouting, MCP serving, setup, diagnostics, status, configuration, and uninstall.
- `repo_scout` over MCP stdio.
- Initial coding-agent integration work.
- npm launcher scaffold.
- Benchmark harness and methodology docs.
