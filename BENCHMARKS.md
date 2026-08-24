# Benchmarks

RepoTracer only counts a cost reduction when the complete task is cheaper after adding Luna's usage. Quality is graded separately and can block the result.

## Current evidence

| Study | Scope | Quality | Complete cost | Wall time | Decision |
|---|---|---:|---:|---:|---|
| Immediate first-operation routing | Three randomized pairs of one read-only cross-file question | 6/6 checks in every arm | **−28.63% median** | +6.65% median | Retained, one question only |
| Repeated natural routing | Three randomized pairs of the same question | 6/6 checks in every arm | **−39.20% median** | +31.21% median | Retained, one question only |
| SWE-bench Astropy 13453 | One coding task | Exact regression passed in both arms | **−50.12%** | −9.60% | Promising diagnostic |
| Google signup | One real implementation task | RepoTracer 78.75, direct 83.125 | **−62.68%** | −24.54% | Quality regression, no win declared |

The repeated routing results show that shifting repository search from Sol to Luna can lower provider cost without losing the checks used by that task. They do not establish an average across repositories.

The Google task found a concrete failure mode. RepoTracer localized the server and both UI entry points, but the parent agent tested a duplicated Better Auth fixture instead of the production module. A separate pilot on the same task also missed more requirements than the direct arm. This is repeat evidence for one task, not proof of a product-wide quality loss.

## What the numbers support

- On the current repeated routing benchmark, median complete provider cost fell 28.63% and all six checks passed in every arm.
- A fixed provider budget would cover about 40% more equivalent runs of that measured task.
- The single SWE-bench run cost 50.12% less and passed the exact regression in both arms.
- RepoTracer can still reduce quality. The Google implementation task was cheaper but scored 4.375 points below the direct arm.

## What the numbers do not support

- A universal cost or token reduction
- An average across independent repositories
- A claim that every Codex subscription limit lasts longer by the same amount
- The same quality as direct Codex across tasks
- A claim that total model tokens always fall; work can move from Sol to cheaper Luna while total tokens rise

## Luna reasoning

Medium remains the production default.

| Scout setting | Production-bound result | Extended result |
|---|---:|---:|
| Luna low | 0/3 inside 120 seconds | 6/6, 174.99s scout time |
| Luna medium | **3/3 inside 120 seconds** | **6/6**, 95.28s scout time |
| Luna high | 0/3 inside 120 seconds | 5/6, 101.85s scout time |

Higher reasoning has not improved the measured result. It missed the production timeout in the bounded trial and lost one evidence check in the extended trial.

## Method

Paired runs hold the solver model, reasoning level, prompt, repository commit, environment, and timeout constant. The assisted arm differs only by installing RepoTracer's MCP server and routing skill.

Every retained study records:

1. The unmodified user prompt
2. Main and scout requests, tokens, and provider cost
3. Wall time
4. Task checks or blind quality scores
5. Failed and rejected arms
6. Raw artifacts and checksums

The launch gate remains 30 independent tasks with three randomized repeats per arm, blind quality grading, and complete main-plus-scout accounting. That multi-task run has not happened, so the current release is a beta.

## Primary artifacts

- [Immediate routing](./benchmarks/results/2026-08-11-stage-a-immediate-routing.json)
- [Repeated natural routing](./benchmarks/results/2026-08-10-gpt-5.6-sol-luna-repeated-optimized.json)
- [SWE-bench Astropy 13453](./benchmarks/results/2026-08-09-gpt-5.6-sol-swebench-astropy-13453.json)
- [Google signup three-arm result](./benchmarks/results/runs/2026-08-24-google-signup-three-arm/result.json)
- [Luna reasoning trials](./benchmarks/results/2026-08-11-reasoning-stage-b-local.json)
- [Extended timeout trials](./benchmarks/results/2026-08-11-extended-timeout-diagnostics.json)
- [Derived benchmark tables](./benchmarks/results/synthesis.json)
- [Artifact index](./benchmarks/results/index.json)

See [the measurement note](./docs/benchmarks/why-token-counters-lie.md) and [`benchmarks/README.md`](./benchmarks/README.md) for the run layout.
