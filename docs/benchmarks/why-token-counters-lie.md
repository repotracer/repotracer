# Measure the complete task

A middleware tool cannot prove savings by counting only the text it removed. Coding agents react to tool output, so one compressed result can change later searches, cache reads, retries, and the final patch.

Measure both arms from the same user prompt:

```text
direct cost = main-agent requests
assisted cost = main-agent requests + scout requests
```

A cost result is valid only when both arms meet the same quality gate.

RepoTracer benchmarks record:

1. Main and scout input, cache, output, and reasoning tokens
2. Complete provider cost
3. Wall time
4. Task checks or blind quality scores
5. All control and experimental runs
6. Raw artifacts and checksums

This distinction matters. JetBrains measured an RTK configuration whose local counter reported 96.2 million saved tokens while complete task cost rose 7.6% at low reasoning effort. The filter reduced its own output, but the agent spent more elsewhere.

Source: [JetBrains RTK paired evaluation](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/)

RepoTracer reports paired medians across repeated runs and full-task benchmarks, ensuring reported gains reflect genuine invoice savings and verified task outcomes rather than localized token reductions that push costs downstream.
