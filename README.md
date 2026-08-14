<p align="center">
  <img src="assets/grephound-avatar.png" width="128" alt="Grephound">
</p>

<h1 align="center">grephound</h1>

<p align="center"><strong>Stop paying frontier models to search your repo.</strong></p>

<p align="center">
  <a href="#install"><img src="https://img.shields.io/badge/install-npx%20grephound%20setup-7EE787?style=for-the-badge&labelColor=0B0F14" alt="Install grephound"></a>
  <a href="#mcp"><img src="https://img.shields.io/badge/MCP-repo__scout-79C0FF?style=for-the-badge&labelColor=0B0F14" alt="MCP repo_scout"></a>
  <a href="BENCHMARKS.md"><img src="https://img.shields.io/badge/benchmarks-measure%20the%20bill-F85149?style=for-the-badge&labelColor=0B0F14" alt="Benchmarks"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-8B949E?style=for-the-badge&labelColor=0B0F14" alt="MIT license"></a>
</p>

<p align="center">
Codex only
</p>

<p align="center">
  <img src="assets/hero.png" alt="grephound architecture: frontier model calls repo_scout, GPT-5.6 Luna explores, validated evidence returns" width="920">
</p>

```bash
cargo install --git https://github.com/grephound/grephound --locked grephound
grephound setup
```

---

## Your $20/M-token model is doing grep duty

Normal coding agents waste the expensive model on exploration:

```text
Codex
  → grep
  → read
  → read
  → glob
  → grep
  → read
  → grep
  → read
  → finally understand the repo
  → solve
```

Every hop enters frontier context. Every hop gets re-read from cache. Every hop bills.

**grephound changes the architecture:**

```text
Codex
  → repo_scout("trace refresh token rotation")
       ↓
  GPT-5.6 Luna scout
  read-only repository exploration
       ↓
  validated file:line evidence
       ↓
  frontier solver gets one bounded handoff
       ↓
  solve
```

The scout searches. The frontier solves. That separation is the product.

<p align="center">
  <img src="assets/architecture.png" alt="without grephound vs with grephound" width="920">
</p>

---

## Token savings are easy to fake

Delete 98% of a command’s output and you can claim “98% fewer tokens.”

That does **not** mean your agent became 98% cheaper.

JetBrains proved this on real paired Claude Code work:

| Tool | Advertised | Measured complete-task cost |
|------|------------|-----------------------------|
| **rtk** | 60–90% savings | **+7.6% more expensive** (low effort, 80 pairs) |
| **Caveman** | ~65% savings | **~8.5%** actual |
| **Ponytail** | code-size claims | test it yourself — measure the bill |

