# repotracer

RepoTracer is an MCP server whose `repo_scout` tool runs an isolated, read-only Luna process and returns validated source citations to Codex Sol.

## Setup

```bash
npx repotracer@latest setup
```

The installer downloads the native binary, verifies its SHA-256 checksum, copies it to `~/.repotracer/bin/repotracer`, registers the stdio MCP server, and adds a managed routing block to `~/.codex/AGENTS.md`.

Codex must already be installed and signed in. RepoTracer reuses that login and does not require another API key.

```bash
npm install -g @openai/codex
codex login
npx repotracer@latest setup
repotracer doctor
```

Preview without changing files:

```bash
npx repotracer@latest setup --dry-run
```

RepoTracer updates automatically. Restart Codex after an update for it to take
effect. To disable automatic updates, set `updates.automatic = false` in
`~/.repotracer/config.toml` or set `REPOTRACER_NO_UPDATE=1`. Running
`npx repotracer@latest setup` still updates a disabled installation.

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
