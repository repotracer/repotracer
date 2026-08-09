# Security

## Reporting

Open a private security advisory on GitHub, or email the maintainers listed on the repo.

## Design guarantees (v1)

- Scout tools are **read-only**
- Paths are constrained to the repository root
- Symlink escapes are rejected
- No default telemetry
- MCP logs go to stderr only

## Out of scope

- Prompt injection against the frontier agent after citations are returned
- Malicious repositories that try to exhaust local CPU via huge greps (mitigated by timeouts/caps)
