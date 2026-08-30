<p align="center">
  <img src="assets/logo-lockup-stacked.svg" alt="RepoTracer" width="260">
</p>

<h3 align="center">Make your Codex subscription last up to 2.7x longer.</h3>

<p align="center">
  RepoTracer searches your repository with a cheaper model through an MCP tool call, not a prompt instruction. Codex spends your budget writing code instead of finding it.
</p>

<p align="center">
  <a href="#install"><img src="assets/button-install.svg" alt="Install RepoTracer" height="38"></a>&nbsp;
  <a href="./BENCHMARKS.md"><img src="assets/button-benchmarks.svg" alt="Benchmarks" height="38"></a>&nbsp;
  <a href="https://repotracer.tech"><img src="assets/button-website.svg" alt="repotracer.tech" height="38"></a>
</p>

<p align="center">
  <img src="assets/demo/scout.gif" alt="Asking RepoTracer a plain-English question about a codebase and getting back four verified file and line-range citations with a summary in 35 seconds" width="100%">
</p>

## The problem

Codex subscriptions have a fixed monthly budget. Every time Codex searches your repository, reading files, grepping for symbols, mapping dependencies, it burns that budget on work that writes zero lines of code.

On the tasks we measured, search ate 30-60% of the total cost. That budget could have gone toward actual edits.

RepoTracer moves search to Luna, a model that costs a fraction of Sol, through an MCP tool call. Same code gets written. Your subscription lasts longer.

## What you get

**Codex calls a tool, not a suggestion.** `repo_scout(query)` is an MCP tool call. Codex can't ignore it, reinterpret it, or go Google how to do it. It calls the function, gets results.

**Cheap search, not cheap quality.** Each scout runs in its own thread with just the query and read-only access. No conversation history dragged along. Costs a fraction of a full subagent.

**No hallucinated paths.** Every file path and line range gets checked before Codex sees it. If Luna returns a file that doesn't exist, RepoTracer drops it.

**Knows when to do nothing.** 42/42 routing decisions correct. If the edit target is already obvious, no scout runs. You don't pay for a search you don't need.

**One command.** `npx repotracer@latest setup`. No API keys, no background service, no Rust toolchain. Uses your existing Codex login.

**Updates itself.** New versions download and verify automatically when Codex starts. Nothing to maintain.

## Install

```bash
npx repotracer@latest setup
```

`setup` downloads the native binary, verifies its SHA-256 checksum, installs it under `~/.repotracer/bin`, registers the MCP server, and adds a managed routing block to `~/.codex/AGENTS.md`. It reuses your existing Codex login.

No Rust toolchain, second API key, background service, or hand-written config is required.

**Don't want more limits and faster outputs for some reason?** Run the same command again and pick *Uninstall*:

```bash
npx repotracer@latest setup
```

```text
RepoTracer is already configured.
Use arrow keys, then Enter. Esc to cancel.

  > Update the configuration
    Uninstall RepoTracer
```

That removes the MCP entry, the routing block, and the local config. Your Codex login and settings are untouched, and every file it edits is backed up alongside the original first. `repotracer uninstall --yes` does the same thing without the menu.

Preview the changes:

```bash
npx repotracer@latest setup --dry-run
```

Want the `repotracer` command in your own shell too?

```bash
npm install -g repotracer
# or
cargo install --git https://github.com/repotracer/repotracer --locked repotracer
```

## Updating

RepoTracer updates itself. Each time its MCP server starts, it checks the release
feed, verifies the new binary's SHA-256 against the published checksums, and
replaces the copy in `~/.repotracer/bin`. The new version takes effect the next
time you start Codex.

It only ever replaces that one binary, then refreshes RepoTracer's managed MCP
entry and `AGENTS.md` block. A `cargo install` build or source checkout is left
alone.

Automatic updates are on by default. Restart Codex after an update for the new
binary and managed integration files to take effect. To disable automatic
updates, set `updates.automatic = false` in `~/.repotracer/config.toml` or set
`REPOTRACER_NO_UPDATE=1`. You can still update a disabled installation with
`npx repotracer@latest setup`.

To update on the spot:

```bash
repotracer update
```

## How it works

```text
You ask Codex to fix something
  Codex decides it needs to find code first
  calls repo_scout(query) via MCP
    RepoTracer spins up an isolated Luna thread
    Luna searches with Read, Glob, Grep
    RepoTracer validates every path and line range
  Codex gets back verified file:line citations
  Codex reads the cited code and makes the edit
```

The scout runs GPT-5.6 Luna at medium reasoning on the fast service tier. It starts clean, no conversation history, no inherited context, and can only read. It cannot edit, delete, commit, or push.

