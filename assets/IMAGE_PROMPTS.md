# Grephound image generation prompts

Feed these to GPT Image / ChatGPT image gen one at a time.

**Global style (append to every prompt):**

```text
Developer-tool aesthetic, dark GitHub-adjacent palette (#0B0F14 background, #161B22 panels, #E6EDF3 text, #7EE787 accent green, #F85149 attack red, #79C0FF info blue). Flat clean UI graphics, sharp type (Inter / SF Pro / IBM Plex Sans), generous padding, no stock photos, no humans, no 3D plastic AI brains, no neon cyberpunk overload, no watermark, no fake UI chrome of real products (no Claude/Cursor logos). High contrast, readable at half size, export-ready for GitHub README.
```

Save outputs into `assets/` with the filenames below.

---

## 1. `hero.png` — above-the-fold hero (1280×720)

```text
Create a GitHub README hero graphic for grephound, a local repository scout for AI coding agents.

Left third: large wordmark "grephound" in white, subtitle "Stop paying frontier models to grep." in muted gray, accent line "Small models search. Big models solve." in electric green.

Right two-thirds: clean architecture flow diagram.
Top box: "Frontier model (Claude / Codex)"
Arrow down to green box: repo_scout("trace auth")
Arrow down to dark box: "Local 4B scout"
Three small parallel arrows to Read, Grep, Glob chips
Arrows merge into "3 validated citations"
Arrow back up-right to "Frontier solves"

Tiny footer: "npx grephound setup"

Square-ish composition safe for 16:9 crop. No fake charts. No photographs.
```

---

## 2. `architecture.png` — without vs with (1400×800)

```text
Split-panel technical comparison diagram titled "Your frontier model should not own repository exploration."

LEFT panel, red-tinted border, header "WITHOUT grephound":
Vertical chain of expensive steps from "Frontier model" through many boxes: grep → read → glob → grep → read → read → solve.
Annotation in red: "All exploration enters expensive context."
Small caption: "More turns. More cache. Higher bill."

RIGHT panel, green-tinted border, header "WITH grephound":
"Frontier model" → single green call repo_scout(query) → "Local 4B scout" with concurrent Read/Grep/Glob → "Validated citations" → "Frontier solves on focus only."
Annotation in green: "Scout searches. Frontier solves."

Bottom center thin banner: "Benchmark the bill — not an imaginary token counter."

Crisp infographic, monospaced labels OK, no clutter.
```

---

## 3. `benchmark-card.png` — credibility card (1200×630)

```text
Dark social/README card that attacks fake token savings.

Big headline top: "Token-saving counters are easy to fake."

Middle section three columns with red X icons:
1. "Delete 98% of output"
2. "Claim 98% savings"
3. "Invoice unchanged (or up)"

Lower section green check row:
"grephound measures complete-task provider cost · explorer included · quality counted"

Small citation line at bottom:
"JetBrains RTK paired trial: advertised 60–90% savings → measured +7.6% cost at low effort"

Include a simple empty results table silhouette with headers:
Vanilla | grephound | Δ
and cells filled with "—" or "TBD after paired runs" — do NOT invent percentages.

Footer: "If the invoice didn’t shrink, you didn’t save tokens."
```

---

## 4. `competitive-map.png` — category kill chart (1400×900)

```text
Clean 2×3 competitive map for coding-agent context tools. Title: "Different layers. Most of them still make the frontier do the search."

Six cards:

1. RTK / shell compressors
   Tag: "Filter Bash output"
   Verdict in red: "Doesn’t own exploration. JetBrains: bill went up."

2. Context Mode
   Tag: "Compress / externalize context"
   Verdict: "Damage control after exploration already started."

3. Serena
   Tag: "Symbol tools for main agent"
   Verdict: "Main model still orchestrates every hop."

4. jCodeMunch
   Tag: "Symbol retrieval"
   Verdict: "Great lookup. Weak open-ended multi-file investigation."

5. FastCtx
   Tag: "Better local tools"
   Verdict: "Nicer grep/read — still frontier-driven."

6. grephound (highlighted green border)
   Tag: "Delegated scout"
   Verdict in green: "One call. Specialist explores. Validated citations."

Center bottom arrow into grephound: "Architecture, not middleware cosplay."

No stolen logos. Text labels only.
```

