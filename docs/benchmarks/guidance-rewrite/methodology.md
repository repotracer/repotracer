# Methodology

The pilot ran on 2026-09-13. Selection used
`random.Random(20260913).sample(sorted(task_paths), 3)` over 113 task directories
from [DeepSWE](https://github.com/datacurve-ai/deep-swe/tree/0b9fabbb63b9104d678fe965e1632f2dd9eaa2ea).
The three selected tasks were Bandit structured nosec directives, Prometheus
typed label sorting and Textual RichLog follow state. The manifest records their
base commits, prompt hashes, image digests and scheduled conditions.

Each task received long, short and baseline trials in both parent harnesses,
with one repeat. Guidance was frozen before scoring. The proposed 132-word
revision was written after reviewing the results and is not a measured arm of
this pilot. A later [native Claude follow-up](native-claude.md) tests that exact
revision under different conditions; the two studies are not pooled.

## Models and isolation

Codex used gpt-6-astra/high, CLI 0.154.0. Claude Code used opus[1m]/xhigh,
CLI 2.1.268, reported as claude-opus-5[1m]. Codex scouts used native
gpt-5.6-luna; Claude scouts used native opus. Both profiles used medium with
adaptive reasoning enabled. Parent-requested effort overrides were allowed:
the long Codex Textual trial requested high, the other Codex investigations used
medium, and all Claude investigations reported high.

Both assisted arms used the same installed RepoTracer v2.1.0 binary. Baseline
had no RepoTracer guidance or tool. Parents and scouts excluded global guidance,
skills, hooks and unrelated MCPs. Fresh checkouts exposed the pinned base and
local trial work, without later repository history. Solvers ran natively on
macOS; graders ran in pinned Linux containers. Up to six trials ran concurrently.
This was workspace isolation, not a hardened adversarial boundary.

## Budget and scoring

Each parent received 1,200 seconds. The submitted patch was captured at normal
exit or budget expiry, then graded without subsequent diagnostic edits.
All nine Codex parents exited normally. The nine Claude parent sessions reached
the configured budget, so their rows measure patch quality at that budget rather
than completed solves. The small elapsed overrun records process termination.
Claude parent token totals are unavailable because no final usage event was
emitted; missing totals remain null, not zero.

Reward is one only if every fail-to-pass and pass-to-pass case succeeds. The
partial table retains differences concealed by binary reward, including empty
Claude Textual assisted patches versus the baseline's partial implementation.
Original reward objects and per-test statuses are published for every trial.

Parent/scout input totals include cached input; reasoning output is already part
of output totals and is not added again. Scout latency can overlap parent work,
so it must not be added to parent elapsed time. Recorded operation counts count
command/MCP start events, not every nested native operation. These metrics do
not establish monetary savings.

## Setup checks and exclusions

Three initial Codex attempts were uniformly aborted because the harness sandbox
blocked local Git writes. They are unscored setup attempts, followed by fresh
trials with local Git writes available and the same guidance. Codex and Claude
tool-and-commit smoke checks passed before scoring. Python environments were
seeded and task dependencies installed consistently across conditions.

Pristine verifier calibration passed all regression cases and none of the new
cases for each task. Verifier housekeeping was adapted to preserve files with
`git show` and backups, and move Bandit's preexisting `.stestr` directory.
Test definitions and scoring criteria were unchanged. The manifest retains the
original runner hashes as provenance; the machine-specific harness is not
included here.

## Trace review and uncertainty

The review rubric was fixed before scored trials: routing/timing, assignment
coverage, scout correctness, use of evidence, implementation and verification.
An AI reviewer inspected recorded actions, requests, reports, submitted code
and held-out results. The review was not blinded or independent of the prompt
author. Reports are evidence of what the scout said, not proof that it was right.

Only explicit parent actions and tool outputs inform the published qualitative
examples. The public export removes session metadata, local paths and native
reasoning. Scout requests replace checkout paths and conversation identifiers
with placeholders; their substantive query/report wording is preserved.
Grade exports keep names/statuses and omit traceback source. Patch contents are
byte-preserved, with upstream licenses retained.

Three tasks and one run per condition cannot isolate causality, estimate variance
or establish significance. Model effort, cache behavior, concurrent load,
platform differences and different parent verification choices affect timing.
There is no negative control for unnecessary scout use on a trivial lookup.
The study supports specific next hypotheses, not a general shorter-is-better
claim.

The prompt structure also follows the emphasis on precise triggers and progressive
disclosure in [OpenAI's prompting article](https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra).
That is a design reference; the pilot results above are the empirical evidence.
