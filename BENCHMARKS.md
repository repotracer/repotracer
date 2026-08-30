# Benchmarks

RepoTracer measures complete task cost including Luna usage alongside solution quality checks.

## Current evidence

| Study | Scope | Quality | Complete cost | Wall time | Decision |
|---|---|---:|---:|---:|---|
| Immediate first-operation routing | Three randomized pairs of one read-only cross-file question | 6/6 checks in every arm | **−28.63% median** | +6.65% median | Retained, one question only |
| Repeated natural routing | Three randomized pairs of the same question | 6/6 checks in every arm | **−39.20% median** | +31.21% median | Retained, one question only |
| SWE-bench Astropy 13453 | One coding task | Exact regression passed in both arms | **−50.12%** | −9.60% | Promising diagnostic |
| Google signup | One real implementation task | RepoTracer 78.75, direct 83.125 | **−62.68%** | −24.54% | Substantial cost drop, minor score delta (−4.38 pts) |
| 2026-08-30 paired release check | Three DeepSWE tasks: TypeScript, Python, Go | 2/3 exact in both; 147/151 feature checks in both; RepoTracer 1956/1956 regression checks vs 1952/1956 | **−27.71% total** | +16.83% total | Retained; quality held, no latency claim |

The repeated routing results show that shifting repository search from Sol to Luna can lower provider cost without losing the checks used by that task. They do not establish an average across repositories.

The Google task showed significant cost savings (−62.68%) alongside a slight quality delta (78.75 vs 83.125). RepoTracer localized the server and both UI entry points, but the parent agent tested a duplicated Better Auth fixture instead of the production module.

## What the numbers support

- On the current repeated routing benchmark, median complete provider cost fell 28.63% and all six checks passed in every arm.
- A fixed provider budget would cover about 40% more equivalent runs of that measured task.
- The single SWE-bench run cost 50.12% less and passed the exact regression in both arms.
- The Google implementation task reduced cost by 62.68% with a slight score difference (78.75 vs 83.125).
- The final three-task paired check reduced complete cost by 27.71%, matched baseline feature quality, and preserved four regression checks baseline lost.

## What the numbers do not support

- A latency improvement: the final paired check was 16.83% slower in aggregate and won only one of three tasks.
- A repository-wide effect size from the three-task final check.
- A universal cost or token reduction
- An average across independent repositories
- A claim that every Codex subscription limit lasts longer by the same amount
- Uniform output quality across all task types
- A claim that total model tokens always fall; work can move from Sol to cheaper Luna while total tokens rise

## Luna reasoning

Medium remains the production default. The final native-tool study ran 8 tasks × 3 repeats × 3 arms through the production subscription path and blind-graded all 72 answers before revealing the arm.

| Scout setting | Blind quality | Median wall time | Equivalent usage cost | Decision |
|---|---:|---:|---:|---|
| Luna low | 3.75/4; 6 minor defects | **34.59s** | **$0.042642** | Too brittle on cross-file flows |
| Luna medium | **4.00/4; 24/24 perfect** | 58.15s | $0.064867 | **Retain** |
| Luna high | **4.00/4; 24/24 perfect** | 138.65s | $0.128335 | No quality gain |

Medium beat low on six paired blind grades and never lost. High tied medium on all 24 grades while taking 2.38× the arm-level median wall time and 6.33× the reasoning-output tokens. Exact lookups worked at low, but a hybrid policy needs a separate held-out classifier study before production.

A preregistered complete-task follow-up then ran two held-out DeepSWE repositories, three repeats per arm. Every Sol run called Scout once. Medium passed 3/6 official verifiers; high passed 4/6. The difference was one recursive-delegation repeat. On the harder Boa task both arms passed 1/3, with opposite repeat winners. High therefore missed the gate of rescuing at least two of three repeats on one task.

Across all six Scout calls per arm, Luna high used 251 requests and cost $0.7328; medium used 79 requests and cost $0.1865. High's median wall time was slower on both tasks: 1,370.86s vs 1,239.61s on Boa and 935.87s vs 657.22s on delegation. Blind review found generally strong Scout evidence and attributed the failed patches to parent execution. Medium remains the default; no adaptive-high rule was added.

Hard-task artifacts: [`summary.json`](./benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning/summary.json) and [`protocol.json`](./benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning/protocol.json).

## Method

Paired runs hold the solver model, reasoning level, prompt, repository commit, environment, and timeout constant. The assisted arm differs only by installing RepoTracer's MCP server and routing instructions.

Every retained study records:

1. The unmodified user prompt
2. Main and scout requests, tokens, and provider cost
3. Wall time
4. Task checks or blind quality scores
5. All control and experimental arms
6. Raw artifacts and checksums

The launch gate remains 30 independent tasks with three randomized repeats per arm, blind quality grading, and complete main-plus-scout accounting. That multi-task run has not happened, so the current release is a beta.

## Primary artifacts

- [Immediate routing](./benchmarks/results/2026-08-11-stage-a-immediate-routing.json)
- [Repeated natural routing](./benchmarks/results/2026-08-10-gpt-5.6-sol-luna-repeated-optimized.json)
- [SWE-bench Astropy 13453](./benchmarks/results/2026-08-09-gpt-5.6-sol-swebench-astropy-13453.json)
- [Google signup three-arm result](./benchmarks/results/runs/2026-08-24-google-signup-three-arm/result.json)
- [Luna reasoning trials](./benchmarks/results/2026-08-11-reasoning-stage-b-local.json)
- [Extended timeout trials](./benchmarks/results/2026-08-11-extended-timeout-diagnostics.json)
- [Final native-tool Luna reasoning study](./benchmarks/results/runs/2026-08-28-scout-reasoning/summary.json)
- [Derived benchmark tables](./benchmarks/results/synthesis.json)
- [Artifact index](./benchmarks/results/index.json)

See [the measurement note](./docs/benchmarks/why-token-counters-lie.md) and [`benchmarks/README.md`](./benchmarks/README.md) for the run layout.
