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
| `grephound-model` | OpenAI-compatible backend + mock |
| `grephound-core` | Scout engine, citations, config |
| `grephound-mcp` | stdio MCP server (`repo_scout`) |
| `grephound` (cli) | setup / doctor / scout / serve + official CLI subscription runners |
| `grephound-bench` | complete-task benchmark harness |

## Host integration

MCP is the portable capability layer. Setup also installs a compact host-native routing policy so agents delegate unfamiliar or multi-file exploration instead of falling back to manual Read/Grep/Glob chains. Claude Code and Codex receive an Agent Skill; Cursor receives a project rule; GitHub Copilot receives project instructions. Hooks are not required for the default path.

## Backend selection

```text
repo_scout
  ├─ ollama / openai-compatible → Grephound tool loop → Read / Glob / Grep
  ├─ codex-cli                  → ephemeral `codex exec` → structured result
  └─ claude-cli                 → safe `claude -p` → structured result
                                      ↓
                            shared citation validation
```

Subscription mode delegates one complete exploration to the provider's official installed CLI. The CLI retains credential ownership. Grephound passes no tokens, disables inherited agent configuration/MCP recursion, constrains execution to read-only operations, applies a process deadline, parses a strict result schema, and validates citations itself.

## Local/custom engine loop

```text
query → system prompt → model
  → tool calls? → validate → execute concurrently → append results → loop
  → final text → parse <final_answer> → validate citations → ScoutResult
```

Hard controls: max turns, max tool calls, per-tool timeout, total timeout, output caps, cancellation via timeout.


## Security

- Read-only tools or provider sandbox only
- Local tool repo-root enforcement + symlink escape rejection
- Repo-root, file, and line-range validation on every returned citation
- Subscription credentials remain owned by the official provider CLI
- No shell interpolation
- No telemetry by default

## FastContext compatibility

Internal tools keep FastContext names and schemas so the specialist model stays effective.
Public product surface is one tool: `repo_scout`.

Upstream fixes included:

1. **Parallel tool execution** (was sequential)
2. **Grep `count` → `--count-matches`** (schema/impl mismatch)

