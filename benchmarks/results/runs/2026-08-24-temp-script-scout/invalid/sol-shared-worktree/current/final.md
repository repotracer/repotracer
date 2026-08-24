Implemented the scout deadline handling:

- Drains already-ready sibling results before canceling remaining futures.
- Cancels pending work and ensures `rg` subprocesses use kill-on-drop.
- Produces consistent failure and elapsed-duration metadata.
- Carries one absolute deadline through model and repository-tool calls.
- Added regression coverage for results completing in the same scheduler turn as the deadline.

Key changes: [exec.rs](/Users/mac/orca/workspaces/tool/temp-script-scout/crates/repo-tools/src/exec.rs:120), [grep.rs](/Users/mac/orca/workspaces/tool/temp-script-scout/crates/repo-tools/src/grep.rs:148), and [engine.rs](/Users/mac/orca/workspaces/tool/temp-script-scout/crates/core/src/engine.rs:103).

`git diff --check` passes. Rust tests could not run because the environment has no installed rustup toolchain.