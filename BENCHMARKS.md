# Benchmarks

## Rule

**If the invoice didn't shrink, don't call it token savings.**



## What we can sell today (attributed)

Until repeated quality-passing Grephound runs land, public savings numbers remain **sourced**, not invented:

| Number | Meaning | Attribution |
|--------|---------|-------------|
| **−60.3%** main-agent tokens | Upper bound from delegated repo exploration | Microsoft FastContext ([arXiv:2606.14066](https://arxiv.org/abs/2606.14066), project README) |
| **+5.5** end-to-end score | Scout architecture can improve solve quality | FastContext SWE-bench-style evaluations |
| **+7.6%** task cost | Middleware “token killer” increased the bill | [JetBrains RTK trial](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/) · 80 clean pairs · low effort |
| **96.2M tokens “saved”** | rtk self-report while invoice rose | Same JetBrains writeup |

**Luna subscription pricing (2026-08-11):** $0.20/M uncached input, $0.02/M cached reads, and $1.20/M output including reasoning. Cache writes are not billed for these subscription-equivalent comparisons. Sol provider costs remain as recorded.

## Actual SWE-bench three-arm diagnostic — 2026-08-09

One real coding task: SWE-bench Verified `astropy__astropy-13453`. The same GPT-5.6 Sol solver received the same issue in every arm. The assisted arms exposed the preserved FastContext explorer backed by either GPT-5.6 Luna at low reasoning or local FastContext 4B Q4. Routing was natural; each arm ran in a fresh official SWE-bench container.

| Metric | Sol alone | Sol + Luna-low | Sol + local FastContext |
|--------|----------:|---------------:|------------------------:|
| Task result | **pass** | **pass** | **pass after scout fallback** |
| Main-agent input tokens | 823,105 | 324,636 (**−60.56%**) | 451,781 (**−45.11%**) |
| Main-agent cached input | 785,514 | 308,810 | 436,372 |
| Main-agent output tokens | 8,061 | 5,736 | 6,157 |
| Scout tokens | 0 | 64,408 | unknown local |
| All-model tokens | 831,166 | **394,780 (−52.50%)** | unknown |
| Total provider API cost | $0.869422 | **$0.433640 (−50.12%)** | $0.499090 (−42.60%) |
| Main / scout model turns | 29 / 0 | 25 / 5 | 30 / unknown |
| Main shell / scout repo-tool calls | 42 / 0 | 31 / 14 | 46 / failed |
| Wall time | 426.52s | **385.57s (−9.60%)** | 408.01s (−4.34%) |
| Exact gold regression | 1 passed | 1 passed | 1 passed |
| Full HTML test file | 10 passed, 16 skipped | 10 passed, 16 skipped | 10 passed, 16 skipped |

**Result:** Luna-low returned focused, validated evidence and its solver submitted the exact two-line gold fix. Its complete arm used 52.50% fewer model tokens and cost 50.12% less in provider charges on this run. Local FastContext did not return usable evidence: citation formatting failed once, then its retry emitted an unresolved literal tool request. Sol fell back and still passed; the local arm's lower bill cannot be attributed to scouting. Local compute and electricity are not priced.

This is one stochastic instance, not a savings headline. Arms ran sequentially, so the latency delta is descriptive only. It tests the preserved FastContext Python explorer scaffold, not Grephound's Rust runtime.

Artifact: [`benchmarks/results/2026-08-09-gpt-5.6-sol-swebench-astropy-13453.json`](./benchmarks/results/2026-08-09-gpt-5.6-sol-swebench-astropy-13453.json)

## Natural low-reasoning paired diagnostic — 2026-08-09

Same GPT-5.6 Luna **low-reasoning** agent and prompt. Grephound's routing skill and `repo_scout` tool are present only in arm B; Luna decides whether to use them.

| Metric | Normal | Grephound natural | Delta |
|--------|-------:|-------------------:|------:|
| Main-model tokens | 497,924 | 813,341 | **+63.35%** |
| FastContext scout tokens | 0 | unknown (2 timed-out calls) | unknown |
| Estimated main-model API cost | $0.043329 | $0.056883 | **+31.28%** |
| Wall time | 127.31s | 279.12s | **+119.25%** |
| Scout selected / useful | 0 / 0 | 2 / 0 | — |
| Quality-passing tasks | 2/3 | 2/3 | unchanged |
| Expected-path recall | 88.9% | 83.3% | −5.6 points |

Luna naturally selected Grephound for 2 of 3 tasks. Both FastContext calls timed out after 60 seconds and returned no citations, so scout tokens are unknown and the main model fell back to normal exploration. This run shows no savings.

Artifact: [`benchmarks/results/2026-08-09-gpt-5.6-luna-low-natural.json`](./benchmarks/results/2026-08-09-gpt-5.6-luna-low-natural.json)

## Forced medium-reasoning paired diagnostic — 2026-08-09

Same GPT-5.6 Luna agent and task, normal repository exploration versus a forced `repo_scout` call followed by fallback when needed:

| Metric | Normal | Grephound forced | Delta |
|--------|-------:|------------------:|------:|
| Frontier tokens | 1,013,514 | 937,254 | −7.52% |
| Estimated API cost | $0.064352 | $0.063002 | −2.10% |
| Wall time | 222.64s | 451.28s | **+102.70%** |
| Quality-passing tasks | 3/3 | 3/3 | unchanged |

This is **not a savings result**. All three MCP scout calls completed, but none returned validated citations, so the agent fell back to normal exploration. Gigo's scout timed out before reporting usage, making complete all-model token consumption unknown. The small frontier-cost reduction cannot be attributed to Grephound. One run per task is diagnostic only.

Artifact: [`benchmarks/results/2026-08-09-gpt-5.6-luna-forced.json`](./benchmarks/results/2026-08-09-gpt-5.6-luna-forced.json)

## Forced scout-report diagnostic — 2026-08-10

Four synthetic changes in this repository were each run once with GPT-5.6 Sol high: direct, with a pre-generated GPT-5.6 Luna-low scout report appended, and with a local FastContext 4B report appended.

| Arm | Named checks | Known all-model tokens | Provider cost | Cost delta | Wall time |
|-----|-------------:|-----------------------:|--------------:|-----------:|----------:|
| Sol alone | 22/22 | 3,147,770 | $4.400854 | — | 2,398s |
| Sol + Luna-low report | 22/22 | 4,323,769 | unavailable at current rates¹ | unavailable¹ | 2,341s |
| Sol + local 4B report | 22/22 | ≥2,925,770 | $4.795013 excluding local compute | +8.96% | 1,676s |
¹ This artifact did not preserve Luna cache-write tokens, so its historical bill cannot be repriced exactly.


This is a **forced report-injection diagnostic**, not a product or routing benchmark. Every assisted prompt already named relevant files, symbols, tests, or a precise change surface; the FastContext paper says to skip the helper when the issue already identifies the relevant file or symbol. The run also bypassed Grephound's runtime and appended each raw scout report directly to Sol.

The hard Luna-assisted pair explains the aggregate regression. Sol used 36 model turns versus 21, adding 825,600 cached-input tokens and 3,250 output tokens while uncached input fell by 3,104. Tool calls rose from 20 to 35: +2 read/search, +4 patch, +3 test, +6 test polls, −1 diff/status, and +1 failed commit attempt. Both runs first missed the same `Usage` re-export; the assisted run additionally retried a nonexistent package target and polled tests six times. The accurate 298-word scout report was redundant with an already-localized task. One stochastic pair cannot show that the report caused the worse implementation trajectory.

Artifact: [`benchmarks/results/2026-08-10-gpt-5.6-sol-scout-report-diagnostic.json`](./benchmarks/results/2026-08-10-gpt-5.6-sol-scout-report-diagnostic.json)

Source audit: [FastContext Appendix B](https://arxiv.org/html/2606.14066v3#A2) defines the intended routing and handoff; [Appendix A.4](https://arxiv.org/html/2606.14066v3#A1.SS4) caps parsed citations at 20 and parallel fan-out at six.

## Natural Luna-low routing diagnostic — 2026-08-10

The same read-only, cross-file setup question was run once with GPT-5.6 Sol high alone and once with Grephound available. The assisted agent naturally called `repo_scout` once; the scout used GPT-5.6 Luna low through the user's configured Codex CLI provider. Both answers passed the six requested behavior checks.

| Arm | Quality | Provider requests | All-model input / cache / output | Complete cost | Wall time |
|-----|--------:|------------------:|---------------------------------:|--------------:|----------:|
| Sol alone | 6/6 | 7 | 126,648 / 72,448 / 2,606 | $0.385404 | 116.7s |
| Sol + Luna-low scout | 6/6 | 12 | 205,854 / 113,664 / 3,064 | **$0.306827 (−20.39%)** | **108.1s (−7.36%)** |

The assisted total is $0.294207 for Sol plus $0.012620 for Luna. Sol input fell 42.45%, but all-model input rose 62.54%; this pair saved money by shifting exploration to the cheaper model, not by reducing aggregate tokens. One pair is diagnostic evidence, not a headline median.

Artifact: [`benchmarks/results/2026-08-10-gpt-5.6-sol-luna-natural.json`](./benchmarks/results/2026-08-10-gpt-5.6-sol-luna-natural.json)

## Savecostmax Luna routing diagnostic — 2026-08-10

The same setup question was rerun after isolating the Codex scout from inherited MCP/configuration, bounding the handoff to six citations and 8 KiB of evidence, tightening natural routing, and reducing the scout prompt. Arm order was assisted then direct; both ran read-only in fresh equivalent repository copies and passed the same six checks.

| Arm | Quality | Provider requests | All-model input / cache / output | Complete cost | Wall time |
|-----|--------:|------------------:|---------------------------------:|--------------:|----------:|
| Sol alone | 6/6 | 6 | 117,904 / 65,536 / 2,312 | $0.363968 | **63.9s** |
| Sol + Luna-low scout | 6/6 | 11 | 149,672 / 83,200 / 3,030 | **$0.242761 (−33.30%)** | 73.5s (+15.12%) |

Sol input fell 51.90%. The Luna scout used 92,959 input tokens, three repository tools, four model steps, and returned six validated citations. Complete all-model input still rose 26.94%; again, the cost win came from shifting exploration to Luna, not reducing aggregate tokens. The assisted arm's summed provider latency rose 14.75%.

Artifact: [`benchmarks/results/2026-08-10-gpt-5.6-sol-luna-savecostmax.json`](./benchmarks/results/2026-08-10-gpt-5.6-sol-luna-savecostmax.json)

## Repeated isolated Luna routing diagnostic — 2026-08-10

The setup question was repeated three times per arm after budgeting the scout for at most three repository tools, capping it at five citations, reducing evidence from 8 KiB to 6 KiB, and disabling unrelated Codex apps, browser, computer-use, image-generation, multi-agent, and plugin capabilities. Pair order alternated. Every answer passed the same six checks; every successful handoff stopped further main-agent repository exploration.

| Pair | Sol alone | Sol + Luna-low | Complete cost delta | Wall-time delta |
|------|----------:|---------------:|--------------------:|----------------:|
| 1 | $0.372301 | $0.226347 | **−39.20%** | +31.21% |
| 2 | $0.387586 | $0.265499 | **−31.50%** | +58.92% |
| 3 | $0.832314 | $0.304628 | **−63.40%** | **−53.25%** |
| **Paired median** | — | — | **−39.20%** | +31.21% |

Cost win rate was **3/3** with quality at **6/6 in every arm**. Median main-agent input fell 26.82%, while median complete all-model input rose 43.44%; the cost win still came from moving exploration to cheaper Luna. The corrected median scout cost is $0.009831; the scout used 77,310 input tokens, three repository tools, five validated citations, and a 9,084-character handoff.

All optimization runs remain in the artifact. Repricing changes the pre-optimization cohort to a −27.46% median paired cost delta and 3/3 cost wins. The interim bounded-handoff cohort had one `repo_scout` call correlate with three Luna conversations; that triggered strict capability isolation. Each of the three final calls correlated with exactly one Luna conversation.

This is repeated evidence on **one question**, not a general savings headline. Independent tasks are still required.

Artifact: [`benchmarks/results/2026-08-10-gpt-5.6-sol-luna-repeated-optimized.json`](./benchmarks/results/2026-08-10-gpt-5.6-sol-luna-repeated-optimized.json)

## Stage A immediate-routing diagnostic — 2026-08-11

**Retained for the Sol-token objective.** The same natural setup question was repeated three times per arm after combining cost-aware routing with immediate delegation. For eligible unknown-location exploration, the main agent had to call `repo_scout` as its first repository operation; prompts that already identified a narrow change surface still skipped it.

| Pair | Sol alone | Sol + Luna-low | Complete cost delta | Wall-time delta |
|------|----------:|---------------:|--------------------:|----------------:|
| 1 | $0.364022 | $0.238091 | **−34.59%** | **−7.84%** |
| 2 | $0.354570 | $0.280316 | **−20.94%** | +6.65% |
| 3 | $0.391796 | $0.279628 | **−28.63%** | +38.10% |
| **Paired median** | — | — | **−28.63%** | +6.65% |

Cost win rate remained **3/3**, and every answer passed **6/6** checks. `repo_scout` was the first repository operation in every assisted run: the main agent made zero repository calls before it and zero after the validated handoff. Median Sol input fell 50.19%; complete all-model input rose 21.97%. Corrected median scout cost was $0.008478; median handoff size was 7,758 characters.

Against the previous independent three-pair cohort, the median wall-time delta improved from +31.21% to +6.65%, and the Sol-input delta improved from −26.82% to −50.19%. The corrected paired cost delta moved from −39.20% to −28.63%, so this small stochastic cohort does **not** establish an incremental cost reduction from Stage A. It establishes the routing invariant while preserving a complete-cost win.

Artifact: [`benchmarks/results/2026-08-11-stage-a-immediate-routing.json`](./benchmarks/results/2026-08-11-stage-a-immediate-routing.json)


Decision: ship immediate routing because Sol-token reduction is now the primary optimization objective. It improved median Sol input, complete input, and wall time while retaining 3/3 quality and complete-cost wins. This three-pair, one-question cohort does not prove a lower absolute assisted cost than the previous stochastic cohort; the 34-task run must test that separately.

## Reasoning, Stage B, and local-model diagnostics — 2026-08-11

These were forced single-scout ablations against the restored pre-Stage-A routing policy. They isolate scout configuration; they are not natural-routing headlines.

The first pass used the 120-second production scout ceiling. That answered “does this configuration finish inside the current UX bound?” but not “can it finish?” An independent outer Codex MCP client also stopped tool calls at 300 seconds. Bills from failed runs are accounting, not savings.

| Original bounded condition | Quality passes | Median complete provider bill | Median wall time | Finding |
|----------------------------|---------------:|------------------------------:|-----------------:|---------|
| Luna low | 0/3 | $0.243810 | 140.64s | Missed the 120s production bound |
| **Luna medium** | **3/3** | **$0.252510** | **110.23s** | **Keep provisionally** |
| Luna high | 0/3 | $0.257510 | 144.07s | Missed the 120s production bound |
| Stage B: medium + adaptive two-operation budget | 0/3 | $0.186659 | 144.31s | Missed the 120s production bound |
| Local FastContext 4B Q4 | 1/3 usable handoffs | $0 provider bill | 162.59s | Completed but unreliable |

We reran with a 600-second scout ceiling and a 700-second MCP-client ceiling. All configurations completed, correcting the earlier capability inference:

| Extended condition | Quality | Complete provider bill | Wall time | Scout time |
|--------------------|--------:|-----------------------:|----------:|-----------:|
| Luna low | 6/6 | $0.208667 | 214.04s | 174.99s |
| Luna medium | 6/6 | $0.271555 | 135.85s | 95.28s |
| Luna high | 5/6 | $0.186764 | 144.66s | 101.85s |
| Stage B run 1 | 5/6 | $0.294075 | 111.69s | 67.25s |
| Stage B run 2 | 6/6 | $0.189637 | 118.23s | 65.90s |
| Stage B run 3 | 5/6 | $0.365622 | 241.20s | 198.88s |

Medium remains the provisional default because it now has four quality-passing observations across both protocols. Low has one pass and was slowest. High was cheaper in its single extended run but missed one direct-evidence check; one run does not overturn the repeated medium result.

Stage B remains rejected and reverted. Every extended scout still consumed the optional third repository operation, strict quality passed only 1/3, and its corrected $0.294075 median complete bill exceeded both the bounded medium median and the single extended medium control. The two-operation wording provided no observed tool-count reduction.

The local model executed Grephound's native `Read`, `Glob`, and `Grep` loop after temporary Ollama compatibility fixes. One run produced a complete evidence map, one returned valid citations with incorrect prose, and one returned invalid prefixed paths. A local-first fallback would therefore retain Luna's provider bill on at least two runs, add the local attempt's latency, and could not safely detect the semantically wrong valid-looking handoff. Local support remains disabled.

Artifacts: [`bounded reasoning, Stage B, and local`](./benchmarks/results/2026-08-11-reasoning-stage-b-local.json) · [`extended timeout correction`](./benchmarks/results/2026-08-11-extended-timeout-diagnostics.json)

Future diagnostics report two separate outcomes: completion inside the production UX bound and eventual completion under an extended experimental ceiling. Production remains configurable at 120 seconds; benchmark ceilings no longer determine capability claims.

**Rule for grephound headlines:** only medians from repeated paired complete-task runs, explorer included, quality counted.

We benchmark complete coding tasks:

| Metric | Why |
|--------|-----|
| Total provider cost | The bill |
| Main input / cache / output tokens | Frontier usage |
| Explorer cost / latency | Scout overhead (local or paid) |
| Turns | Control-loop waste |
| Wall-clock | UX |
| Task success | Quality gate |

We do **not** publish:

- “tokens avoided” by truncating tool output
- unpaired single runs as headlines
- quality-blind cost wins

## Methodology

See [docs/benchmarks/why-token-counters-lie.md](./docs/benchmarks/why-token-counters-lie.md).

Primary paired arms:

- **A — direct:** coding agent alone
- **B — natural:** same agent and prompt; `repo_scout` is available and the agent decides whether to call it

The solver receives only each task's natural `prompt`, verbatim and at the normal user-message position. It never receives expected paths, routing labels, file hints, `repo_scout` instructions, or a requirement to scout at a particular time. Arm B differs only because the installed Grephound skill and MCP tool are available.

Diagnostic arms stay separate:

- **Forced:** require `repo_scout` only on tasks preclassified as scout-eligible; estimates conditional scout efficacy, not routing value
- **Report injection:** append fixed evidence; isolates handoff quality from runtime and routing
- **Oracle evidence:** provide gold file/line evidence; estimates the maximum value of perfect localization

Hold constant: solver model, reasoning effort, system prompt, agent build, repository commit, task, environment, timeout, service tier, and cache policy. Prewarm outside measured conversation IDs. Randomize pair order and use fresh worktrees with equivalent build-cache state.

Timeouts answer two different questions. Run each experimental configuration with the production ceiling to classify UX viability, then with an extended ceiling to classify eventual capability. The outer agent/MCP client ceiling must exceed the scout-process ceiling. Record both limits and never label a production-timeout failure as model incapability.

Predeclare scout eligibility from the routing contract: cold-start exploration, broad cross-file localization, or a failed targeted search. Mark tasks that already name the relevant files, symbols, or precise change surface as expected skips. Report natural-arm call rate and economics separately for eligible/called, useful, and skipped tasks.

Launch gate: at least 30 independent tasks and at least three randomized repeats per arm, increased when preregistered power analysis requires it. Grade task success blind to arm. Report paired medians, spread, bootstrap confidence intervals, win rate, and complete main-plus-scout cost; never discard failed or expensive pairs after seeing results.

## Reproduce

```bash
cargo run -p grephound-bench -- --suite benchmarks
```

Run artifacts, including rejected candidates, live under `benchmarks/results/`. [`index.json`](./benchmarks/results/index.json) standardizes the inventory, pricing profile, decisions, headline gate, and SHA-256 digests; `SHA256SUMS` is the independent checksum ledger.

## Status

The harness scaffold ships in 0.1.0. The predeclared suite now contains **34 independent tasks**: 20 scout-eligible, 14 expected skips, and 91 expected-path assertions. This satisfies the task-count prerequisite only. No multi-task paired economics have been run, so headline Δ numbers remain blocked on three randomized quality-graded repeats per arm.

### Optimization groundwork — 2026-08-11

- **Native orientation map rejected and removed.** Nine randomized Luna pairs across three repository questions produced a +20.53% median paired subscription-equivalent cost delta. Cost won only 2/9 pairs. Median wall time was effectively flat (52.08s baseline, 52.49s map; paired median −3.93%). Blind median quality tied at 5/6; mean quality rose 4.89→5.33, but the map won only 4/9 quality pairs, tied 2, and lost 3. That is not robust enough to buy the cost regression. Full artifact: [`2026-08-11-native-map-ablation.json`](./benchmarks/results/2026-08-11-native-map-ablation.json).
- **Repository-tool output bounds retained.** Ten alternating-order pairs on deterministic worst-case fixtures reduced median serialized output from 3,991,018→32,073 bytes for Read (−99.20%), 70,226→32,074 for Grep (−54.33%), and 50,599→32,467 for Glob (−35.83%). Every capped result preserved exact prefix evidence and actionable continuation guidance; median tool latency was unchanged. This is an output-footprint result, not a provider-token claim. Full artifact: [`2026-08-11-tool-output-footprint-ablation.json`](./benchmarks/results/2026-08-11-tool-output-footprint-ablation.json).
- **Routing changes retained.** The predeclared evaluator now covers 34 tasks, 20 expected scout calls, 14 expected skips, and 91 path assertions. The existing three-pair natural-routing cohort remains 3/3 on quality and cost after Luna repricing, with a corrected −39.20% median paired complete-cost delta; it is still one question, not a launch headline. Summary: [`2026-08-11-routing-suite-summary.json`](./benchmarks/results/2026-08-11-routing-suite-summary.json).
- **MCP handoff rewrite rejected.** Source excerpts already appear once, in the legacy-compatible text content. Structured content contains locators and truncation metadata rather than a second source copy. Removing the text fallback would break clients using the advertised 2024-11-05 protocol.
- **External map dependencies rejected.** GitNexus is PolyForm Noncommercial; Aider's map requires its Python/tree-sitter stack; CodeGraph is Apache-2.0 but ships a much larger standalone engine and MCP surface. None earned an added dependency or install step.
