# Architecture

## Product thesis

```text
Generic MCP repository tool

Main LLM
 → grep
 → read
 → grep
 → read


grephound

Main LLM
 → repo_scout(question)
      ↓
   small specialist
      → Grep / Glob / Read  (concurrent)
      ↓
   validated citations
 → main LLM reads only those regions
 → solve
```

![Delegated repository exploration](../assets/architecture.png)

## Crates

| Crate | Role |
|-------|------|
| `grephound-repo-tools` | Read, Glob, Grep + concurrent executor |
| `grephound-model` | OpenAI-compatible GPT backend + mock |
| `grephound-core` | Scout engine, citations, config |
| `grephound-mcp` | stdio MCP server (`repo_scout`) |
| `grephound` (cli) | setup / doctor / scout / serve + isolated Codex runner |
| `grephound-bench` | complete-task benchmark harness |

## Host integration

Setup installs Grephound's MCP server and a managed routing policy into Codex. The policy delegates unfamiliar or cross-file exploration before Codex falls back to manual Read/Grep/Glob chains. Other agent hosts are intentionally unsupported until they have equivalent end-to-end evidence.

## GPT scout execution

```text
repo_scout
  ├─ default → isolated ephemeral `codex exec` → GPT-5.6 Luna
  └─ custom  → OpenAI-compatible GPT endpoint → Read / Glob / Grep loop
                                      ↓
                  bounded excerpts + validated citations
```

The zero-config path delegates one exploration to the installed Codex CLI. Codex retains credential and provider ownership. Grephound passes no token, ignores inherited user instructions and MCP servers, constrains the process to read-only access, applies a deadline, parses strict structured output, and validates citations itself.

The custom-endpoint loop is available for GPT-compatible endpoints:

```text
query → system prompt → GPT model
  → tool calls? → validate → execute concurrently → append results → loop
  → final text → parse <final_answer> → validate citations → ScoutResult
```

Hard controls: maximum model steps, maximum tool calls, per-tool timeout, total timeout, output caps, and cancellation.


## Security

- Read-only tools or provider sandbox only
- Local tool repo-root enforcement + symlink escape rejection
- Repo-root, file, and line-range validation on every returned citation
- Subscription credentials remain owned by the official provider CLI
- No shell interpolation
- No telemetry by default

## Repository tools

The custom endpoint loop exposes only read-only `Read`, `Glob`, and `Grep` tools. Independent calls execute concurrently, and Grep `count` maps to ripgrep `--count-matches`.

The public product surface remains one high-level tool: `repo_scout`.
