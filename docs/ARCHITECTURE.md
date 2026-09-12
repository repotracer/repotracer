# Architecture

## Request path

```text
Codex Sol
  → MCP call: repo_scout(query, investigation)
    → RepoTracer MCP server
      → reusable isolated codex app-server process
        → fresh conversation, or bounded explicit follow-up
        → GPT-5.6 Luna, medium reasoning, fast service tier
          → read-only shell and local Symbols lookup
      ← structured scout result
    ← validated citations and source excerpts
  → use findings and excerpts; inspect any missing evidence
  → edit and verify
```

`repotracer setup` registers the stdio MCP server and writes managed instructions describing the scout. The parent chooses whether to delegate or investigate directly. A natural-language query is sufficient, even when the parent does not know the relevant files or search terms. Optional context, questions, intent, and paths can help the scout. They do not prescribe its search sequence.

## Components

| Crate | Responsibility |
|---|---|
| `repotracer-repo-tools` | Read, Glob, Grep, syntax index, root checks, and concurrent execution |
| `repotracer-model` | OpenAI-compatible model client and mock backend |
| `repotracer-core` | Scout loop, prompts, configuration, and citation parsing |
| `repotracer-mcp` | MCP stdio server and `repo_scout` schema |
| `repotracer` | CLI, Codex setup, doctor, scout, and server commands |
| `repotracer-bench` | Paired benchmark manifests and runners |

## Default Codex backend

The default backend uses the existing Codex subscription through `codex app-server` over local stdio. It retains the process between requests. A temporary Codex home exposes current authentication and active model-provider settings while excluding personal instructions, skills, hooks, plugins, and MCP servers. Configuration and authentication file fingerprints are checked before reuse; changes retire the old process. No separate API key or gateway is required.

Conversation reuse is independent. The MCP server generates and returns a handle when `investigation.conversation_id` is omitted. Supply the returned handle for a related follow-up, or a caller-owned ID from the first request. The handle remembers the canonical repository. `repository` can explicitly select a different checkout from the server startup directory; Git can also resolve an absolute focus in another checkout. Different repositories require different handles. Reuse is best-effort: idle expiry, thread budgets, configuration changes, errors, and process recycling can start a fresh conversation. `conversation.status` reports resumed, fresh, or unknown from the first attempt's native thread counter, not the warm-process flag. Supply the current objective and necessary context. Freshness is a model instruction, not a filesystem-snapshot guarantee.

Session defaults are engineering bounds, not benchmark-derived optima: 300 idle seconds, two idle processes, four requests per continued conversation, a 120,000-token previous-input ceiling, and 32 created threads per process. Process recycling bounds server-retained old threads. `max_warm` limits idle sessions, not active concurrency. Named conversations are retained separately, including within one repository, so A → B → A can resume while the cache budget permits it. Independent MCP requests execute concurrently and complete responses are written one at a time with their original JSON-RPC IDs. A per-handle gate orders calls to the same conversation across the entire adaptive operation. Startup is capped at 60 seconds, or the shorter configured inactivity timeout; model inactivity timeout remains separately configurable.

The MCP handle registry stores up to 1,024 root bindings and ordering locks, not
source or model history. It evicts the oldest inactive binding when full and
never evicts an active gate. Native idle-session retention is a separate,
smaller budget. Generated handles missing after eviction or server restart
require an explicit repository rather than silently falling back to startup.

The child process receives:

- The repository root
- The scout query
- RepoTracer's read-only scout instructions
- Read-only filesystem access
- A restricted capability set without inherited MCP servers, apps, plugins, browser, image, or multi-agent tools

The child returns text plus structured citation metadata. RepoTracer rejects paths outside the repository, missing files, invalid line ranges, and symlink escapes before returning the result to the MCP client.

## Scout-requested reasoning

Native backends also use `AdaptiveScout` to permit one scout-requested
higher-effort continuation. The initial configured or parent-selected effort
does not change. Native model metadata determines which higher settings can
be offered, and positive turn ceilings prevent spending that budget twice.
`[model].adaptive_reasoning = false` disables this behavior.

Codex can reuse the conversation at higher effort subject to its existing
reuse limits. Claude currently restarts its native process when effort changes.
The continuation carries the original request and first findings in both cases.
It returns a revised full report; a failed or unusable continuation retains
the first findings as partial. Two-attempt statistics preserve cache-aware
usage and reported cost, with missing dimensions left unknown. Native metadata
does not prove the applied effort when an organization silently caps it.

## OpenAI-compatible backend

Custom GPT endpoints use RepoTracer's native tool loop:

```text
query + system prompt
  → model response
  → validate tool calls
  → execute independent Read / Glob / Grep / Symbols calls concurrently
  → append bounded results
  → repeat until final answer or limit
  → parse and validate citations
```

Set the backend in the CLI or config file. `REPOTRACER_API_KEY` supplies endpoint authentication when required.

## Limits

The generic OpenAI-compatible engine enforces:

- Maximum model turns
- Maximum repository tool calls
- Per-tool timeout
- Total scout timeout
- Tool-result byte limits

Each MCP answer representation has a 36 KiB serialized ceiling. Text and structured output are compatible alternatives, so neither is shortened just to make room for the other on the wire. This is a safeguard, not a target answer length or a demonstrated quality optimum. There are no intent-specific citation counts or per-citation excerpt limits.

