# Changelog

## 0.1.0 — 2026-08-08

### Added
- Rust scout engine with Read / Glob / Grep tools
- Concurrent tool execution (bounded, ordered results)
- Grep `count` mode mapped to `rg --count-matches`
- Citation parse + path/line validation
- OpenAI-compatible model backend (Ollama default)
- Deterministic mock model for CI
- CLI: `scout`, `serve`, `setup`, `doctor`, `status`, `config`, `uninstall`
- MCP `repo_scout` tool over stdio
- Claude Code, Codex, and Cursor auto-configuration
- Interactive macOS, Linux, and Windows setup with Ollama install, model pull, and live tool-call verification
- Zero-download Codex and Claude subscription backends through the official installed CLIs, with read-only execution, strict structured output, timeouts, and citation revalidation
- Hardware-aware setup defaults plus explicit `--provider ollama|codex|claude|custom` selection
- Grephound routing skills for Claude Code and Codex, Cursor rules, and GitHub Copilot instructions
- MCP `repo_scout` prompt for generic clients
- npm launcher package scaffold
- Benchmark harness scaffold and methodology docs
