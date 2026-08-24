# I moved repository search from Sol to Luna

I came across Microsoft's [FastContext paper](https://arxiv.org/abs/2606.14066v3), which separates repository exploration from solving. A smaller model searches the code and returns the relevant locations. The main coding model works from that evidence instead of carrying every exploratory search in its context.

I wanted that workflow in Codex, so I built RepoTracer.

RepoTracer is an MCP server. Its `repo_scout` tool starts an isolated GPT-5.6 Luna process with read-only Read, Glob, and Grep tools. RepoTracer validates the returned paths and line ranges, then sends the citations, excerpts, and findings back to Codex Sol. A managed block in `~/.codex/AGENTS.md` tells Sol when the task is broad enough to use it.

```text
Sol → MCP repo_scout → Luna → Read / Glob / Grep
Sol ← validated citations and excerpts
Sol → edit and verify
```

My Codex limits seemed to last longer after I started using it. That was useful feedback, but it was not evidence. I ran paired benchmarks with the same prompts, repositories, commits, models, and checks.

In three randomized pairs of the same cross-file question, the current routing policy reduced median complete provider cost by 28.63%. Both direct and RepoTracer arms passed all six checks. At that measured rate, a fixed provider budget covers about 40% more equivalent runs of that task.

A single SWE-bench task was 50.12% cheaper and passed the exact regression in both arms.

There is a bad result too. On a real Google authentication implementation task, RepoTracer was 62.68% cheaper but scored 4.375 points below direct Codex in blind grading. It found the right files, but the parent agent tested a copied fixture instead of production configuration. That is why I am launching RepoTracer as a beta rather than claiming the same quality as direct Codex.

Every number includes Luna's usage. The repository keeps the losing runs and raw artifacts alongside the successful ones.

## Install

```bash
npx repotracer setup
```

Codex must already be installed and signed in. RepoTracer reuses that login, installs the MCP server and routing instructions, and requires no second API key.

- Repository: https://github.com/repotracer/repotracer
- Benchmarks: https://github.com/repotracer/repotracer/blob/main/BENCHMARKS.md
- Paper: https://arxiv.org/abs/2606.14066v3
