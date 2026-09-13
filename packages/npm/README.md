# repotracer

A repo investigator for Claude Code and Codex.

RepoTracer installs an MCP tool named `repo_scout`. The coding agent can call it when a task needs repository investigation, then continue with the returned findings instead of spending the same conversation tracing the repo itself.

## Setup

```bash
npx repotracer@latest setup
```

Choose Claude Code, Codex, or both.

RepoTracer uses the native CLI you are already signed into. A second provider login is not required for native investigations.

Run the same command later to reconfigure or uninstall.

## Commands

```bash
npx repotracer@latest "where is authentication handled?"
npx repotracer@latest scout "trace token refresh"
npx repotracer@latest symbols "Config" --mode references
npx repotracer@latest serve
npx repotracer@latest doctor
npx repotracer@latest status
npx repotracer@latest update
npx repotracer@latest uninstall --yes
```

`serve` starts the MCP server over stdio.

`symbols` performs a local syntax lookup without a model call.

## Native investigators

Claude Code and Codex investigations run through the corresponding native CLI.

The investigator gets a separate conversation for the repository work. Related calls can continue that investigation instead of starting over, and independent investigations can run in parallel.

Native investigations are **not** an enforced read-only sandbox. The underlying CLI can expose execution or filesystem capabilities beyond RepoTracer's own repository tools.

For the current security model, read [SECURITY.md](https://github.com/repotracer/repotracer/blob/main/SECURITY.md).

## When the investigator is unsure

A result can preserve what was found, what was checked, and what remains unresolved.

That lets Claude Code or Codex continue from useful partial work instead of repeating the investigation from scratch.

## Custom OpenAI-compatible endpoint

```bash
npx repotracer@latest setup \
  --base-url http://localhost:11434/v1 \
  --model deepseek-coder
```

Set `REPOTRACER_API_KEY` when the endpoint requires authentication.

Custom OpenAI-compatible models use RepoTracer's own repository-tool loop rather than the native Claude Code or Codex environment.

## Requirements

- Node.js 18 or newer for the `npx` launcher
- `codex` for the Codex integration
- `claude` for the Claude Code integration

Published native packages target macOS arm64/x64, Linux arm64/x64, and Windows x64.

## Documentation

- [README](https://github.com/repotracer/repotracer)
- [Benchmarks](https://github.com/repotracer/repotracer/blob/main/BENCHMARKS.md)
- [Architecture](https://github.com/repotracer/repotracer/blob/main/docs/ARCHITECTURE.md)
- [Claude Code and Codex integration](https://github.com/repotracer/repotracer/blob/main/docs/CLAUDE_CODE_INTEGRATION.md)
- [Security](https://github.com/repotracer/repotracer/blob/main/SECURITY.md)

MIT licensed.
