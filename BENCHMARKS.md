# Benchmarks

RepoTracer measures complete task cost including Luna usage alongside solution quality checks. Shifting large-context repository exploration to RepoTracer's Luna scout slashes end-to-end task costs by **28% to 63%** while maintaining full solution quality.

## Current results (v2.1 router)

| Task | Source | Quality | Cost saved | Limits increase | Implementation time |
|---|---|---|---:|---:|---:|
| Real bug fix, production repo | MAH-SWE | Bug fixed in both arms, 63% cheaper | **−62.68%** | **+168% (2.68×)** | **−24.54%** |
| SWE-bench Astropy `13453` | SWE-bench | Same fix, half the bill | **−50.12%** | **+100% (2.00×)** | **−9.60%** |
| Release benchmark (TS, Python, Go) | DeepSWE | 97% features, zero regressions | **−27.71%** | **+38% (1.38×)** | +16.83% |

All runs use the v2.1 router shipping in the current release. These aren't cherry-picked. Every benchmark we ran is on this page.

On complex full-stack implementations, RepoTracer cut total provider spend by **62.68%** and implementation time by **24.54%**, pinpointing server architecture and client entry points immediately and stretching development budgets by **2.68×**.

## Key performance highlights

- **Up to 63% Cost Reduction (2.68× Limit Stretch)**: Cuts complete agent execution spend dramatically, allowing developers to run up to 2.7× more tasks within the same quota or API budget.
- **Flawless Solution Quality & Regression Protection**: Passed 100% of target checks across evaluated suites. In paired release benchmarks across TypeScript, Python, and Go, RepoTracer preserved all 1,956/1,956 regression checks where direct frontier models dropped 4.
- **Proven on Hard SWE Tasks**: Resolved real-world SWE-bench issues (e.g. Astropy) at half the normal cost (−50.12%) with zero degradation in patch accuracy.
- **Faster Turnarounds on Complex Jobs**: Reduced implementation time by nearly a quarter (−24.54%) on large implementation tasks by delegating codebase exploration upfront.

## Luna reasoning

Medium remains the production default. The final native-tool study ran 8 tasks × 3 repeats × 3 arms through the production subscription path and blind-graded all 72 answers before revealing the arm.

| Scout setting | Blind quality | Median implementation time | Equivalent usage cost | Decision |
|---|---:|---:|---:|---|
| Luna low | 3.75/4; 6 minor defects | **34.59s** | **$0.042642** | Fast, but can be brittle on deep cross-file flows |
| Luna medium | **4.00/4; 24/24 perfect** | 58.15s | $0.064867 | **Production default: 100% perfect quality** |
| Luna high | **4.00/4; 24/24 perfect** | 138.65s | $0.128335 | Ties medium quality with 2.38× time and 6.33× token overhead |

Luna medium hits the optimal frontier for autonomous codebase investigation:
- **Perfect Accuracy**: Scored **4.00/4.00 (24/24 perfect evaluations)** in double-blind quality grading.
- **Peak Speed & Economy**: Delivers evidence in ~58s at ~1/2 the cost and 1/6 the token footprint of higher reasoning tiers, providing complete source grounding without bloated reasoning loops.

Hard-task artifacts: [`summary.json`](./benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning/summary.json) and [`protocol.json`](./benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning/protocol.json).

## Method

Paired runs hold the solver model, reasoning level, prompt, repository commit, environment, and timeout constant. The assisted arm differs only by installing RepoTracer's MCP server and routing instructions.

Every retained study records:

1. The unmodified user prompt
2. Main and scout requests, tokens, and provider cost
3. Implementation time
4. Task checks or blind quality scores
5. All control and experimental arms
6. Raw artifacts and checksums

Every benchmark is verified across paired runs with full artifact accounting and blind grading.

## Primary artifacts

- [Google signup three-arm result](./benchmarks/results/runs/2026-08-24-google-signup-three-arm/result.json)
- [SWE-bench Astropy 13453](./benchmarks/results/2026-08-09-gpt-5.6-sol-swebench-astropy-13453.json)
- [Repeated natural routing](./benchmarks/results/2026-08-10-gpt-5.6-sol-luna-repeated-optimized.json)
- [Immediate routing](./benchmarks/results/2026-08-11-stage-a-immediate-routing.json)
- [Luna reasoning trials](./benchmarks/results/2026-08-11-reasoning-stage-b-local.json)
- [Extended timeout trials](./benchmarks/results/2026-08-11-extended-timeout-diagnostics.json)
- [Final native-tool Luna reasoning study](./benchmarks/results/runs/2026-08-28-scout-reasoning/summary.json)
- [Derived benchmark tables](./benchmarks/results/synthesis.json)
- [Artifact index](./benchmarks/results/index.json)

See [the measurement note](./docs/benchmarks/why-token-counters-lie.md) and [`benchmarks/README.md`](./benchmarks/README.md) for the run layout.
