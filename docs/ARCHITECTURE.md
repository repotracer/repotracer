# Architecture

## Request path

```text
Codex Sol
  → MCP call: repo_scout(query)
    → RepoTracer MCP server
      → isolated codex app-server thread
        → GPT-5.6 Luna, medium reasoning
          → Read / Glob / Grep
      ← structured scout result
    ← validated citations and source excerpts
  → read cited code
  → edit and verify
```

`repotracer setup` registers the stdio MCP server and writes a managed routing block into Codex instructions. The routing block selects `repo_scout` for unfamiliar or cross-file exploration and skips it when the prompt already identifies the file or symbol.

## Components

| Crate | Responsibility |
|---|---|
| `repotracer-repo-tools` | Read, Glob, Grep, root checks, and concurrent execution |
| `repotracer-model` | OpenAI-compatible model client and mock backend |
| `repotracer-core` | Scout loop, prompts, configuration, and citation parsing |
| `repotracer-mcp` | MCP stdio server and `repo_scout` schema |
| `repotracer` | CLI, Codex setup, doctor, scout, and server commands |
| `repotracer-bench` | Paired benchmark manifests and runners |

## Default Codex backend

The default backend launches `codex app-server` over local stdio and creates an ephemeral GPT-5.6 Luna thread. A temporary Codex home exposes the current authentication and active model-provider settings while excluding personal instructions, skills, hooks, plugins, and MCP servers from the scout. RepoTracer rebuilds that small provider config for each scout, so provider changes are picked up without maintaining a second Codex installation.

The child process receives:

- The repository root
- The scout query
- RepoTracer's read-only scout instructions
- Read-only filesystem access
- A restricted capability set without inherited MCP servers, apps, plugins, browser, image, or multi-agent tools

The child returns text plus structured citation metadata. RepoTracer rejects paths outside the repository, missing files, invalid line ranges, and symlink escapes before returning the result to the MCP client.

## OpenAI-compatible backend

Custom GPT endpoints use RepoTracer's native tool loop:

```text
query + system prompt
  → model response
  → validate tool calls
  → execute independent Read / Glob / Grep calls concurrently
  → append bounded results
  → repeat until final answer or limit
  → parse and validate citations
```

Set the backend in the CLI or config file. `REPOTRACER_API_KEY` supplies endpoint authentication when required.

## Limits

The engine enforces:

- Maximum model turns
- Maximum repository tool calls
- Per-tool timeout
- Total scout timeout
- Tool-result byte limits
- Citation count and evidence byte limits
- Cancellation when the MCP request closes

Read, Glob, and Grep return bounded output with continuation information. This prevents a single file or search from filling the scout context.

## MCP result

`repo_scout` returns:

- A concise finding
- Validated `path:start-end` citations
- Bounded source excerpts
- Truncation metadata
- The recommended next repository action
- Scout usage and timing when the backend reports them

RepoTracer does not edit repository files. The parent Codex process owns edits, commands, and verification.

## Security

- Repository-root enforcement on every local tool call
- Symlink-escape rejection
- Read-only child sandbox
- No shell interpolation in repository tools
- Citation validation before MCP output
- Provider credentials owned by the provider CLI
- No telemetry by default
