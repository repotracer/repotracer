<p align="center">
  <img src="assets/grephound-avatar.svg" width="128" alt="Grephound logo">
</p>

<h1 align="center">grephound</h1>

<p align="center"><strong>Stop paying frontier models to search your repo.</strong></p>

**grephound** gives Claude Code, Codex, Cursor and other coding agents a small dedicated repository scout.

**Small models search. Big models solve.**

```bash
npx grephound setup
```

Works with: **Claude Code · Codex · Cursor · OpenCode · MCP**

---

## The problem

```text
WITHOUT grephound

Claude / Codex
  ↓ grep
  ↓ read
  ↓ read
  ↓ glob
  ↓ grep
  ↓ read
  ↓ finally solve

All exploration enters expensive context.
```

```text
WITH grephound

Claude / Codex
       ↓
  repo_scout("trace auth")
       ↓
   Local 4B scout
   ↙   ↓   ↘
Read Grep Glob
   ↘   ↓   ↙
  3 citations
       ↓
Claude reads 3 focused regions
       ↓
solve
```

---

## Install

```bash
# from source (today)
cargo install --path crates/cli

# or
npx grephound setup
```

```bash
grephound setup
grephound doctor
grephound scout "where is authentication handled?"
```

---

## Token-saving counters are easy to fake

Delete 98% of a command's output and you can claim “98% fewer tokens.”

That doesn't mean your agent became 98% cheaper.

**We don't count characters we filtered. We measure the entire coding task.**

If the invoice didn't shrink, you didn't save tokens.

See [BENCHMARKS.md](./BENCHMARKS.md) and [docs/benchmarks/why-token-counters-lie.md](./docs/benchmarks/why-token-counters-lie.md).

> Benchmark numbers will appear here only after paired runs. No fake savings.

---

## How it works

1. Your coding agent calls one tool: `repo_scout`
2. A small specialist model explores with **read-only** `Read`, `Glob`, `Grep`
3. Independent tool calls run **concurrently**
4. Citations are **validated** (path + line bounds, no escapes)
5. The frontier model reads only those regions and solves

The scout can search your code. **It cannot change it.**

---

## CLI

```bash
grephound "where is auth handled?"          # shorthand
grephound scout "trace refresh token rotation"
grephound serve                             # MCP stdio
grephound setup
grephound doctor
grephound status
grephound config --init
grephound uninstall --yes
```

Example output:

```text
Found 3 relevant locations in 1.2s

src/auth/refresh.ts:44-108
  Validates and rotates refresh tokens.

src/auth/session.ts:81-144
  Creates the replacement session.

tests/auth/refresh.test.ts:91-173
  Rotation and reuse-detection tests.

Scout: fastcontext
Turns: 3
Tool calls: 9
```

---

## MCP

Primary tool:

| Tool | Input | Output |
|------|--------|--------|
| `repo_scout` | `{ "query": "..." }` | summary + validated file:line citations |

Tool description is tuned so agents call it for exploration-heavy work and skip it for trivial edits.

```bash
grephound serve   # stdio MCP — never writes non-protocol text to stdout
```

---

## Local model

Default: OpenAI-compatible endpoint at Ollama.

```bash
ollama serve
# Use the official FastContext model when available in your registry, or any tool-calling model:
ollama pull <your-explorer-model>
```

Config (`~/.grephound/config.toml`):

```toml
[model]
backend = "ollama"
model = "fastcontext"
base_url = "http://127.0.0.1:11434/v1"

[explorer]
max_turns = 6
timeout_seconds = 60
```

Official specialist model: [microsoft/FastContext-1.0-4B-RL](https://huggingface.co/microsoft/FastContext-1.0-4B-RL)

---

## Examples

```bash
grephound scout "Trace how refresh-token reuse is detected and where sessions are revoked."
grephound scout "Find the full request path from POST /checkout to Stripe and identify rollback behavior."
grephound scout "Which code paths invalidate the build cache after package metadata changes?"
grephound scout "Find the implementation, tests, and configuration involved in retry backoff."
```

---

## Privacy

Local setup:

```text
your repository → local scout model → citations → your coding agent
```

No telemetry by default. No source code leaves the machine for the scout when using a local backend.

If you point the scout at a remote model endpoint, code snippets may reach that provider.

---

## FAQ

**Why not just use grep?**  
Because the expensive model has to decide every grep/read step and consume the outputs. The scout owns that loop.

**Does the small model edit my code?**  
No.

**Is it always cheaper?**  
No. Trivial tasks may gain nothing. That's why agents should use the scout for exploration-heavy work — and why we benchmark complete tasks.

**Why not Serena / jCodeMunch / Context Mode / RTK?**  
Different layers. grephound delegates the entire exploration loop to a specialist model and returns validated citations. See the comparison notes in the docs.

---

## Develop

```bash
cargo test --workspace
cargo run -p grephound -- scout "where is config loaded?" --mock
cargo run -p grephound -- doctor
```

Architecture: [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)

---

## Research credit

Built on insights from Microsoft **FastContext** ([paper](https://arxiv.org/abs/2606.14066), [model](https://huggingface.co/microsoft/FastContext-1.0-4B-RL), [source](https://github.com/Cirius1792/fastcontext)).

Production runtime, concurrent tools, packaging, MCP product surface, and complete-task benchmarks are grephound's.

Not affiliated with or endorsed by Microsoft. See [NOTICE](./NOTICE).

---

## License

MIT — [LICENSE](./LICENSE)