One tool call. Structured output. Verified results. No prompt interpretation, no web searches for "how to start a Luna agent."

## "Can't I just put 'use Luna' in agents.md?"

You can try. We did. Here's what happens.

Sol doesn't have to follow prompt instructions. It interprets them however it wants, or ignores them, or goes and searches the web for "how to start a Luna agent" instead of starting one. An MCP tool call is a function call. Sol calls it, gets back results, moves on.

Subagents also inherit the full conversation context. On a long session that inherited context alone can cost more than the search was supposed to save. RepoTracer starts an isolated thread with only the query and read-only tools. No history.

And raw Luna output is inconsistent. Without validation you get wrong paths, bad line numbers, phantom files. RepoTracer checks every citation before returning it. Bad results get dropped.

We spent weeks benchmarking both approaches. The one-line agents.md instruction consistently cost more than not using it at all. The MCP approach is the one that actually showed up in the numbers.

## Benchmarks

Tested on DeepSWE (industry-standard multi-language coding tasks) and MAH-SWE (a benchmark built from real agentic coding sessions on production repositories, not synthetic prompts).

Complete task cost, RepoTracer's usage included.

| Task | Source | Cost | Budget stretch | Quality |
|---|---|---:|---:|---|
| Real bug fix, production repo | MAH-SWE | −62.68% | **2.68x** | Both arms worked |
| SWE-bench Astropy 13453 | SWE-bench | −50.12% | **2.00x** | Regression passed |
| Release benchmark (TS, Python, Go) | DeepSWE | −27.71% | **1.38x** | 147/151 features |
| Median of 3 paired runs | DeepSWE | −39.20% | **1.64x** | 6/6 checks every arm |

Budget stretch means how many times you can run the same task on a fixed budget. A task that costs 62.68% less fits 1 / 0.3732 = 2.68 times. That's not the same number as the cost reduction.

Paired runs hold everything constant: same model, same prompt, same repo commit, same timeout. The only difference is whether RepoTracer is installed.

42/42 routing decisions correct. 24/24 holdout. 3/3 real-task runs passed both verifiers. 87 workspace tests passing, formatting and strict Clippy clean.

Every run is published with raw artifacts. [Methods, caveats, full data.](./BENCHMARKS.md)

## When it kicks in

Not every task needs a scout. The routing is tuned and benchmarked, 42/42 decisions correct.

Scout runs when Codex needs to:
- Find code across multiple files
- Trace callers, dependencies, or exported APIs
- Navigate an unfamiliar repository
- Locate tests and fixtures for a behavior

Scout stays out of the way when:
- The edit target is already known
- The task is a single-file change
- Codex already has the file open

Small edits stay fast.

## Current support

| Item | Current value |
|---|---|
| Coding agent | Codex |
| Interface | MCP and CLI |
| Default scout | `gpt-5.6-luna` |
| Scout service tier | `fast` |
| Scout reasoning | `medium` |
| Scout tools | Read, Glob, Grep |
| Repository writes | Disabled |
| Custom backend | OpenAI-compatible GPT endpoint |

Luna medium remains the default because it passed the bounded reasoning trial. High reasoning missed the production timeout in all three bounded runs and scored 5/6 in the extended run.

## CLI

```bash
repotracer "where is auth handled?"      # scout the current repository
repotracer scout "trace refresh rotation"
repotracer serve                         # MCP over stdio
repotracer setup
repotracer update                         # replace the binary with the newest release
repotracer doctor
repotracer status
repotracer config --init
repotracer benchmark
repotracer uninstall --yes
```

`--json` is available on `scout`, `doctor`, and `status`.

### Custom GPT endpoint

```bash
repotracer \
  --base-url https://models.example.com/v1 \
  --model gpt-5.6-mini \
  setup
```

Set `REPOTRACER_API_KEY` when the endpoint requires authentication.

## Security

- Read-only repository access
- Repository-root and symlink-escape checks
- Returned path and line-range validation
- No default telemetry
- Provider credentials remain with the official CLI

See [SECURITY.md](./SECURITY.md).

## Resources

- [repotracer.tech](https://repotracer.tech)
- [Benchmarks](./BENCHMARKS.md)
- [Architecture](./docs/ARCHITECTURE.md)
- [Why complete-task measurement matters](./docs/benchmarks/why-token-counters-lie.md)
- [Microsoft FastContext paper](https://arxiv.org/abs/2606.14066v3)

RepoTracer is an independent project inspired by FastContext. It is not affiliated with or endorsed by Microsoft. See [NOTICE](./NOTICE).

## Develop

```bash
cargo test --workspace
cargo run -p repotracer -- doctor
cargo run -p repotracer -- scout "where is config loaded?" --mock
```

## License

MIT. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
