# repotracer

RepoTracer is an MCP server whose `repo_scout` tool runs an isolated, read-only Luna process and returns validated source citations to Codex Sol.

## Setup

```bash
npx repotracer setup
```

The installer downloads the native binary, verifies its SHA-256 checksum, copies it to `~/.repotracer/bin/repotracer`, registers the stdio MCP server, and adds a managed routing block to `~/.codex/AGENTS.md`.

Codex must already be installed and signed in. RepoTracer reuses that login and does not require another API key.

```bash
npm install -g @openai/codex
codex login
npx repotracer setup
repotracer doctor
```

Preview without changing files:

```bash
npx repotracer setup --dry-run
```

## Commands

```bash
repotracer "where is auth handled?"
repotracer scout "trace refresh token rotation"
repotracer doctor
repotracer status
repotracer uninstall --yes
```

## Permanent install

```bash
npm install -g repotracer
# or
cargo install --git https://github.com/repotracer/repotracer --locked repotracer
```

Supported platforms: macOS arm64 and x64, Linux arm64 and x64, and Windows x64. Node.js 18 or newer.

Read the [documentation and benchmarks](https://github.com/repotracer/repotracer).

MIT licensed.
