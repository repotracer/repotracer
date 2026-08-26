# Codex subscription integration

Decision: use the official Codex app-server over local stdio JSON-RPC. Do not
adopt the TypeScript SDK for the Rust MCP server. The TypeScript SDK launches
`codex exec`; the Python SDK speaks app-server but adds a Python runtime. T3
Code, Happier, Codex Relay, and Codexia all use app-server patterns.

Reuse: app-server handshake, streaming events, approval routing, `CODEX_HOME`
delegation, and process ownership patterns from those projects. Keep custom:
RepoTracer's scout prompt, citation validation, and MCP result shape.

Risk: app-server is still a Codex runtime and does not guarantee immunity from
Windows sandbox regressions. Pin and smoke-test the supported Codex version.

Sources: [official options](sources/official-options.md), [T3 Code](sources/t3-code.md), [other tools](sources/other-tools.md).
