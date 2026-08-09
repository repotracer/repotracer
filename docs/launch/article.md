# We stopped paying frontier models to search the repository

## 1. The stupid part of coding-agent economics

Your most expensive model is doing `grep` duty.

## 2. Why grep isn't free

Every exploratory hop enters context, shapes the next hop, and compounds.

## 3. Small-model scout architecture

One tool: `repo_scout(query)` → specialist explores → validated citations → frontier solves.

## 4. Microsoft's FastContext insight

Train/use a small model for autonomous repository exploration with Read/Glob/Grep.

## 5. What we rebuilt

Rust engine, product CLI/MCP, setup UX, concurrent tools, citation trust layer.

## 6. Parallel exploration

One model turn, many independent tools — run concurrently.

## 7. Citation validation

Model text is not truth. Paths and line ranges are checked.

## 8. The RTK/JetBrains lesson

Measure complete tasks. Middleware counters can lie.

## 9. Our paired benchmark

(Insert results only when real.)

## 10–11. Where it wins / doesn't

Exploration-heavy tasks vs trivial known-file edits.

## 12. Install

```bash
npx grephound setup
```
