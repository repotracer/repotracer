# Official OpenAI Codex integration options

Research snapshot: 2026-08-26. Sources are official OpenAI documentation, the
official `openai/codex` repository, and the npm/PyPI registries. The source
repository was at `039eb58a0ba6647fb8f29fdd35341f3f1b153728` (2026-08-26
09:17:25Z); its latest GitHub release was `rust-v0.149.1` / `0.149.1`,
published 2026-08-24.

## Bottom line for a Rust MCP server

Use the Codex app-server over local stdio JSON-RPC, launched as a child
`codex app-server --listen stdio://` process. This is the smallest new
integration for an existing Rust MCP server: Rust already has the required
process and JSONL plumbing, and the app-server protocol exposes threads,
streaming events, structured `outputSchema`, sandbox/approval controls, and
thread continuation. Run the child under the same user and `CODEX_HOME` as the
user's Codex installation so Codex itself reuses the ChatGPT login cache; do
not copy or parse `auth.json` in the MCP server.

The app-server command is still marked experimental, and some methods/fields
require `initialize.params.capabilities.experimentalApi=true`. Keep the
initial implementation to the documented stable subset (`initialize`,
`thread/start`/`thread/resume`, `turn/start`, notifications, and
`turn/interrupt`) and pin the executable version. Generate and test schemas
from that exact executable (`codex app-server generate-json-schema` or
`generate-ts`). Handle server-initiated approval requests rather than
auto-accepting them.

The Python SDK is a convenient reference implementation, but embedding Python
in a Rust MCP server adds a runtime and packaging boundary. The TypeScript SDK
is less suitable for this use case: it is explicitly a wrapper around
`codex exec`, so it still adds a Node sidecar and an exec process rather than
removing the process boundary.

## Version and packaging snapshot

| Component | Observed version/status | Runtime and packaging evidence |
| --- | --- | --- |
| Codex repository | HEAD `039eb58a0ba6647fb8f29fdd35341f3f1b153728` (2026-08-26); latest release `rust-v0.149.1` (2026-08-24) | Rust implementation; repository Apache-2.0. |
| TypeScript SDK, `@openai/codex-sdk` | npm `0.149.1`, published 2026-08-24 | Node `>=18`; exact dependency on `@openai/codex@0.149.1`; Apache-2.0. |
| Codex CLI npm package, `@openai/codex` | npm `0.149.1`, published 2026-08-24 | Node launcher plus platform native package; CLI metadata says Node `>=16`; Apache-2.0. |
| Python SDK, `openai-codex` | PyPI `0.147.0`, latest upload 2026-08-18 | Python `>=3.10`; source repository and package build metadata say Apache-2.0. The published PyPI metadata leaves `License` blank. |
| Python SDK's pinned runtime | Source `sdk/python/pyproject.toml` pins `openai-codex-cli-bin==0.147.0` | The runtime package contains the native CLI and is installed by the SDK; PyPI also showed a separately published `0.149.0` runtime (2026-08-21). Do not mix SDK/runtime versions. |

The repository changes faster than the published SDKs. Treat registry versions
above as the reproducible published snapshot, and the commit as the source
snapshot. Pin both the app-server/CLI executable and any SDK rather than using
`latest`.

## TypeScript SDK

### What it launches

`@openai/codex-sdk` exports `Codex`, `Thread`, and typed event/item interfaces.
Its source `src/exec.ts` calls Node `child_process.spawn` with:

```text
codex exec --experimental-json ...
```

It sends the prompt on stdin and reads JSONL events from stdout. The SDK is
therefore not an in-process Codex implementation and does not speak
app-server directly. The dependency `@openai/codex` includes the native Rust
CLI in platform packages; its `bin/codex.js` selects and spawns the native
binary.

### API/output surface

`Codex.startThread()` and `resumeThread(id)` return a thread. `thread.run()`
returns `{ items, finalResponse, usage }`; `runStreamed()` yields structured
events including `thread.started`, item lifecycle events, and `turn.completed`.
Per-turn JSON Schema is supported through `outputSchema`, implemented by a
temporary schema file passed as `--output-schema`. Thread options include
model, working directory, additional directories, reasoning effort, web
search, approval policy, and sandbox mode.

### Auth and subscription reuse

