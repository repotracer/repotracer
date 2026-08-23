# Show HN draft

**Title:** Show HN: repotracer – Stop paying frontier models to search your repo

**Body:**

repotracer is a local repository scout for coding agents (Claude Code, Codex, MCP).

Idea: a small specialist model owns the Read/Glob/Grep exploration loop and returns validated file:line citations. The frontier model solves instead of grepping.

```bash
npx repotracer setup
repotracer scout "trace refresh token rotation"
```

Why we built it:

- Frontier models waste context on exploratory search
- “Token savings” middleware often measures the wrong thing
- We benchmark complete-task cost (the bill), not characters filtered

Inspired by Microsoft FastContext; production runtime is Rust (concurrent tools, citation validation, one-command setup).

Not always cheaper on trivial edits — and we say so.

Repo: https://github.com/repotracer/repotracer