Source: [JetBrains RTK trial](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/), [Caveman trial](https://blog.jetbrains.com/ai/2026/07/speak-to-ai-agents-like-cavemen-tosave-tokens/), [Ponytail trial](https://blog.jetbrains.com/ai/2026/07/ponytail-skill-claude-tested/).

rtk’s own scoreboard claimed **96M tokens “saved”** while the **invoice went up**.

Why middleware counters lie:

1. They grade their own homework (raw output vs compressed output).
2. Agents already truncate giant tool results.
3. Most cost is cached re-reads, turns, and recovery searches — not one shell dump.
4. Compressing one step changes the whole feedback loop.

### Our rule

> **If the invoice didn’t shrink, you didn’t save tokens.**

> **We don’t count characters we filtered. We measure the entire coding task.**

| We measure | We refuse to market |
|------------|---------------------|
| Total provider $ | “tokens avoided” |
| Main input / cache / output | Bytes deleted from one tool |
| Explorer cost included | Unpaired single runs |
| Turns + latency | Quality-blind compression |
| Task success | README fan fiction |

<p align="center">
  <img src="assets/benchmark-card.png" alt="benchmark card placeholder — paired complete-task economics" width="920">
</p>

> Headline Δ numbers ship only after repeated paired runs. No fake savings. Ever.

Methodology: [BENCHMARKS.md](./BENCHMARKS.md) · [why token counters lie](./docs/benchmarks/why-token-counters-lie.md)

Latest repeated same-task diagnostic: Sol + Luna cut total provider cost by **up to 58.81%**, with a **28.56% paired median across three repeated pairs on one task**. Every arm passed 6/6 checks; median wall time was **31.21% slower**. This is optimization evidence, not a cross-task headline. [Full bill and artifact →](./BENCHMARKS.md#repeated-isolated-luna-routing-diagnostic--2026-08-10)

Follow-up ablation: Luna medium passed 3/3 runs inside the production timeout. Extended-ceiling runs corrected the earlier inference—low, high, and Stage B can finish, but Stage B still lost on quality and complete bill. Medium remains provisional until natural multi-task validation. [All retained and rejected results →](./BENCHMARKS.md#reasoning-stage-b-and-local-model-diagnostics--2026-08-11)

---

## Install

```bash
cargo install --git https://github.com/grephound/grephound --locked grephound
grephound setup
```

Setup asks no questions. It uses the installed Codex CLI and its existing login, pins the scout to `gpt-5.6-luna`, writes `~/.grephound/config.toml`, and configures Codex. No API key, model download, Python environment, or background service.

Prerequisite: install Codex and sign in once if it is not already ready:

```bash
npm install -g @openai/codex
codex login
grephound setup
grephound doctor
```

| Host | Installed integration |
|------|-----------------------|
| Codex | MCP server + automatic routing instructions |

Inspect the exact setup actions without writing files:

```bash
grephound setup --dry-run
```

Advanced: route a GPT model through an OpenAI-compatible endpoint instead of the Codex CLI:

```bash
grephound --base-url https://models.example.com/v1 --model gpt-5.6-mini setup
```

Grephound currently accepts GPT scouts only. Remove its config and agent integrations with `grephound uninstall --yes`; the Codex installation and login remain untouched.

```bash
grephound scout "Trace how refresh tokens are created, validated, rotated, and revoked."
```

```text
Found 4 relevant locations in 1.2s

src/auth/refresh.ts:44-108
  Validates and rotates refresh tokens.

src/auth/session.ts:81-144
  Creates the replacement session.

src/store/tokens.ts:31-79
  Persists and revokes token state.

tests/auth/refresh.test.ts:91-173
  Rotation and reuse-detection tests.

Scout: gpt-5.6-luna
Model steps: 3
Tool calls: 9
```

<p align="center">
  <img src="assets/demo.gif" alt="15s grephound demo" width="920">
</p>

---

## Why grephound is not “another MCP grep”

### vs RTK / shell output compressors

RTK filters **Bash** output after the frontier already decided to shell out.
Claude’s `Read` / `Grep` bypass it. JetBrains measured **no savings** on complete tasks.

grephound removes the frontier from the exploration loop entirely.

Different layer. Different claim. We still measure the bill.

### vs Context Mode

Context Mode manages / compresses what enters expensive context. Useful idea.

grephound’s bet is stronger: **don’t put exploration in the expensive model at all.**

Compression is damage control. Delegation is architecture.

### vs Serena

Serena exposes semantic / symbol tools to the **main** agent.
The expensive model still orchestrates hops and eats results.

grephound: one call → specialist owns the multi-step search → citations only.

### vs jCodeMunch

Excellent symbol-level retrieval. Wrong product for:

> “What happens between the OAuth callback and session persistence, including error paths?”

That needs autonomous exploration, not a symbol table lookup.

### vs FastCtx

FastCtx is a better local tool runtime (read/grep/glob/bash for the main agent).

If the finished project can be described as “grep through MCP but nicer,” **we failed.**
Our differentiator is the **autonomous scout layer**.

### vs JetBrains Context

JetBrains Context is real: IDE / ecosystem repository intelligence.

grephound is:

- open source
- agent-independent
- local specialist explorer
- MCP + CLI
- works outside one IDE tax surface

### vs “just use grep”

Grep is free. **Deciding every grep from a frontier model is not.**

The bill is reasoning + tool results + cache churn + turns. Not the `rg` binary.

### vs FastContext (research)

Microsoft FastContext proved the thesis: train/use a small model for delegated repo exploration ([paper](https://arxiv.org/abs/2606.14066), [model](https://huggingface.co/microsoft/FastContext-1.0-4B-RL)).

grephound is the **product**:

| FastContext research UX | grephound |
|-------------------------|-----------|
| Python / env ceremony | one native binary |
| Sequential tool await bug | concurrent tools |
| `count` / `count_matches` mismatch | fixed `count` → `--count-matches` |
| DIY MCP JSON | `grephound setup` for Codex |
| Benchmarks for papers | complete-task bill benchmarks for users |

Not affiliated with or endorsed by Microsoft. See [NOTICE](./NOTICE).

---

## How it works

1. Agent calls **`repo_scout`** with a natural-language question
2. Specialist model explores with read-only **Read / Glob / Grep**
3. Independent tool calls run **concurrently**
4. Final `<final_answer>` citations are **path + line validated**
5. Frontier model reads only those regions and ships the patch

**The scout can search your code. It cannot change it.**

Editing authority stays with the expensive agent. On purpose.

---

## When it should fire (and when it shouldn’t)

**Use scout for**

- unfamiliar repos
- cross-file behavior
- auth / payments / cache invalidation traces
- “find implementation + tests + config”
- impact analysis before a risky edit

**Skip scout for**

- known file, known line
- typo in README
- single obvious symbol already in context
- pure formatting

A 4B multi-hop scout on a one-line rename is how you lose the plot. We optimize for adoption quality, not forced usage cosplay.

---

## CLI

```bash
grephound "where is auth handled?"     # shorthand
grephound scout "trace refresh rotation"
grephound serve                        # MCP stdio
grephound setup
grephound doctor
grephound status
grephound config --init
grephound uninstall --yes
grephound benchmark
```

Machine output: `--json` on scout / doctor / status.

---

## GPT scout backend

### Default: GPT-5.6 Luna through Codex

Grephound invokes the installed Codex CLI as a disposable read-only subprocess. Codex owns login and token refresh; Grephound never reads or stores subscription credentials. Each scout run is ephemeral, ignores inherited user rules and MCP servers, preserves only provider routing, and returns strict structured evidence that Grephound validates against the repository.

```toml
[model]
backend = "codex-cli"
model = "gpt-5.6-luna"
timeout_ms = 120000
```

### Custom GPT endpoint

An OpenAI-compatible GPT endpoint can run Grephound's internal Read / Glob / Grep loop:

```toml
[model]
backend = "openai-compatible"
model = "gpt-5.6-mini"
base_url = "https://models.example.com/v1"
```

Set `GREPHOUND_API_KEY` when the endpoint requires a token. Grephound rejects non-GPT model names and never silently falls back to another backend.

---

## Privacy

```text
repo → GPT scout → bounded validated evidence → your coding agent
```

- No Grephound telemetry by default
- Scout execution is read-only
- Repository snippets go only to the GPT provider configured through Codex or the custom endpoint

---

## FAQ

**Is it always cheaper?**
No. Trivial tasks can gain nothing. That’s honesty, not a dodge. We recommend scout for exploration-heavy work and we benchmark complete tasks.

**Does the small model edit my repo?**
No.

**Why should I trust your savings claims?**
You shouldn’t until the paired bill says so. Challenge us via the Benchmark discrepancy issue template.

**Is this just FastContext with a logo?**
No. FastContext is the research insight + model. grephound is the production scout runtime, packaging, MCP product surface, concurrent tools, citation trust layer, and complete-task benchmark standard.

**Will you publish fake “97% tokens saved” charts?**
No. That’s the category we’re killing.

---

## Develop

```bash
cargo test --workspace
cargo run -p grephound -- doctor
cargo run -p grephound -- scout "where is config loaded?" --mock
```

- Architecture: [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)
- Launch drafts: [docs/launch/](./docs/launch/)

---

## Research credit

Microsoft **FastContext** — [paper](https://arxiv.org/abs/2606.14066) · [model](https://huggingface.co/microsoft/FastContext-1.0-4B-RL) · [source](https://github.com/Cirius1792/fastcontext)

JetBrains for doing the industry a favor by **measuring complete-task bills** instead of cosplaying savings counters.

---

## License

MIT · [LICENSE](./LICENSE) · [NOTICE](./NOTICE)