The SDK inherits the Node process environment by default and passes an
explicit `apiKey` as `CODEX_API_KEY`. It has no public ChatGPT browser/device
login method. For a ChatGPT subscription, the user must already have logged in
with Codex CLI (or another Codex local surface), and the spawned CLI must run
as the same OS user with the same `CODEX_HOME`/credential-store context. The
official auth documentation says local Codex surfaces share cached login
details in `~/.codex/auth.json` or the OS credential store. An API key instead
uses usage-based Platform billing, not the ChatGPT plan.

### Windows and pinning

The SDK and npm launcher map Windows x64 to
`x86_64-pc-windows-msvc` and Windows ARM64 to
`aarch64-pc-windows-msvc`, then invoke `codex.exe`. The npm package's optional
platform dependencies are version-matched to the top-level package. Node 18+
is the SDK requirement (the underlying CLI metadata says Node 16+). Pin the
SDK and its exact CLI dependency together.

## Python SDK

### What it launches

The Python package is a typed JSON-RPC client, but it still launches a Codex
process. `CodexClient.start()` uses `subprocess.Popen` with:

```text
<bundled-or-configured-codex> app-server --listen stdio://
```

Published SDK builds depend on the exact `openai-codex-cli-bin` runtime. The
runtime package contains the platform-native `codex`/`codex.exe`; callers can
intentionally override it with `CodexConfig(codex_bin=...)`, or replace the
launch command with `launch_args_override`. Normal use therefore has no
separate Codex installation requirement, but it does download/package a
native Codex runtime.

### API/output surface

`Codex` and `AsyncCodex` provide `thread_start`, `thread_resume`,
`thread_fork`, listing/archive operations, model listing, and ChatGPT/API-key
login helpers. A thread's `run()` returns a typed `TurnResult` containing turn
status/error/timestamps, `final_response`, completed typed items, and token
usage. `turn()` exposes a handle for streaming, steering, and interrupting.
`output_schema` is sent as the app-server `turn/start.outputSchema` JSON
Schema. The generated Pydantic models and notification registry are derived
from the app-server v2 protocol.

### Auth and subscription reuse

The SDK README explicitly says it reuses existing Codex authentication. It
also exposes `login_chatgpt()` (browser URL plus wait handle),
`login_chatgpt_device_code()`, `login_api_key()`, `account()`, and `logout()`.
The child process inherits the parent environment and Codex's normal
`CODEX_HOME`/credential-store resolution. ChatGPT login uses the user's
subscription entitlements; API-key login is billed by the OpenAI Platform.

### Permission controls and caveat

Public `Sandbox` presets are `read_only`, `workspace_write`, and `full_access`.
Thread and turn calls also accept approval mode, cwd, model, effort, and
output schema. The lower-level `CodexClient` accepts an approval handler for
server-initiated requests. Its built-in default handler accepts command and
file-change approvals, so a security-sensitive embedding should provide an
explicit handler or use restrictive approval/sandbox settings.

### Windows and pinning

The runtime publisher provides wheels for macOS x64/ARM64, Linux x64/ARM64
(glibc and musl), and Windows AMD64/ARM64. The Python launcher uses
`codex.exe` on Windows and contains Windows-specific `PATH` handling. Python
requires 3.10+. The daemon/lifecycle helper is Unix-only, but this limitation
does not apply to launching the normal app-server process directly on Windows;
Windows sandbox setup is represented in the app-server protocol.

## Direct app-server protocol

### Transport and stable surface

Official docs describe app-server as the interface used by rich Codex clients
and recommend it for deep product integration. The default transport is stdio
newline-delimited JSON (JSON-RPC 2.0 with the `jsonrpc` header omitted on the
wire). Unix sockets are available; WebSocket is explicitly experimental and
unsupported for production. A client must send `initialize`, then the
`initialized` notification, before any other request.

The core flow is:

1. Spawn `codex app-server --listen stdio://` and keep stdin/stdout open.
2. Send `initialize` with `clientInfo`, then `initialized`.
3. Call `thread/start` or `thread/resume`.
4. Call `turn/start`; consume `item/*`, agent-message deltas, and
   `turn/completed` notifications while routing response IDs.
5. Reply to server-initiated approval requests and interrupt when required.

`thread/start` accepts model, cwd, approval policy, legacy sandbox, and other
settings. `turn/start` accepts `sandboxPolicy`, model/effort, personality,
cwd, and per-turn `outputSchema`. The protocol supports explicit read roots,
writable roots, network access, and `externalSandbox`. Beta named permission
profiles are available through `permissionProfile/list` and require the
experimental capability. `command/exec` exists for sandboxed commands, but
`thread/shellCommand` and `process/*` are outside the sandbox; expose those
only for deliberate user-authorized operations.