---

## 5. `flow-detail.png` — engine internals (1200×800)

```text
Horizontal pipeline diagram titled "What repo_scout actually does".

Stages left to right with numbered circles:
1 Query
2 Specialist prompt
3 Model turn
4 Concurrent tools (Read/Grep/Glob fan-out then fan-in)
5 More turns until final_answer
6 Parse citations
7 Validate path + lines + repo bounds
8 Compact result to frontier

Callout bubbles:
- "Read-only"
- "Bounded concurrency"
- "No symlink escape"
- "Bad citations dropped"

Subtitle: "The small model’s text is not truth. Citations are the trust layer."
```

---

## 6. `social.png` — GitHub / X social preview (1280×640)

```text
Bold Open Graph image.

Left: grephound avatar-style green hound mark (simple geometric hound + branch motif) and huge type:
grephound

Right stacked lines:
Small models search.
Big models solve.

Bottom bar:
Stop paying frontier models to grep.   ·   npx grephound setup

Minimal. Memetic. High contrast. No paragraph text.
```

---

## 7. `demo-storyboard.png` — frames for a 15s GIF (1920×1080)

```text
4-panel storyboard on one canvas for a terminal demo GIF (not an animated GIF — static frames).

Panel 1: terminal running `npx grephound setup` with green checkmarks Claude Code / Ollama / MCP.
Panel 2: Claude-like agent UI bubble: user asks "Trace refresh-token rotation".
Panel 3: tool call repo_scout(...) then "Scout searched 31 files in 0.9s" with 3 file:line citations.
Panel 4: agent answers from those citations only; tiny footer "frontier never grepped".

Look like a polished product demo screenshot collage. Fictional UI, not real Claude branding.
```

---

## 8. `invoice-vs-counter.png` — meme-level attack visual (1080×1080)

```text
Square meme-infographic, still professional enough for README.

Top half red:
Big fake dashboard "TOKENS SAVED 97%" with confetti energy, subtitle "middleware self-report".

Bottom half green/black:
Simple invoice line items:
Frontier input
Cache reads
Turns
Explorer
Total ↑

Stamp across the middle in brutalist type:
"BENCHMARK THE BILL"

Footer: "rtk scoreboard: 96M tokens saved. Measured bill: +7.6%."
```

---

## 9. `local-privacy.png` — trust diagram (1200×600)

```text
Simple left-to-right trust flow on dark background.

your repository → local grephound scout (lock icon) → citations only → your coding agent

Annotations:
- scout: read-only
- no default telemetry
- code snippets leave only if YOU pick a remote model endpoint

Title: "Local means local."
```

---

## 10. `readme-strip-badges.png` optional (1600×200)

```text
Thin horizontal strip of fake-but-clean status chips for README decoration (not real shields.io):
Rust · MCP · Claude Code · Codex · Cursor · Read-only scout · Concurrent tools · Bill benchmarks

Dark background, green accents, monospaced labels.
```

---

## After generation checklist

1. Drop files into `assets/` using exact names:  
   `hero.png`, `architecture.png`, `benchmark-card.png`, `competitive-map.png`, `flow-detail.png`, `social.png`, `demo-storyboard.png`, `invoice-vs-counter.png`, `local-privacy.png`
2. Export `social.png` also as GitHub repo social preview.
3. Record a real `demo.gif` later from a live scout run (do not fake terminal output).
4. Never put invented % savings into any graphic.

---

## Ollama note (for humans, not the image model)

Ollama default is **from FastContext docs** (easiest local OpenAI-compatible path + their client’s `api_key or "ollama"`).  
grephound kept it as the zero-friction local backend. The product interface is still any OpenAI-compatible endpoint.
