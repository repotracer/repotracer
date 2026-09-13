# Security

## Reporting a vulnerability

Please report security issues through GitHub's private security advisory feature.

Do not open a public issue for an unpatched vulnerability.

## Expected behavior

RepoTracer can start investigators through the Claude Code or Codex CLI you already use.

Native investigators run inside that CLI environment. Depending on its permissions and configuration, they may run commands or tests, create temporary files, and inspect related repositories or paths.

RepoTracer does **not** turn a native Claude Code or Codex session into a read-only sandbox, and the starting repository is not an operating-system filesystem boundary.

Those behaviors are expected. If you are investigating an untrusted repository or need stricter isolation, use the sandbox, container, VM, or permission controls of the underlying CLI and operating system.

OpenAI-compatible investigators are different: they use RepoTracer's own repository-tool loop rather than inheriting the native Claude Code or Codex environment.

## Source validation

When RepoTracer attaches a structured file and line range, it can check that the location resolves.

That verifies the source location, not the investigator's conclusion. Paths mentioned only in free-form model text are not necessarily validated source attachments.

## Credentials and telemetry

Native investigations use the provider CLI's existing authentication. Custom endpoints may use `REPOTRACER_API_KEY`.

RepoTracer does not enable product telemetry by default. Provider CLIs and custom endpoints have their own logging, retention, and data policies.

For the runtime model, see [Architecture](./docs/ARCHITECTURE.md).