The response/event schema is strong but version-specific: the CLI can emit
TypeScript or JSON Schema artifacts matching the executable. The docs call out
an experimental API capability; omit it to stay on the documented stable
subset, or opt in only after testing the exact pinned version. The CLI
subcommand itself is currently labeled experimental, so app-server should be
treated as a pinned, compatibility-tested integration rather than a forever
stable wire contract.

### Auth, Windows, and Rust fit

App-server has no separate subscription credential protocol in the client
handshake. The spawned Codex runtime loads the normal local Codex credentials,
so same-user/same-`CODEX_HOME` execution reuses the user's ChatGPT login. The
official auth docs say `codex login` supports ChatGPT subscription access and
that local surfaces share the cached login. Do not expose a remote unauthenticated
WebSocket: docs provide capability-token and signed-bearer-token modes, but
local stdio avoids that network boundary.

Direct Rust integration needs only a child process, line-delimited JSON-RPC
reader/writer, request correlation, notification routing, and approval
handlers. It avoids embedding Node or Python and can map the app-server's
generated JSON schema into the Rust MCP server's existing JSON values. This is
the least-new-complexity official path for a Rust MCP server, subject to
shipping/documenting the required Codex CLI installation and version.

## Deprecated Codex MCP server

`codex mcp-server` exposes `codex` and `codex-reply` as MCP tools with
`structuredContent.threadId` and text content. The official page now says this
command is deprecated and directs new integrations to app-server. Its example
also uses an `OPENAI_API_KEY`, so it is not the preferred answer when the
hard requirement is reusing a user's ChatGPT subscription. Keep it only for
backward compatibility with an existing MCP client.

## Primary sources

- [Codex SDK docs](https://developers.openai.com/codex/sdk.md) — official TS/Python positioning, requirements, sandbox presets, and SDK links.
- [TypeScript SDK source README](https://github.com/openai/codex/blob/main/sdk/typescript/README.md) — wrapper behavior, streaming, structured output, environment/config options.
- [TypeScript process launcher](https://github.com/openai/codex/blob/main/sdk/typescript/src/exec.ts) — `spawn`, `codex exec --experimental-json`, platform selection, API-key environment.
- [Python SDK source README](https://github.com/openai/codex/blob/main/sdk/python/README.md) — auth reuse and login methods.
- [Python client launcher](https://github.com/openai/codex/blob/main/sdk/python/src/openai_codex/client.py) — `Popen`, `app-server --listen stdio://`, config, inherited environment.
- [Python sandbox API](https://github.com/openai/codex/blob/main/sdk/python/src/openai_codex/_sandbox.py) — read-only/workspace-write/full-access mappings.
- [Python SDK pyproject](https://github.com/openai/codex/blob/main/sdk/python/pyproject.toml) — Python requirement and exact runtime dependency pin.
- [Python runtime package](https://github.com/openai/codex/tree/main/sdk/python-runtime) — bundled native runtime layout and Apache-2.0 metadata.
- [App-server docs](https://developers.openai.com/codex/app-server.md) — transports, handshake, lifecycle, schemas, approvals, sandbox/permissions, Windows setup, and experimental API rules.
- [Authentication docs](https://developers.openai.com/codex/auth.md) — ChatGPT subscription vs API-key auth, credential caching, `CODEX_HOME`, and token guidance.
- [CLI README](https://github.com/openai/codex/blob/main/README.md) — Windows install, ChatGPT plans, API-key alternative, and license.
- [Deprecated MCP-server docs](https://developers.openai.com/codex/mcp-server.md) — legacy tool schemas and deprecation notice.
- [GitHub repository metadata](https://api.github.com/repos/openai/codex), [HEAD commit](https://github.com/openai/codex/commit/039eb58a0ba6647fb8f29fdd35341f3f1b153728), and [latest release](https://github.com/openai/codex/releases/tag/rust-v0.149.1).
- [npm `@openai/codex-sdk` metadata](https://registry.npmjs.org/@openai/codex-sdk), [npm `@openai/codex` metadata](https://registry.npmjs.org/@openai/codex).
- [PyPI `openai-codex` metadata](https://pypi.org/pypi/openai-codex/json), [PyPI runtime metadata](https://pypi.org/pypi/openai-codex-cli-bin/json).
