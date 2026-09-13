# Parent guidance pilot

The measured short guidance retained scout use in all six assisted trials and
matched or improved the number of fully passing patches in each parent harness.
The traces also show that calling the scout is only part of the problem: an
investigation can target the wrong subsystem, overlap with implementation, or
return advice that still needs a specific check.

This motivates a rewrite around assignment coverage, an investigation stopping
condition and use of the handoff. It does not establish that fewer words alone
improve outcomes, or that RepoTracer always beats direct investigation.

## What was compared

Three randomly selected DeepSWE tasks, three conditions, two parent harnesses:
18 scored trials, one observation per cell. Both assisted conditions used the
same RepoTracer v2.1.0 binary. Baseline had no RepoTracer tool or guidance.

| Guidance | Words | Status |
| --- | ---: | --- |
| [v2.1.0 long](guidance/long.md) | 333 | Measured |
| [Short candidate](guidance/short.md) | 76 | Measured |
| [Proposed routing text](../../../crates/cli/src/agents.rs) | 132 | Revised after trace review; not benchmarked |

Word counts use whitespace splitting, including any Markdown headings in the
frozen benchmark inputs. The proposed routing body has no heading.

## Results

A passing patch must pass every new-behavior test and every regression test.
Scores are for the patch submitted within the fixed trial budget. See
[methodology](methodology.md) for termination, isolation and usage accounting.

| Parent | Long | Short | No RepoTracer |
| --- | ---: | ---: | ---: |
| Codex, fully passing patches | 1/3 | 2/3 | 2/3 |
| Claude Code, fully passing patches | 1/3 | 1/3 | 0/3 |
| Codex, mean elapsed minutes | 15.02 | 10.44 | 9.29 |
| Codex, parent + scout input tokens | 16,422,232 | 4,982,016 | 3,485,011 |
| Codex, parent + scout output tokens | 116,812 | 62,472 | 44,660 |
| Assisted trials using the scout, both parents | 6/6 | 6/6 | n/a |

Token totals sum the three tasks. Inputs include cache reads. Different parent
and scout models have different billing, so these are not dollar estimates.
The long Codex total is dominated by one high-effort Textual investigation.
The short condition matched baseline's Codex patch score while taking longer
on average and consuming more combined tokens.

The partial scores matter too. `New` means fail-to-pass; `Reg` means pass-to-pass.

| Task | Parent | Long, New / Reg | Short, New / Reg | Baseline, New / Reg |
| --- | --- | --- | --- | --- |
| Bandit | Codex | 67/69, 281/282 | 67/69, 280/282 | 67/69, 280/282 |
| Bandit | Claude | 69/69, 281/282 | 67/69, 280/282 | 69/69, 281/282 |
| Prometheus | Codex | 17/17, 28/28 | 17/17, 28/28 | 17/17, 28/28 |
| Prometheus | Claude | 17/17, 28/28 | 17/17, 28/28 | 16/17, 28/28 |
| Textual | Codex | 19/20, 6/6 | 20/20, 6/6 | 20/20, 6/6 |
| Textual | Claude | 0/20, 6/6 | 0/20, 6/6 | 17/20, 6/6 |

## Why these instructions

The [trace analysis](trace-analysis.md) separates observations from hypotheses.
It supports the following design choices for the proposed revision:

| Observed behavior | Proposed instruction |
| --- | --- |
| Short guidance triggered every assisted trial; initial calls followed 0 to 2 setup operations. | Keep an explicit investigation trigger. No hook is justified by these cases. |
| A scout investigated canonical labels instead of PromQL sorting; another assignment omitted the requested example. | Include relevant requirements and the questions the answer must resolve. |
| A long investigation returned after the parent had already implemented the same area. | State when the questions are answered well enough to proceed; work on independent parts while waiting. |
| Claude's Textual parents continued exploring after detailed handoffs and submitted empty patches. | Continue implementation from the evidence, checking specific gaps or contradictions. |
| A Bandit review endorsed incorrect metric semantics; a Textual recommendation was reverted after snapshot comparison. | Ask for evidence and uncertainties, and retain targeted verification. |

The rewrite keeps repository targeting, conversation reuse, automatic effort and
tool-discovery/output fallback instructions. The MCP schema continues to document
the full interface. It changes the shared text installed for Codex and Claude
Code, with no hook or runtime enforcement.

The evidence supports trying this revision, not claiming it has already beaten
the measured candidate. A follow-up should run the exact proposed text, add a
small direct-lookup control, and check whether handoffs replace enough parent
exploration. These three tasks cannot establish statistical significance or
general routing accuracy.

## Audit the data

Run from the repository root:

```sh
python3 docs/benchmarks/guidance-rewrite/verify_data.py
```

[Results](results.json) include all 18 trial outcomes, termination states, usage
and submitted-patch hashes. [Manifest](manifest.json) pins tasks, images, models,
guidance, the binary and the scheduled trial order. `grades/` contains original
reward objects and test names/statuses; `scouts/` contains the recorded requests,
reports and usage with host metadata removed.

`patches/` contains the exact submitted diffs encoded as JSON strings, preserving
whitespace and SHA-256 through text-formatting tools. They are benchmark outputs,
not changes to RepoTracer. Extract a patch for inspection or regrading with:

```sh
python3 -c 'import json,sys; sys.stdout.buffer.write(json.load(open(sys.argv[1]))["patch"].encode())' \
  docs/benchmarks/guidance-rewrite/patches/t15.json > /tmp/t15.patch
```

Upstream source licenses and notices are retained in `licenses/`. Generated
patches change the respective pinned upstream projects. Full native sessions,
host configuration and held-out test source are not part of this publication.
The public bundle supports checking recorded scores, reports and patches; it is
not a turnkey reproduction of the original local subscription harness.
