<p align="center">
  <img src="assets/logo-lockup-stacked.svg" alt="RepoTracer" width="260">
</p>

<h3 align="center">Get up to 60% more from your Codex limits.</h3>

<p align="center">
  RepoTracer is an MCP server whose <code>repo_scout</code> tool runs an isolated, read-only Luna process and returns validated source citations to Codex Sol.
</p>

<p align="center">
  <a href="#install"><img src="assets/button-install.svg" alt="Install RepoTracer" height="38"></a>&nbsp;
  <a href="./BENCHMARKS.md"><img src="assets/button-benchmarks.svg" alt="Benchmarks" height="38"></a>
</p>

<p align="center">
  <img src="assets/demo/scout.gif" alt="Asking RepoTracer a plain-English question about a codebase and getting back four verified file and line-range citations with a summary in 35 seconds" width="100%">
</p>

RepoTracer is a beta. It reduced complete provider cost in several measured runs, but one recent implementation task also exposed a quality regression. The raw results are public in [BENCHMARKS.md](./BENCHMARKS.md).

## Install

```bash
npx repotracer setup
```

`setup` downloads the native binary, verifies its SHA-256 checksum, installs it under `~/.repotracer/bin`, registers the MCP server, and adds the Codex routing skill. It reuses your existing Codex login.

No Rust toolchain, second API key, background service, or hand-written config is required.

If Codex is not installed:

```bash
npm install -g @openai/codex
codex login
npx repotracer setup
repotracer doctor
```

Preview the changes:

```bash
npx repotracer setup --dry-run
```

Permanent installs are also available:

```bash
npm install -g repotracer
# or
cargo install --git https://github.com/repotracer/repotracer --locked repotracer
```

## How it works

1. The installed routing skill tells Codex when an unfamiliar or cross-file task needs repository exploration.
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

Complete cost includes both Sol and Luna.

| Run | Scope | Quality | Complete cost | Wall time |
|---|---|---:|---:|---:|
| Google signup | One real implementation task | 78.75 vs 83.13 — slightly lower | **−62.68%** | −24.54% |
| SWE-bench Astropy 13453 | One coding task | Exact regression passed in both arms | **−50.12%** | −9.60% |
| Repeated natural routing | Three randomized pairs of one cross-file question | 6/6 checks in every arm | **−39.20% median** | +31.21% median |
| Current immediate routing | Three randomized pairs of one cross-file question | 6/6 checks in every arm | **−28.63% median** | +6.65% median |

On the Google task the expensive model also took in 74.86% fewer tokens. A fixed provider budget would cover roughly 2.7x as many equivalent runs of it. That is one task, and it does not prove every Codex limit lasts proportionally longer.

The Google result matters: RepoTracer found the right production files, but Codex wrote its regression check against a duplicated test fixture. The run was much cheaper and slightly worse. We are investigating that failure before claiming the same quality as direct Codex.

[Read the methods, caveats, rejected runs, and raw artifacts.](./BENCHMARKS.md)

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
