# Native Claude follow-up

The exact 132-word proposal received the highest blind quality grade on this
case and cost less than the current guidance. It took twice as long. The
parent-only baseline was the cheapest and fastest condition.

This is one additional task, with one solve per condition. It supports adding
evidence to the proposal, not a claim that shorter guidance generally improves
quality, cost or latency. It is separate from the earlier 76-word pilot.

## Results

The task was `django__django-13158` from SWE-bench Lite, selected before solving
with seed `20260913`. The bug concerns `QuerySet.none()` returning records after
combining queries. All three candidates passed the independently executed hidden
assertions. No solving session was restarted or given a whole-task deadline.

| Condition | Words | Complete cost, USD | Elapsed seconds | Blind grade | Scout calls |
| --- | ---: | ---: | ---: | ---: | ---: |
| Parent only | 0 | 0.5886515 | 123.775 | 9/10 | 0 |
| Current guidance | 349 | 1.8018530 | 302.909 | 9/10 | 1 |
| Proposed guidance | 132 | 0.7868645 | 607.024 | 10/10 | 0 |

Cost includes parent and scout, cache usage and native auxiliary model usage.
These are API-price equivalents reported by Claude Code, not subscription
invoices. Preparation, external acceptance checks, grading and diagnosis are
excluded from solving cost and time.

The proposal cost 56.3% less than the current guidance and took 2.004 times as
long. Both assisted conditions cost more and took longer than the baseline.
The one-point quality difference is the grader's judgment on these patches.
There is no estimate of variation between runs.

## Exact versions and conditions

The runner came from [PR #21](https://github.com/repotracer/repotracer/pull/21),
commit `d1527a327bc997d9d92fd83699e16f504789c287`, with local measurement adaptations
listed below. Both assisted conditions used the same RepoTracer 2.1.1 binary.
The current text at that commit has 349 words. It differs from the 333-word text
in the original pilot. The proposal was the exact 132-word body from PR #19 at
`90e11e4e0f8f3eb751a81266fe02e4fb0f1dbc1a`; its SHA-256 remains
`f3518ab90019f93878e69ee18058a319c8c286c068c59b3161444ce79efd4c89`.

The [summary data](native-claude.json) records all version and input hashes,
per-condition measurements, grades and limitations. Dataset revision was
`6ec7bb89b9342f664a54a6e0a6ea6501d3437cc2`; Django base was
`7af8f4127397279d19ef7c7899e93018274e2f9b`. This task had not informed the proposal.

Parents used native Claude Code 2.1.268 with observed model `claude-opus-5` and
native default effort. Scouts were configured as Opus Auto low/medium. The
current-guidance parent explicitly requested medium, and the scout used medium.
The proposed-guidance parent did not call the scout.

Conditions ran sequentially on the same macOS arm64 machine, using the same
source snapshot, task prompt, Python 3.9.6 dependencies and acceptance criteria.
The native runner shuffled their order. Caches were not flushed. User guidance,
hooks, auto-memory and unrelated MCPs were excluded consistently; ordinary
project tools remained available. The baseline had no RepoTracer tool.

A separate native `gpt-6-astra` session at high effort graded the patches and
test evidence, without condition labels, costs, elapsed task time or solving
conversations. It made no tool calls. The grades were fixed before diagnosis.

## What explains the differences

With the current guidance, the parent called the scout early and supplied a
relevant, detailed question. The scout reproduced the bug and found that a naive
fix would mutate shared source queries. It tested alternatives and recommended
propagating emptiness together with cloning in `Query.clone()`. The parent used
that recommendation. The evidence does not support an ignored handoff or a
malformed assignment as the explanation for this group's result.

The scout took 227.423 seconds and cost 1.216446 USD, or 67.5% of the group's
complete cost. Both parents that worked without a scout also found the shared
query problem. This task therefore shows no additional correctness benefit from
delegation. A narrower investigation might reduce duplicated work, but that is
an untested hypothesis; medium effort itself is not shown to be wrong.

The proposed-guidance parent kept cloning local to `set_empty()` and added tests
for nested unions and preservation of the original inputs. The blind grader
preferred that scope and coverage. The current and baseline patches instead
expanded cloning for combined queries generally. Their respective committed
tests covered three set operations and union only.

Most of the proposed condition's latency came from two full Django suite runs,
lasting 242.841 and 243.843 seconds. The first retained only the output tail, so
the parent ran the suite again to list failures. It then reproduced the same
five failures without its patch. Those failures remain in the evidence. Together,
the two full runs took 486.684 seconds, about 80% of the condition's elapsed time.

Keeping the first test output would remove the need to rerun solely to recover
failure names. Subtracting that time is not a measured alternative result. The
same applies to hypothetical savings from doing more work during a scout call.
The proposal's zero scout calls also cannot demonstrate better use of handoffs.

## Usage

The buckets below are disjoint. Auxiliary Haiku usage reported by the native
client is included as per-model aggregates. Its individual request count and
effort are unknown. Billable buckets and reported total costs reconcile.

| Condition | Role | Uncached input | Cache read | Cache write | Output |
| --- | --- | ---: | ---: | ---: | ---: |
| Parent only | Parent | 1260 | 498959 | 16838 | 6788 |
| Parent only | Scout | 0 | 0 | 0 | 0 |
| Current | Parent | 1246 | 407260 | 24349 | 5493 |
| Current | Scout | 1786 | 685464 | 46585 | 16250 |
| Proposed | Parent | 1270 | 702595 | 22655 | 8315 |
| Proposed | Scout | 0 | 0 | 0 | 0 |

Grading used 32253 uncached input tokens and 558 output tokens. Its dollar cost
is unavailable because no reported cost or matching rate card was present.
It is separate from solving costs.

## Measurement repairs and verification

The local reader joined final Claude streaming usage to the initial assistant
snapshots and retained native auxiliary model totals. It also recognized Django
`runtests.py` commands and corrected empty-string path redactions. Fifty local
runner unit tests passed. These adaptations do not change the solving binary and
are not included as product changes in this guidance PR.

A candidate test used the same method name as the hidden regression test, so
Python shadowed one method. Each candidate therefore received an additional
independent acceptance checkout with a unique name for the hidden method. Its
assertion AST was verified unchanged. All passed. Original acceptance evidence
was retained, and the final blind packet preserved its original label assignment
and review order. The pristine calibration had one expected new-behavior failure,
29 passes and two backend-capability skips.

Native Bash outputs without numeric exit codes remain marked `not_run` by the
runner, with their text visible to the grader. Here this means an uncertified
exit status, not that the command never executed. The independent acceptance
checks have explicit exit codes. Controller replacements preserved the active
solving processes. Two pre-model grader launch failures produced no grades or
model usage; their records were retained before the final native Astra launch.

Run the published-data checks from the repository root:

```sh
python3 docs/benchmarks/guidance-rewrite/verify_data.py
```

The public contribution contains this report and a sanitized summary, including
hashes of the private candidate patches and native traces. Raw runs, patches,
hidden test source, label keys, account configuration and local measurement
scripts remain outside tracked files, following the [benchmark guide](../../BENCHMARKING.md).
The summary supports arithmetic and version checks; it is not a complete replay
bundle. No new default or general performance claim follows from this one case.
