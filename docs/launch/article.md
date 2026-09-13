# I stopped making the coding agent investigate the repo itself

A lot of coding-agent work happens before any code gets changed.

The agent searches for the entry point, opens candidates, traces callers, backtracks, runs a check, and only then starts implementing. All of that stays in the same conversation and uses the same limits the agent later needs for the actual change.

I wanted to separate those jobs.

So I built RepoTracer.

RepoTracer gives Claude Code or Codex one MCP tool: `repo_scout`. When a task needs repository investigation, a separate agent traces the code and reports back. The coding agent keeps the implementation.

```text
without RepoTracer

Claude / Codex
investigate → reason → implement → verify


with RepoTracer

RepoTracer
investigate
    ↓
Claude / Codex
reason → implement → verify
```

The investigator is more than a keyword search. It can follow behavior across files or related repositories, run checks or experiments, continue an earlier investigation, and say what it could not resolve.

That last part matters. If the investigator reaches the edge of what it can settle, the work is not thrown away. Claude Code or Codex gets what was found, what was checked, and what still needs attention.

## What happened in the benchmarks

We measure the complete task:

```text
without RepoTracer = coding-agent usage
with RepoTracer    = coding-agent usage + investigator usage
```

The current public paired runs produced:

| Task | Same limits | Time | Quality |
|---|---:|---:|---|
| Production bug fix | **2.68×** | **24.54% faster** | Bug fixed in both |
| SWE-bench Astropy 13453 | **2.00×** | **9.60% faster** | Passing fix in both |
| Multi-language release | **1.38×** | 16.83% slower | RepoTracer kept all existing tests passing; direct broke 4 |

A complete run that costs 62.68% less lets the same fixed limit cover 2.68× as many runs.

The part I did not expect was quality. On the public tasks where the two outputs differed in quality, RepoTracer produced the better result.

Every public run has raw artifacts and checksums in the repository.

## Why this can work

The coding model and the investigator do different jobs.

The investigator can spend its conversation understanding the repository. The coding agent receives the result without carrying every exploratory read and dead end in the conversation it later uses to implement the change.

That does not mean any small model will work. In a 72-run blind-graded reasoning study, the lowest investigator setting was faster and cheaper but produced six minor defects. Medium and high both scored 4.00/4.00 across their 24 evaluations, while high took much longer.

RepoTracer also keeps uncertainty visible. An unresolved investigation stays unresolved instead of being upgraded into a confident answer.

## Claude Code and Codex

RepoTracer supports both.

Native investigations run through the CLI you already use and reuse its login. They are not a RepoTracer-enforced read-only sandbox; the underlying native environment still matters.

You can also use an OpenAI-compatible endpoint, including a local one.

## Install

```bash
npx repotracer@latest setup
```

Repository: https://github.com/repotracer/repotracer

Benchmarks: https://github.com/repotracer/repotracer/blob/main/BENCHMARKS.md

Architecture: https://github.com/repotracer/repotracer/blob/main/docs/ARCHITECTURE.md

RepoTracer was inspired by Microsoft's FastContext work: https://arxiv.org/abs/2606.14066v3
