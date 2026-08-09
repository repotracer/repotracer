# Why token counters lie

A tool cannot prove savings by comparing its output against the output it chose not to send.

The coding agent is a feedback loop. Compressing one step can change everything that happens afterward:

- more turns
- rereads
- bypassed tools
- different cache behavior
- compensating searches
- lost evidence → worse patches → retries

Middleware “97% savings” counters measure **their own filter**, not your invoice.

## Precedent

JetBrains' RTK paired evaluation showed that an arm marketed around token savings can still increase **complete-task** cost:

https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/

Related evaluation culture:

- https://blog.jetbrains.com/ai/2026/07/speak-to-ai-agents-like-cavemen-tosave-tokens/
- https://blog.jetbrains.com/ai/2026/07/ponytail-skill-claude-tested/

## Our standard

1. Same tasks, same model, same agent
2. Measure total provider cost including explorer cost
3. Measure success, not only tokens
4. Report median + variance on repeated pairs
5. Publish raw artifacts

**Benchmark the bill or shut up about token savings.**
