# Claude Code and Codex integration

RepoTracer can install `repo_scout` for Claude Code, Codex, or both.

Each parent has its own investigator profile, so changing the Codex setup does not silently change Claude Code and vice versa.

## Requirements

Install the native CLI you want RepoTracer to integrate with:

- Codex: `codex`
- Claude Code: `claude`

Sign in through the native CLI. RepoTracer reuses that session; it does not copy provider credentials into a second login system.

Node.js 18 or newer is required for the `npx` launcher.

## Setup

```bash
npx repotracer@latest setup
```

On a first install, the wizard can configure Claude Code, Codex, or both.

Run the same command later to reconfigure or uninstall.

For explicit non-interactive selection:

```bash
npx repotracer@latest setup --agents codex
npx repotracer@latest setup --agents claude
npx repotracer@latest setup --agents both --dry-run
```

`--dry-run` previews the changes without writing them.

Restart the parent agent after changing its MCP configuration.

## Investigator models

Each parent can use its own native investigator model.

The current defaults in the release configuration are:

| Parent | Native investigator |
|---|---|
| Codex | `gpt-5.6-luna` |
| Claude Code | `opus` with automatic reasoning |

These are setup defaults, not guarantees about cost, speed, or quality on every repository.

Use `settings` to change them:

```bash
npx repotracer@latest settings
```

For scripted mappings:

```bash
npx repotracer@latest settings --agents both \
  --codex-scout codex --codex-model gpt-5.6-luna \
  --claude-scout claude --claude-model opus
```

Native model catalogs and accepted reasoning settings come from the installed CLI and can change independently of RepoTracer.

## What gets installed

RepoTracer registers an MCP server with the selected parent and adds the instructions that tell the parent when `repo_scout` is useful.

The parent still decides whether to call the tool. The managed guidance asks it to
send the objective, relevant requirements and questions the investigation must
answer, work on independent parts while waiting, and continue from the returned
evidence. It adds no hook or enforced investigation limit.

The [guidance studies](benchmarks/guidance-rewrite/README.md) distinguish the
original 76-word pilot candidate from the later native comparison of this exact
132-word revision, including its mixed cost, time and quality results.

A focused edit can stay direct:

```text
"Rename this variable in src/config.ts"
  → Claude Code / Codex
```

A broad investigation can delegate:

```text
"Trace why refresh tokens fail after rotation"
  → repo_scout
```

The current routing test records 42/42 correct decisions.

## Native investigator behavior

A native investigator runs through the Claude Code or Codex CLI you already use.

It starts in a separate investigation conversation. It may trace behavior across files or related repositories, run checks or experiments, and report what it found.

RepoTracer does not turn the native CLI into a read-only sandbox. The investigator is instructed to investigate rather than modify the product, but capabilities available through the underlying CLI can remain available.

The repository is a starting location, not a guaranteed operating-system filesystem boundary.

If you need stricter execution or filesystem controls, configure them in the native CLI environment itself.

## Continuing an investigation

Related follow-ups can continue the same investigation instead of starting over.

The caller can pass the previous conversation handle with the follow-up request. Independent questions should start independent investigations.

A continuation keeps useful model context, but it is not a snapshot of the repository. Files can change between calls, so the investigator should re-check current code when that matters.

## Results and source

The investigator returns a model-written report. It can include findings, source, checks or experiment results, unresolved questions, and limitations.

When RepoTracer attaches structured source ranges, it checks that the cited location resolves. That is a location check, not proof that the investigator's conclusion is correct.

A source attachment can fail without making the whole investigation useless; the result keeps the report and records the delivery problem.

## If the investigator cannot settle the task

It should say so.

A useful partial report tells the coding agent what was found, what was checked, and what remains unresolved. Claude Code or Codex can continue from that work rather than repeating the same investigation from scratch.

## Custom OpenAI-compatible investigator

You can also point RepoTracer at an OpenAI-compatible endpoint:

```bash
npx repotracer@latest setup \
  --base-url http://localhost:11434/v1 \
  --model deepseek-coder
```

Set `REPOTRACER_API_KEY` when authentication is required.

This path uses RepoTracer's own repository-tool loop. It does not automatically inherit the native Claude Code or Codex execution environment.

## Timeouts

Warm native provider processes may be kept for related work.

- `session.idle_secs` retires an inactive retained process. It is not a total request timeout.
- `model.timeout_ms` controls native stream inactivity when configured.
- `explorer.timeout_seconds` is a whole-investigation limit for the generic OpenAI-compatible engine, not native Claude Code or Codex investigations.

## Verification

GitHub CI covers the Rust workspace and supported platforms, including the Codex native integration checks used by the repository.

Live Claude Code checks require a signed-in `claude` CLI and remain an opt-in local smoke test. See [CONTRIBUTING.md](../CONTRIBUTING.md).

For implementation details, see [ARCHITECTURE.md](./ARCHITECTURE.md).
