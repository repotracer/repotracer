# Architecture

RepoTracer separates **repository investigation** from **implementation**.

Claude Code or Codex keeps the coding task. When it needs help understanding the repository, it calls `repo_scout`. The investigator traces the relevant behavior in a separate conversation, can run useful checks or experiments, and reports back with findings, source, results, and anything it could not resolve.

```text
Claude Code / Codex
        │
        │ repo_scout
        ▼
    RepoTracer
        │
        ▼
 investigator conversation
   trace / inspect / test
        │
        ▼
 findings + source + results + unresolved parts
        │
        ▼
Claude Code / Codex
   implement + verify
```

The coding agent can use that report directly, inspect any remaining gaps, and continue with the change.

## Two investigator paths

RepoTracer supports native Claude Code / Codex investigators and OpenAI-compatible model endpoints. They do not have the same runtime.

### Native Claude Code and Codex

Native investigations run through the CLI the user already has installed and signed into.

Each `repo_scout` call starts a new investigation or continues an existing one. The repository is the starting point, not a hard operating-system boundary. A native investigator may follow relevant code into related repositories or paths, and it may run checks or create temporary experiment artifacts when that helps answer the question.

The investigator is instructed to investigate rather than modify the product, but RepoTracer does **not** turn the native CLI into a read-only security sandbox. Capabilities available to the native CLI can remain available to the investigator.

That distinction is important:

- do not assume native investigations are limited to Read / Grep / Glob
- do not assume every file mentioned in prose was validated
- do not assume temporary investigation artifacts are deleted automatically
- do not treat RepoTracer as an operating-system sandbox

Use the permission and sandbox controls of the underlying Claude Code or Codex installation for anything security-sensitive.

### OpenAI-compatible endpoints

Custom OpenAI-compatible models use RepoTracer's own model/tool loop instead of inheriting the native Claude Code or Codex environment.

At a high level:

```text
query
  → model
  → repository tool calls
  → tool results
  → model
  → final report
```

RepoTracer provides the repository tools for this path and validates their arguments before execution. This backend does not automatically gain the native CLI's execution environment or capabilities.

Configure it with a base URL and model ID:

```bash
npx repotracer@latest setup \
  --base-url http://localhost:11434/v1 \
  --model deepseek-coder
```

Set `REPOTRACER_API_KEY` when the endpoint requires authentication.

## Investigations and continuation

An investigation has its own conversation outside the coding agent's main conversation.

A related `repo_scout` call can continue the same investigation instead of asking a fresh model to rediscover the repository. Independent investigations can run in parallel.

Continuation is useful when the next question depends on work the investigator already did:

```text
1. Trace how refresh rotation works.
2. Continue that investigation: where can the rotated token be rejected?
3. Continue again: does the mobile client use the same path?
```

A continuation can also move into a related repository when the investigation requires it. The caller should still provide the current objective and any requirements that matter; the investigator does not inherit the coding agent's full conversation.

## When the investigator is not sure

A hard investigation does not have to collapse into a confident guess.

The report can preserve:

- what the investigator found
- what it checked or ran
- the source it selected
- results from experiments or checks
- unresolved questions
- limitations or missing evidence

The coding agent can continue from that work. A partial investigation is still useful if it prevents the parent from repeating the same search from zero.

## MCP result

`repo_scout` returns a model-written report plus structured metadata when available.

The report can include findings, selected source, experiment results, unresolved questions, usage, timing, and continuation information.

Structured source attachments may contain `path:start-end` ranges and source text. RepoTracer checks the attachment location before returning it. That check answers:

> Does this source location exist and match the requested range?

It does **not** answer:

> Is the investigator's conclusion true?

Source delivery and claim correctness are separate.

A source attachment can also fail while the rest of the report remains useful. RepoTracer preserves the answer and records the attachment problem instead of discarding the entire investigation.

## Parent responsibilities

RepoTracer investigates. Claude Code or Codex still owns the coding task.

The parent can:

- use the returned findings directly
- inspect anything still unclear
- edit files
- run its own verification
- call `repo_scout` again for a related or independent question

It does not have to reopen every cited file before it can act, though important changes should still be verified with the repository's normal tests and checks.

## Routing

RepoTracer is not meant to run on every prompt.

```text
"Rename this variable in src/config.ts"
  → parent handles it directly

"Trace why refresh tokens fail after rotation"
  → repo_scout
```

The current routing suite records **42/42 correct decisions**.

Routing is instruction-driven rather than a security boundary. The parent agent ultimately decides whether to call the MCP tool.

## Components

The repository is split into small Rust crates:

| Crate | Responsibility |
|---|---|
| `repotracer-repo-tools` | Local repository tools and syntax lookup used by RepoTracer-managed backends |
| `repotracer-model` | OpenAI-compatible model client and deterministic mock backend |
| `repotracer-core` | Investigation loop, prompts, configuration, result handling |
| `repotracer-mcp` | MCP stdio server and `repo_scout` schema |
| `repotracer` | CLI, setup, settings, doctor, scout, and server commands |
| `repotracer-bench` | Paired benchmark manifests and runners |

The native Claude Code / Codex path can use capabilities from those CLIs that are broader than the local repository-tool crate.

## Sessions and concurrency

RepoTracer can retain native provider processes so related work does not pay startup cost every time. Conversation continuation and process reuse are separate ideas: a warm process can host more than one investigation, and an investigation can be restarted if reuse is no longer possible.

Independent MCP requests can run concurrently. Calls that continue the same investigation are ordered so two follow-ups do not race the same conversation state.

`session.idle_secs` controls how long an inactive retained process can stay warm. It is not a total investigation timeout.

`model.timeout_ms` controls native stream inactivity when configured.

`explorer.timeout_seconds` applies only to the generic OpenAI-compatible investigation loop.

## Syntax lookup

`repotracer symbols` provides local Tree-sitter-based syntax lookup for Rust, Python, JavaScript, TypeScript/TSX, and Go.

It can return definitions, references, or an outline. It is a lookup tool, not a resolved call graph or a complete semantic index, so text search and ordinary investigation are still necessary.

```bash
repotracer symbols "Config" --mode references
```

## Usage accounting

Complete-task comparisons count both sides of the assisted run:

```text
without RepoTracer = coding-agent usage
with RepoTracer    = coding-agent usage + investigator usage
```

Provider counters differ by backend. Missing usage stays unknown rather than being filled in with an estimate. See [Benchmarks](../BENCHMARKS.md) for the public results and [Measure the complete task](./benchmarks/why-token-counters-lie.md) for the reasoning behind this rule.

## Security model

Native investigators run in the user's existing Claude Code or Codex environment. RepoTracer is not a security sandbox around those CLIs.

OpenAI-compatible investigations use RepoTracer's own repository-tool loop and therefore have a different capability surface.

See [SECURITY.md](../SECURITY.md) for the threat model and operational guidance.
