# Show HN draft

**Title:** Show HN: I cut one repeated Codex task's cost 28.6% by moving repo search to Luna

**Body:**

I came across Microsoft's [FastContext paper](https://arxiv.org/abs/2606.14066v3). Its main idea is simple: use a smaller model for repository exploration when the coding agent does not yet know where to look.

I built RepoTracer to use that pattern with Codex.

Technically, RepoTracer is an MCP server whose `repo_scout` tool starts an isolated GPT-5.6 Luna process with read-only Read, Glob, and Grep tools. It validates the returned paths and line ranges, then returns structured citations, source excerpts, and findings to Codex Sol. Installed routing instructions tell Sol when to call it.

My Codex limits seemed to last longer, but that was only an anecdote. I ran paired tests using the same prompts and repositories instead.

The current routing policy reduced median complete provider cost by 28.63% across three randomized pairs of one cross-file task. Direct and RepoTracer runs passed all six checks. A fixed measured budget would cover about 40% more equivalent runs of that task.

One SWE-bench task was 50.12% cheaper with the exact regression passing in both arms.

This is not a general average. A newer Google authentication task was 62.68% cheaper with a minor scoring difference (78.75 vs 83.125 in blind grading). I kept that result in the benchmark report. RepoTracer is a beta as I continue expanding and validating across more tasks.

```bash
npx repotracer setup
```

Repo: https://github.com/repotracer/repotracer

Benchmarks: https://github.com/repotracer/repotracer/blob/main/BENCHMARKS.md
