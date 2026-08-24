<p align="center">
  <img src="assets/logo-lockup-stacked.svg" alt="RepoTracer" width="260">
</p>

<h3 align="center">Get up to 60% more from your Codex limits.</h3>

<p align="center">
  A cheaper model searches your repository and hands Codex verified <code>file:line</code> citations, so Sol stops burning turns on search.
</p>

<p align="center">
  <a href="#install"><img src="assets/button-install.svg" alt="Install RepoTracer" height="38"></a>&nbsp;
  <a href="./BENCHMARKS.md"><img src="assets/button-benchmarks.svg" alt="Benchmarks" height="38"></a>&nbsp;
  <a href="https://repotracer.tech"><img src="assets/button-website.svg" alt="repotracer.tech" height="38"></a>
</p>

<p align="center">
  <img src="assets/demo/scout.gif" alt="Asking RepoTracer a plain-English question about a codebase and getting back four verified file and line-range citations with a summary in 35 seconds" width="100%">
</p>

## Install

```bash
npx repotracer setup
```

`setup` downloads the native binary, verifies its SHA-256 checksum, installs it under `~/.repotracer/bin`, registers the MCP server, and adds a managed routing block to `~/.codex/AGENTS.md`. It reuses your existing Codex login.

No Rust toolchain, second API key, background service, or hand-written config is required.

**Don't want more limits and faster outputs for some reason?** Run the same command again and pick *Uninstall*:

```bash
npx repotracer setup
```

```text
RepoTracer is already configured.
Use arrow keys, then Enter. Esc to cancel.

  > Update the configuration
    Uninstall RepoTracer
```

That removes the MCP entry, the routing block, and the local config. Your Codex login and settings are untouched, and every file it edits is backed up alongside the original first. `repotracer uninstall --yes` does the same thing without the menu.

`setup` runs `doctor` itself at the end, so you see whether it actually works rather than a list of things it wrote.

Preview the changes:

```bash
npx repotracer setup --dry-run
```

Want the `repotracer` command in your own shell too?

```bash
npm install -g repotracer
# or
cargo install --git https://github.com/repotracer/repotracer --locked repotracer
```

## How it works

1. The installed routing instructions tell Codex when an unfamiliar or cross-file task needs repository exploration.
2. Codex calls the MCP tool `repo_scout(query)`.
3. RepoTracer starts an isolated `codex exec` process using GPT-5.6 Luna at medium reasoning.
4. Luna can call only read-only repository tools. It receives bounded Read, Glob, and Grep results.
5. RepoTracer validates every returned path and line range, then returns structured citations, source excerpts, and a handoff.
6. Codex Sol reads the cited code and performs the edit.

```text
Codex Sol
  → MCP repo_scout(query)
    → isolated Luna scout
      → Read / Glob / Grep
    ← validated citations and excerpts
  → edit and verify
```

The scout cannot edit, delete, patch, commit, or push.

## Measured results

Whole task, with Luna's usage counted in.

| Run | Complete cost | Wall time |
|---|---:|---:|
| Real bug fix, production repo | **−62.68%** | −24.54% |
| SWE-bench Astropy 13453 | **−50.12%** | −9.60% |
| Median of three paired runs | **−39.20%** | +31.21% |

Every run is published, including the ones where it lost.

[Methods, caveats, rejected runs, and raw artifacts.](./BENCHMARKS.md)

## When Codex calls it

Use `repo_scout` for cold-start exploration, unfamiliar cross-file behavior, or a failed targeted lookup. Skip it when the prompt already names the file, symbol, or exact change location.

Small edits can cost more if a scout runs unnecessarily. Routing is part of the product, not an optional benchmark trick.

## Current support

| Item | Current value |
|---|---|
| Coding agent | Codex |
| Interface | MCP and CLI |
| Default scout | `gpt-5.6-luna` |
| Scout reasoning | `medium` |
| Scout tools | Read, Glob, Grep |
| Repository writes | Disabled |
| Custom backend | OpenAI-compatible GPT endpoint |

Luna medium remains the default because it passed the bounded reasoning trial. High reasoning missed the production timeout in all three bounded runs and scored 5/6 in the extended run.

## CLI

```bash
repotracer "where is auth handled?"      # scout the current repository
repotracer scout "trace refresh rotation"
repotracer serve                         # MCP over stdio
repotracer setup
repotracer doctor
repotracer status
repotracer config --init
repotracer benchmark
repotracer uninstall --yes
```

`--json` is available on `scout`, `doctor`, and `status`.

### Custom GPT endpoint

```bash
repotracer \
  --base-url https://models.example.com/v1 \
  --model gpt-5.6-mini \
  setup
```

Set `REPOTRACER_API_KEY` when the endpoint requires authentication.

## Security

- Read-only repository access
- Repository-root and symlink-escape checks
- Returned path and line-range validation
- No default telemetry
- Provider credentials remain with the official CLI

See [SECURITY.md](./SECURITY.md).

## Resources

- [repotracer.tech](https://repotracer.tech)
- [Benchmarks](./BENCHMARKS.md)
- [Architecture](./docs/ARCHITECTURE.md)
- [Why complete-task measurement matters](./docs/benchmarks/why-token-counters-lie.md)
- [Microsoft FastContext paper](https://arxiv.org/abs/2606.14066v3)

RepoTracer is an independent project inspired by FastContext. It is not affiliated with or endorsed by Microsoft. See [NOTICE](./NOTICE).

## Develop

```bash
cargo test --workspace
cargo run -p repotracer -- doctor
cargo run -p repotracer -- scout "where is config loaded?" --mock
```

## License

MIT. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).

