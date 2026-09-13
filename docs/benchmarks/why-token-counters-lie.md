# Measure the complete task

A tool cannot prove savings by counting only the text it removed.

Coding agents react to tool output. A shorter search result can change what the agent does next: more reads, fewer reads, retries, cache hits, reasoning, or a different patch. The only useful comparison is the whole task.

## The accounting rule

Run the same task both ways:

```text
direct   = coding-agent usage

assisted = coding-agent usage
         + investigator usage
```

A cost result counts only if both runs still pass the same quality gate.

This prevents a common failure mode: making one local step cheaper while pushing more work downstream.

## What RepoTracer records

Depending on the backend and benchmark, a retained run records:

- coding-agent and investigator requests
- input, cache, output, and reasoning usage when reported
- provider-reported or provider-equivalent cost fields when available
- implementation or wall-clock time
- task checks or blind quality scores
- control and assisted runs
- raw artifacts and checksums

Missing usage remains unknown. RepoTracer does not fill gaps with invented precision.

## Why local counters can mislead

JetBrains published a paired RTK evaluation where a local counter reported **96.2 million saved tokens**, while complete task cost at low reasoning effort **increased by 7.6%**.

The filter did reduce its own output. The coding agent simply spent more elsewhere.

Source: [JetBrains RTK paired evaluation](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/)

That is why RepoTracer's headline numbers come from complete-task comparisons rather than the amount of text one tool claims to have removed.

## Quality comes first

A cheaper failed run is not a saving.

Every benchmark needs a task-specific quality check: tests, verifiers, or blind grading. Only after both arms meet that bar does the cost comparison mean what users think it means.

See [BENCHMARKS.md](../../BENCHMARKS.md) for the current public results and raw artifact links.
