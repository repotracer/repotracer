# Scout tool decision 001: test Colby CodeGraph first

- Date: 2026-08-27
- Status: historical diagnostic proposal; its fixed matrix and existing-index-only rule are superseded by the adaptive tool phase in `../PLAN.md`
- Scope: Scout repository retrieval; no production adoption decision
- Priority: quality, then complete provider cost, then end-to-end time
- Supersedes: the three-candidate ordering in `2026-08-27-scout-search-tool-decision.html`

## Decision

Test [`colbymchenry/codegraph`](https://github.com/colbymchenry/codegraph) before any other new Scout retrieval tool. Add one bounded `codegraph_explore` affordance to the candidate arm and nothing else. The control keeps Scout's current `Read`, `Glob`, and `Grep` tools.

This is the first experiment, not a recommendation to ship CodeGraph.

## Why this goes first

1. **It tests the largest missing capability.** Scout can already search text and read files. CodeGraph adds cross-file symbols, calls, imports, and current line-numbered source in one natural-language query.
2. **Its default interface is unusually close to Scout's boundary.** `codegraph explore <query>` is one composite read-only operation. We do not need to expose its lower-level commands.
3. **It can obey the no-surprise-index rule.** The tool appears only when a compatible `.codegraph/` index already exists. RepoTracer never creates or updates an index.
4. **The result is informative even if it loses.** CodeGraph publishes both throughput gains and a serious counter-result: in its own seven-repository, three-turn Sonnet campaign, retrieval output left 82% more context resident on average than file access, while still reducing processed tokens, cost, time, and tool calls. RepoTracer needs to measure both sides on Scout rather than inherit either claim.
5. **The experiment is cheap to implement and easy to remove.** The local workstation already has the CLI. No package, version, provider, or benchmark-task change is required to prove the mechanism.

Probe remains second. It is the best zero-index candidate, but it overlaps more with current `Grep` and asks a less decisive first question.

## Candidate arm contract

Expose exactly one additional tool:

```text
codegraph_explore(query: string) -> current line-numbered source plus relevant call paths
```

Rules:

- Require an existing compatible `.codegraph/` index.
- Never run `codegraph init`, `codegraph index`, a watcher, or any setup command.
- Keep `Read`, `Glob`, and `Grep` unchanged.
- Do not expose CodeGraph's search, overview, context, symbol, daemon, or maintenance commands.
- Do not add a prompt recipe or mandatory tool order. The tool description is the treatment.
- Bound subprocess time and output through RepoTracer's existing tool limits; record truncation as a trial event.
- Return a clear unavailable result if the index or executable is absent. Do not silently create state or route to a network service.
- Treat every returned path as untrusted until RepoTracer's existing root check and citation validation accept it.

## Diagnostic experiment

### Arms

- **Control:** current Scout with `Read`, `Glob`, and `Grep`.
- **Candidate:** byte-identical model request and task, plus `codegraph_explore`.

Keep the same repository snapshot, model, reasoning setting, task order, environment, and existing `.codegraph/` directory in both arms. The model must have no shell route to the CodeGraph executable; only the candidate adapter may invoke it. Record any direct CLI attempt or output as contamination.

### Workload

Run an eight-task paired diagnostic, three randomized repeats per arm: 48 trials.

- Four cross-file architecture or request-flow questions.
- Two change-impact or call-site questions with known required paths.
- Two negative controls dominated by local configuration, prose, or exact text where graph traversal should not help.

Use repositories and tasks already supported by the benchmark harness. Build or obtain indexes before timed trials and record their provenance. Index creation stays outside RepoTracer and outside steady-state trial time.

This diagnostic selects whether CodeGraph deserves a release-grade run. It cannot justify a release claim.

### Runtime regimes

Measure both:

1. **Cold attach:** first query against a ready index, including process or daemon startup.
2. **Warm steady state:** pre-warm the CodeGraph process before paired trials so retrieval quality is not confounded with attach latency.

Record index build time, disk size, peak memory, and incremental refresh time separately as adoption costs.

### Metrics

Use the existing priority order and accounting rules:

1. **Quality:** blind weighted task score, required-behavior failures, expected-path coverage, and citation validity.
2. **Complete subscription usage:** count the solver, children, and every Scout call from Codex subscription telemetry; never estimate provider price from tokens.
3. **End-to-end time:** complete trial wall time, plus cold and warm Scout retrieval time.
4. **Retrieval behavior:** Scout tool calls, failed calls, returned bytes, truncations, fallback reads, and whether negative-control tasks avoid the graph tool.
5. **Context pressure:** final Scout input tokens, retrieval tokens still resident at synthesis, and share of Scout's context. Do not substitute total processed tokens for occupancy.
6. **Adoption cost:** index build, disk, memory, refresh latency, unsupported-language rate, and stale-index failures.

### Gates

The diagnostic advances CodeGraph only if:

- neither arm introduces a new security, data-loss, citation, or required-behavior failure;
- candidate quality is no worse on the two negative controls;
- at least four of the six graph-suitable tasks improve or tie quality while reducing median baseline repository calls or complete cost;
- every comparison reports residual context occupancy, including regressions; and
- no trial is contaminated by an uncounted CLI or network path.

Production promotion still requires the existing release gate: at least 30 independent tasks, three repeats per arm, a 95% lower bound of paired quality delta at or above -5 points, positive 95% lower-bound cost savings with a 60% median target, and no more than +20% time regression at the 95% upper bound.

## Next candidates

If CodeGraph fails the diagnostic, test one candidate at a time in this order:

1. [`probelabs/probe`](https://github.com/probelabs/probe) — zero-index hybrid search with a small read-only surface.
2. [`ast-grep/ast-grep`](https://github.com/ast-grep/ast-grep) — only on syntax-shaped tasks; not a general Scout replacement.
3. [SCIP](https://github.com/sourcegraph/scip) or an existing language-server adapter — definition/reference precision when the project already has semantic state.
4. [`cocoindex-io/cocoindex-code`](https://github.com/cocoindex-io/cocoindex-code) — one semantic search affordance, with index and embedding costs measured separately.
5. [`yoanbernabeu/grepai`](https://github.com/yoanbernabeu/grepai) — semantic search plus call-graph retrieval, after its persistent-index and embedding boundary is accepted.

Do not bundle them. Each tool changes retrieval, prompt surface, latency, and failure modes; a bundle would hide which change caused the result.

## Evidence

- [CodeGraph README](https://github.com/colbymchenry/codegraph/blob/main/README.md): SQLite graph, source extraction, auto-sync, and composite `explore` interface.
- [CodeGraph residual context study](https://github.com/colbymchenry/codegraph/blob/main/docs/benchmarks/residual-context-occupancy.md): vendor-run three-turn campaign, contamination analysis, 82% higher retrieval residual, and simultaneous throughput savings.
- [CodeGraph feedback metrics](https://github.com/colbymchenry/codegraph/blob/main/docs/benchmarks/agent-eval-feedback-metrics.md): separates occupancy, sufficiency, and allocation efficiency; documents pre-warming and CLI contamination controls.
- [Probe README](https://github.com/probelabs/probe/blob/main/README.md): zero-index local search, AST parsing, and MCP/CLI surfaces. Performance claims remain vendor claims until reproduced.
- [ast-grep documentation](https://ast-grep.github.io/guide/introduction.html): structural AST search and rewrite rather than natural-language repository retrieval.
- [SCIP README](https://github.com/sourcegraph/scip/blob/main/README.md): definitions, references, implementations, and language-specific indexers.
- [Zoekt README](https://github.com/sourcegraph/zoekt/blob/main/README.md): trigram-indexed code search, symbol-aware ranking, and local or service deployment.
- [RTK README](https://github.com/rtk-ai/rtk/blob/master/README.md): command-output filtering. Useful inspiration, but outside Scout's built-in-tool path and not a retrieval-quality upgrade.

Facts above come from current repository source or documentation. Performance numbers are source-published claims or source-published self-benchmarks. RepoTracer fit and ordering are our inference and must be tested.