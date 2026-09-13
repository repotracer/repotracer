# Benchmarks

RepoTracer measures the whole coding task, not just the investigator.

```text
without RepoTracer = coding-agent usage
with RepoTracer    = coding-agent usage + investigator usage
```

A saving counts only when the complete RepoTracer run costs less **and** the task still passes the same quality check.

## Current public results

| Task | Direct | With RepoTracer | Same limits | Implementation time |
|---|---|---|---:|---:|
| Production bug fix | Bug fixed | Bug fixed | **2.68×** | **24.54% faster** |
| SWE-bench Astropy `13453` | Passing fix | Passing fix | **2.00×** | **9.60% faster** |
| Multi-language release | Broke 4 existing tests | Kept all existing tests passing | **1.38×** | 16.83% slower |

The largest reduction was **62.68% per complete run**. At that cost, the same fixed limit covers **2.68× as many runs**.

On the public tasks above, RepoTracer never produced the worse quality result. Two tasks reached the same result. On the multi-language release benchmark, the RepoTracer run kept every existing test passing while the direct run broke four.

## What is held constant

Paired runs keep the following fixed:

- user prompt
- coding-agent model and reasoning setting
- repository commit and environment
- task timeout
- task-specific quality gate

The assisted arm adds RepoTracer and its routing instructions. RepoTracer's investigator usage is included in the assisted total.

This matters because moving work to another model is not a saving by itself. If the coding agent compensates with more searches, retries, or reasoning later, that extra work still belongs in the total.

## What each run records

Depending on the backend and benchmark, the artifacts retain:

- coding-agent and investigator usage
- provider-reported or provider-equivalent cost fields when available
- wall-clock or implementation time
- task checks or blind quality scores
- control and assisted runs
- raw outputs
- checksums

Missing provider counters stay missing. They are not estimated into existence.

## Investigator reasoning study

The native-tool reasoning study ran 8 tasks × 3 repeats × 3 reasoning settings and blind-graded all 72 answers before revealing the arm.

| Scout setting | Blind quality | Median time | Equivalent usage cost | Result |
|---|---:|---:|---:|---|
| Luna low | 3.75/4; 6 minor defects | **34.59s** | **$0.042642** | Fastest, but missed some deeper cross-file details |
| Luna medium | **4.00/4; 24/24 perfect** | 58.15s | $0.064867 | Best tested balance |
| Luna high | **4.00/4; 24/24 perfect** | 138.65s | $0.128335 | Same quality as medium, with much more reasoning |

The point of this study is not that every small model is good enough. Low reasoning was cheaper and faster, but it also produced defects. When the investigator cannot settle a question, RepoTracer preserves the unresolved parts so the coding agent can continue from the work already done instead of treating an uncertain report as a complete answer.

Hard-task artifacts:

- [`summary.json`](./benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning/summary.json)
- [`protocol.json`](./benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning/protocol.json)

## Routing

The current routing suite contains **42 cases**, all classified correctly in the recorded run.

The router exists to keep RepoTracer out of tasks that do not need repository investigation. A localized edit can stay with Claude Code or Codex. A task that requires tracing behavior across files can call `repo_scout`.

Routing accuracy is tested separately from coding-task quality. A correct routing decision does not by itself prove a good implementation.

## Earlier routing studies

Earlier releases also kept their public results:

| Study | Scope | Quality | Cost saved | Same limits | Implementation time |
|---|---|---|---:|---:|---:|
| Repeated natural routing | 3 randomized pairs, 1 question | 6/6 checks in every arm | **39.20%** | **1.64×** | 31.21% slower |
| Immediate first-operation routing | 3 randomized pairs, 1 question | 6/6 checks in every arm | **28.63%** | **1.40×** | 6.65% slower |

These are historical results, not the headline numbers for the current release.

## Artifacts

Primary public artifacts include:

- [Google signup three-arm result](./benchmarks/results/runs/2026-08-24-google-signup-three-arm/result.json)
- [SWE-bench Astropy 13453](./benchmarks/results/2026-08-09-gpt-5.6-sol-swebench-astropy-13453.json)
- [Repeated natural routing](./benchmarks/results/2026-08-10-gpt-5.6-sol-luna-repeated-optimized.json)
- [Immediate routing](./benchmarks/results/2026-08-11-stage-a-immediate-routing.json)
- [Luna reasoning trials](./benchmarks/results/2026-08-11-reasoning-stage-b-local.json)
- [Extended timeout trials](./benchmarks/results/2026-08-11-extended-timeout-diagnostics.json)
- [Final native-tool reasoning study](./benchmarks/results/runs/2026-08-28-scout-reasoning/summary.json)
- [Derived benchmark tables](./benchmarks/results/synthesis.json)
- [Artifact index](./benchmarks/results/index.json)

See [why RepoTracer measures the complete task](./docs/benchmarks/why-token-counters-lie.md) and [`benchmarks/README.md`](./benchmarks/README.md) for the run layout.