Do not assume native engine turn/tool limits apply to Codex's internal agent loop. Subscription requests use the read-only Codex sandbox, startup/inactivity bounds, and bounded local Symbols results. MCP cancellation drops the targeted handler future, releasing its conversation guard and owned native session. Native child processes use `kill_on_drop`. This verifies local cancellation, not upstream billing cancellation or guaranteed termination of every descendant. Missing provider usage remains unknown.

The stdio dispatcher accepts up to 16 executing requests and 16 pending
requests. It continues reading cancellation notifications at full capacity;
overflow requests receive a queue-full error before model startup. If a client
stops reading and rejection replies fill the bounded output buffer, the server
closes that connection and cancels its active work rather than blocking input.
Active cancellation drops the handler; pending cancellation removes the request
before it reaches the backend. Unknown, completed or malformed cancellation
targets are ignored, and numeric IDs remain distinct from string IDs. The
notification receives no response. Normal EOF drains accepted work. A frame
being read must retain its parser state while other requests complete.

Claude native I/O uses the configured stream inactivity timeout, not a total
investigation deadline. Zero disables that timeout. Pre-terminal failures
preserve observed tool-call counts and elapsed time, without inventing token
usage or cost when the native process has not reported them.

Read, Glob, and Grep return bounded output with continuation information. This prevents a single file or search from filling the scout context.

## Syntax navigation

`Symbols` and `repotracer symbols` reuse upstream Tree-sitter tag queries for Rust, Python, JavaScript, TypeScript/TSX, and Go. They return definitions, name references, or an outline. Content hashes invalidate changed files; deletion and incomplete scans are handled explicitly. The in-memory cache belongs to the long-lived scout, independently of its provider processes.

This is not a resolved call graph, persistent disk index, or Aider-style ranked map. Hidden/ignored/generated files and unsupported languages may be excluded. Results report scope and limitations; text search remains necessary.

Codex replies include per-investigation `stats.index_usage`: calls, failures,
parsed/reused files, incomplete calls, duration, and output bytes. Counts do
not include source or search terms. `available: true` with zero calls means
the index was offered but unused. Claude reports `available: false`; absent
telemetry means unknown, as with older backends. These counters do not measure
answer quality or prove the index saved tokens.

## MCP result

`repo_scout` returns:

- An explanation and findings for locate, explain, change-impact, diagnose, or inventory intent
- Complete, partial, not-found, or failed status, plus unresolved questions, searched scope, and limitations
- Validated `path:start-end` citations
- Source text once per selected rendering, with overlapping or adjacent spans merged
- Truncation metadata
- The recommended next repository action
- Scout usage and timing when the backend reports them

RepoTracer does not edit repository files. The parent Codex process owns edits, commands, and verification.

Citation validation proves locations exist, not that claims are true. The scout reports whether the objective is answered. Rust checks output integrity and citation locations; matching question labels or counting findings cannot establish semantic completeness. One finding may address several questions.

Handoff version 4 keeps both supported renderings self-contained.
`structuredContent.report` contains the explanation and `evidence[].text`
contains line-numbered source. Citation locations, source omissions, conversation
metadata, continuation requests, and usage remain structured fields.
`content[0].text` is a full readable fallback. Callers should forward either
representation, not the entire envelope with both compatibility copies.

Version 4 removes the version-3 fields `investigation`, `next_action`,
`handoff_limitations`, and `omitted_citations`. Consumers that need those fields
must keep their version-3 parser separate from the version-4 parser. Failed
investigations, including a configured total timeout, set the MCP envelope's
`isError` to true while retaining the report, usage, and conversation metadata.
Partial and not-found results are not tool errors. Source attachment failures
also preserve the answer and carry explicit delivery warnings.

Version 2 used cross-field byte references, which were unsafe for clients that
retain only one rendering. Versions 3 and 4 instead embed report and source text
in each rendering. The smoke reader accepts versions 1 through 4. The CLI's own
`ScoutResult` JSON and the package version are unchanged by this protocol bump.

The formatter returns the selected source ranges without the former 36 KiB
handoff ceiling or source eviction. Overlapping ranges share one source block.
Each MCP citation adds `source_status`: `included`, `truncated`, or `omitted`.
Attachment failures add `source_error`, with details also available under
`evidence_omissions.errors`; the text fallback names unavailable ranges. These
fields describe source delivery, not claim confidence.

The parent passes relevant task requirements in the existing query, separately
from assumptions about current code. The scout does not inherit the parent
conversation. Supplied requirements guide the requested investigation; they
are not claims that the feature already exists. Completion describes the
investigation, not implementation progress. Missing requirements that block
the answer and unresolved source behavior should be identified separately.
No automatic filter removes real unresolved questions or upgrades confidence.

Subscription usage uses cumulative thread updates with request-local deltas when available. Missing counters and fallback snapshots remain qualified. Task comparisons include parent and scout usage, with cached tokens counted at their documented weights. These API-equivalent figures are not subscription invoices. Instrumented Codex trials retain native per-request usage and tool outputs to check accounting and what reached the parent; they do not capture provider HTTP payloads.

Claude Code's `total_cost_usd` is cumulative across a streaming-input session.
The Claude adapter reports the increase since the preceding result, including
terminal failures, and resets its baseline for a new process. Missing or
regressing snapshots make that request's cost unknown. Its separate `usage`
object is retained as reported; it must not be presented as covering every
query-pipeline helper call. The native `modelUsage` totals cover that wider
scope and are not yet retained by this adapter.

## Security

- Repository-root enforcement on every local tool call
- Symlink-escape rejection
- Read-only child sandbox
- No shell interpolation in repository tools
- Citation validation before MCP output
- Provider credentials owned by the provider CLI
- No telemetry by default
