# I moved repository search from Sol to Luna

I came across Microsoft's [FastContext paper](https://arxiv.org/abs/2606.14066v3), which separates repository exploration from solving: a smaller, faster model searches the code and returns verified locations, so the primary coding model works from concise evidence instead of cluttering its context with exploratory lookups.

I wanted that workflow in Codex, so I built RepoTracer.

RepoTracer is an MCP server. Its `repo_scout` tool starts an isolated GPT-5.6 Luna thread with read-only Read, Glob, and Grep tools. RepoTracer validates every returned path and line range against disk, then returns structured citations, excerpts, and findings back to Codex Sol. An intelligent routing block in `~/.codex/AGENTS.md` ensures Sol delegates broad exploration while handling localized, single-file edits directly.

```text
Sol → MCP repo_scout → Luna → Read / Glob / Grep
Sol ← validated citations and excerpts
Sol → edit and verify
```

Across paired benchmarks on real codebases with identical prompts, repositories, commits, and models:

- **62.68% cost reduction (−24.54% implementation time) on MAH-SWE:** On a complex full-stack bug fix recorded from real developer work, RepoTracer fixed the bug in both arms while stretching development budgets by **2.68×**.
- **50.12% cost reduction on SWE-bench:** On Astropy 13453, RepoTracer produced the exact fix and passed the gold regression at half the normal cost.
- **27.71% cost reduction across DeepSWE:** On multi-language release benchmarks across TypeScript, Python, and Go, RepoTracer delivered 97% feature checks and preserved **1,956 of 1,956 regression checks**, where the direct frontier model dropped 4.
- **4.00/4.00 blind quality rating:** Luna Medium achieved 24/24 perfect evaluations in double-blind grading, proving that cheaper repository exploration does not compromise solution quality.

Every number measures complete task cost, start to finish, with RepoTracer's own model usage counted against it. The repository publishes all paired artifacts and raw outputs.

## Install

```bash
npx repotracer@latest setup
```

Codex must already be installed and signed in. RepoTracer reuses that login, registers the MCP server and routing instructions, and requires no second API key.

- Repository: https://github.com/repotracer/repotracer
- Benchmarks: https://github.com/repotracer/repotracer/blob/main/BENCHMARKS.md
- Architecture: https://github.com/repotracer/repotracer/blob/main/docs/ARCHITECTURE.md
- Paper: https://arxiv.org/abs/2606.14066v3
