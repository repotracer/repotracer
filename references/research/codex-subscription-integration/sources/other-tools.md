# Other Codex integrations

Research snapshot: 2026-08-26. Every code link below is pinned to the commit
inspected. These projects are useful implementation references, but none is an
official RepoTracer dependency or an OpenAI-supported subscription API.

## Happier

Repository: [happier-dev/happier](https://github.com/happier-dev/happier) at
[`5837a03a513b4725f14419b78ec5d2646c7bd7a1`](https://github.com/happier-dev/happier/commit/5837a03a513b4725f14419b78ec5d2646c7bd7a1), committed 2026-08-26.
GitHub reports MIT. The CLI package at this snapshot is `@happier-dev/cli`
`0.2.10`.

Happier is the strongest reference for a long-lived local integration. Its
Codex app-server client launches `codex app-server --listen stdio://`, performs
JSONL request/notification routing, registers handlers for server-initiated
requests, and keeps a disposable child-process owner:

- [app-server client](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/backends/codex/appServer/client/createCodexAppServerClient.ts#L357-L391)
- [initialize and capability negotiation](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/backends/codex/appServer/client/createCodexAppServerClient.ts#L771-L784)
- [Codex home/environment selection](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/backends/codex/appServer/resolveCodexAppServerProcessEnv.ts#L12-L50)
- [CLI auth probe](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/backends/codex/cli/auth/codexCliAuthSpec.ts#L17-L50)

Auth is delegated to Codex CLI for the ordinary path. Happier also has a
connected-service runtime with an explicit `CODEX_HOME` affinity, allowing
separate Codex homes and account state. That is the right boundary for a
multi-account host: choose the home before spawning Codex, then let Codex read
and refresh its own credentials. Happier's connected-service auth adapter can
apply a selected OAuth identity to the runtime, but RepoTracer should not copy
that private credential-handling path without a product need:

- [connected-service home synchronization](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/backends/codex/connectedServices/syncCodexConnectedServiceHome.ts)
- [runtime auth adapter](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/backends/codex/connectedServices/createCodexConnectedServiceRuntimeAuthAdapter.ts#L150-L218)

Permissions are mapped in one place to approval policy, sandbox, and reviewer
behavior. The app-server permission-profile adapter covers read-only,
workspace, and no-sandbox modes, while `CodexLikePermissionHandler` separately
guards write-like tools and routes approvals to the UI:

- [app-server permission profile](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/backends/codex/appServer/permissionProfile.ts#L15-L62)
- [permission handler](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/agent/permissions/CodexLikePermissionHandler.ts#L1-L22)

The lifecycle work is unusually careful. Disposal snapshots descendants,
terminates the process tree, waits briefly, and escalates to `SIGKILL`. The
client also resolves Windows command invocation and sets `windowsHide: true`:

- [process-tree cleanup](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/agent/runtime/process/killProcessTree.ts#L69-L111)
- [Windows-aware app-server spawn](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/backends/codex/appServer/client/createCodexAppServerClient.ts#L380-L390)
- [managed `codex-acp` release selection](https://github.com/happier-dev/happier/blob/5837a03a513b4725f14419b78ec5d2646c7bd7a1/apps/cli/src/runtime/managedTools/providers/codexAcpRelease.ts#L20-L46)

The managed release code selects a target triple and records a release digest,
but the main app-server path can still use a configured or user-installed CLI.
RepoTracer should make that choice explicit and pin the runtime it supports.

## Codex Relay

Repository: [gronxb/codex-relay](https://github.com/gronxb/codex-relay) at
[`13bc4bea2f165b2a487eff26de0ee1e69b795b31`](https://github.com/gronxb/codex-relay/commit/13bc4bea2f165b2a487eff26de0ee1e69b795b31), committed 2026-08-26.
The published package is `codex-relay` `1.4.12`, Apache-2.0, and requires Node
`>=22.14.0`. It pins both `@openai/codex` and `@openai/codex-sdk` to `0.149.1`:

- [package metadata and pins](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/package.json#L1-L74)
- [CLI requirements and local login assumption](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/README.md#L1-L12)

Relay uses two official-client paths. Its main mobile session client speaks to
a manually implemented app-server client. Some short-running operations use
the official TypeScript SDK, which itself launches Codex. The app-server path
supports private stdio and an optional shared server. On macOS it prefers the
Codex shared Unix socket, attaches when one already exists, and reconnects a
broken WebSocket without killing a server it does not own. Native Windows uses
a loopback WebSocket for shared mode and `windowsHide` for child processes:

- [official SDK wrapper](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/src/codex.ts#L1-L25)
- [stdio/shared app-server lifecycle](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/src/app-server.ts#L659-L695)
- [shared-server attach/reconnect](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/src/app-server.ts#L697-L881)
- [platform command resolution](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/src/codex-binary.ts#L31-L111)
- [shared-mode platform behavior](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/README.md#L31-L63)

The relay inherits `CODEX_HOME` and expects the user to have installed and
logged into Codex CLI. It adds its own remote pairing layer: approval codes,
hashed client tokens, and an encrypted session using an ephemeral key exchange
and AES-GCM. The default HTTP listener is `0.0.0.0:8787`, so the pairing gate
and transport encryption matter. `--dangerously-auto-approve` exists and is
explicitly documented for controlled demos only:

- [pairing/session storage](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/src/pairing-store.ts#L46-L121)
- [encrypted transport](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/src/secure-transport.ts#L1-L106)
- [listener and approval settings](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/README.md#L125-L150)

Relay retains direct child handles and has a PID-file stop path for its
background relay. Its shared-server ownership distinction is the useful part
to copy. A consumer must never kill an app-server it merely attached to:

- [background process ownership](https://github.com/gronxb/codex-relay/blob/13bc4bea2f165b2a487eff26de0ee1e69b795b31/packages/codex-relay/src/background-process.ts#L28-L115)

## Codexia

Repository: [milisp/codexia](https://github.com/milisp/codexia) at
[`ec21cc1b1321941c9e21db6f0dc523f6d71bb8fb`](https://github.com/milisp/codexia/commit/ec21cc1b1321941c9e21db6f0dc523f6d71bb8fb), committed 2026-08-20.
The Tauri/Rust application is version `0.48.1`, MIT, and ships macOS, Linux,
and Windows builds. Its Codex crate uses Tokio and discovers a local Codex
binary, then launches `codex app-server` with piped stdin/stdout/stderr. The
JSON-RPC handshake and notification forwarding are a useful Rust example:

- [Rust app-server connector](https://github.com/milisp/codexia/blob/ec21cc1b1321941c9e21db6f0dc523f6d71bb8fb/crates/codex/src/app_server.rs#L103-L206)
- [Tauri Codex commands](https://github.com/milisp/codexia/tree/ec21cc1b1321941c9e21db6f0dc523f6d71bb8fb/src-tauri/src/commands/codex)
- [Windows build workflow](https://github.com/milisp/codexia/blob/ec21cc1b1321941c9e21db6f0dc523f6d71bb8fb/.github/workflows/ci-windows.yml)

Codexia provides UI approval commands for command execution, file changes, and
permissions. It also saves named account snapshots by reading and copying
`CODEX_HOME/auth.json`, then switches accounts by sending
`account/login/start` with the saved access token:

- [account snapshot and switch implementation](https://github.com/milisp/codexia/blob/ec21cc1b1321941c9e21db6f0dc523f6d71bb8fb/crates/codex/src/accounts.rs#L7-L169)
- [approval commands](https://github.com/milisp/codexia/blob/ec21cc1b1321941c9e21db6f0dc523f6d71bb8fb/src-tauri/src/commands/codex/approval.rs#L1-L77)

There is one important lifecycle defect to avoid. `connect_codex` takes the
child's stdio handles and moves those handles into reader tasks, but it does
not retain the `tokio::process::Child` in `CodexAppServer`, wait on it, or kill
it during shutdown. Dropping the local child handle does not provide a reliable
process-tree cleanup contract. Codexia is therefore a good Rust protocol and
Windows hidden-process reference, but not a lifecycle template.

## OpenCode OpenAI Codex Auth

Repository: [numman-ali/opencode-openai-codex-auth](https://github.com/numman-ali/opencode-openai-codex-auth) at
[`bec2ad69b252ef4ad7dd33b9532ff8b4fdb6d016`](https://github.com/numman-ali/opencode-openai-codex-auth/commit/bec2ad69b252ef4ad7dd33b9532ff8b4fdb6d016), released 2026-01-09 as version
`4.4.0`. The package declares MIT with a personal-use/OpenAI-terms disclaimer.

The only part worth borrowing is the narrow OAuth mechanics: PKCE, a random
state value, the official `auth.openai.com` authorization/token endpoints, and
a loopback callback on `127.0.0.1:1455`:

- [PKCE OAuth flow](https://github.com/numman-ali/opencode-openai-codex-auth/blob/bec2ad69b252ef4ad7dd33b9532ff8b4fdb6d016/lib/auth/auth.ts#L1-L194)
- [loopback callback server](https://github.com/numman-ali/opencode-openai-codex-auth/blob/bec2ad69b252ef4ad7dd33b9532ff8b4fdb6d016/lib/auth/server.ts#L12-L75)

Do not use this plugin's request path as a RepoTracer integration. It replaces
OpenAI API URLs with `https://chatgpt.com/backend-api`, adds ChatGPT-specific
headers, normalizes request bodies, and uses a dummy API key to make the
OpenCode provider accept OAuth credentials:

- [private backend URL and dummy key](https://github.com/numman-ali/opencode-openai-codex-auth/blob/bec2ad69b252ef4ad7dd33b9532ff8b4fdb6d016/lib/constants.ts#L9-L16)
- [URL/header rewriting](https://github.com/numman-ali/opencode-openai-codex-auth/blob/bec2ad69b252ef4ad7dd33b9532ff8b4fdb6d016/lib/request/fetch-helpers.ts#L82-L195)
- [request transformation](https://github.com/numman-ali/opencode-openai-codex-auth/blob/bec2ad69b252ef4ad7dd33b9532ff8b4fdb6d016/lib/request/request-transformer.ts#L180-L187)

That backend is private and version-sensitive. Reproducing it would create a
compatibility, account-security, and terms-of-service burden. Use the official
Codex runtime and app-server instead.

## RepoTracer recommendations

1. Integrate with the official Codex app-server using newline-delimited JSON-RPC over stdio: launch `codex app-server --listen stdio://`, perform `initialize`/`initialized`, correlate responses, route notifications, and answer approval requests.
2. Retain the child handle and an ownership record. On shutdown, close the owned process tree, wait, and escalate. If attaching to a shared socket, record `attached` versus `owned` and never terminate an attached server.
3. Reuse the user's subscription through delegated CLI auth and the selected `CODEX_HOME`. If RepoTracer needs isolated accounts, assign one isolated home per account. Do not parse or copy `auth.json` unless the product explicitly accepts that credential risk.
4. Keep permission policy centralized. Map RepoTracer modes to Codex approval policy and sandbox policy, and preserve server-initiated approval requests for the user.
5. Resolve Windows commands deliberately. Handle `.cmd`/`.bat` invocation, use hidden child-process creation, and test Windows-specific sandbox/readiness behavior.
6. Pin and verify the Codex runtime. Record the supported CLI/app-server version and, if downloading a binary, verify its release asset digest. Do not use an unbounded `latest` dependency in the runtime path.
7. Treat a shared Unix socket or loopback WebSocket as an optional optimization. Add reconnect logic and explicit process ownership before enabling it.
8. Do not emulate the private ChatGPT backend. PKCE and a loopback callback are the only reusable ideas from the OpenCode plugin; the supported integration boundary is the official Codex CLI/app-server.
