# What the investigations did well and poorly

The request/report records below are original tool-facing content with host
metadata removed. The interpretations were written after grading. A score gap
alone does not establish why an arm performed differently.

## Useful delegation, with a measurable cost

In Codex Prometheus [t08](scouts/t08.json), the short-guidance assignment covered
typed domains and ordering requirements. The scout located the shared comparison
seam, recommended exact arithmetic and warned about fixed-width duration
parsing. The parent used those recommendations and passed all 45 cases, as did
baseline t07. Parent input fell from 692,158 to 578,026 tokens, but scout input
brought the assisted total to 1,382,985. Elapsed time rose from 413.65 to 496.30
seconds. Work was delegated successfully without a net token or time saving.

Codex Textual [t14](scouts/t14.json) covered widget behavior, superclass scroll
watchers, reflow and the example. The implementation used the report's advice
and passed all 26 cases, as did baseline t13. Its 19.81-second advantage over
baseline is too small and unreplicated to distinguish from ordinary variation.

## Wrong target, then a useful correction

Codex Prometheus [t09](scouts/t09.json) first investigated canonical
`model/labels` sorting instead of PromQL `sort_by_label`. The parent located the
actual functions and redirected the same conversation. The second report
identified the integration harness correctly, and the final patch passed.

This is a concrete scoping detour. The short trial found the correct subsystem
from similarly broad wording, so the evidence does not prove that long guidance
caused it. The long parent also ran substantially more verification, including
over 107,000 fuzz comparisons. That work prevents attributing its entire timing
gap to the two scout calls.

## Early invocation, late handoff

Codex Textual [t15](scouts/t15.json) requested high effort. Its investigation took
599.20 seconds and 85 tools, versus t14's 99.26 seconds and 26 tools at medium.
The [selected parent events](parent-events.json) record the scout starting at
event 7, the parent reporting implementation in place at event 34, and the scout
returning at event 67. The report itself reviews `LogScrollView` in the live
worktree. The initial investigation had overlapped with implementation.

The parent used a useful pruning finding. It also acted on a recommendation to
pad ordinary entries, then reverted that change after snapshot comparisons
showed a regression. The handoff was used, with mixed consequences.

The final patch failed the example's usable-layout check at 100 by 28 cells.
Its own example test used 110 by 40. The scout assignment omitted the example,
while t14's included it. This is a coverage gap, not proof that mentioning the
example would have prevented the defect.

## Correct advice can still become an incorrect patch

Bandit [t02](scouts/t02.json) called early with a detailed assignment. The scout
explained the distinction between blanket and selective suppression counters.
The patch nevertheless normalized specific selectors covering all enabled tests
into blanket suppression. It failed the same four held-out cases as baseline.
That supports an implementation semantics error, not a missing trigger or a
wholly ignored handoff.

Long-guidance [t01](scouts/t01.json) made a focused second review request about
those metrics. The scout explicitly endorsed that normalization; the submitted
patch still failed the three metric cases. More review supplied incorrect
reassurance. Neither report length nor another call guarantees correctness.

Claude Bandit short t04 had the same four failures, while long t05 and baseline
t06 had only the statement-range failure. Both Claude scouts described the
existing counter distinction correctly. The traces do not isolate why the
parents implemented it differently.

## A returned report is not the end of parent exploration

Claude Textual [t16](scouts/t16.json) and [t17](scouts/t17.json) received detailed
maps after 510.79 and 398.27 seconds. The parents continued exploring covered
areas. Their last recorded actions were a broad baseline suite and further
reactive/scroll-internal inspection, respectively. Both submitted empty patches;
baseline t18 submitted a partial patch passing 17 of 20 new cases.

Some reads may have been necessary to edit safely. The observed result is that
the handoff did not replace enough investigation to produce an implementation
within the budget. This supports testing a clearer condition for moving from
investigation to implementation, rather than adding an invocation hook.

Claude Prometheus provides a counterpoint: both assisted patches passed every
case; baseline missed compound-duration ordering. The scouts found the right
subsystem and duration references, but neither explicitly identified the
`1h30m` versus `90m` case. Architectural help is a plausible contributor, not a
demonstrated cause of the score difference.
