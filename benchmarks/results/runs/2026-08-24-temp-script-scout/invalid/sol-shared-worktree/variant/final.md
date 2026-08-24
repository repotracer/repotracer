Implemented scout deadline handling.

- Deadline-aware batches preserve completed sibling results, cancel unfinished work, and emit failed `ToolResult`s with measured durations in [exec.rs](/Users/mac/orca/workspaces/tool/temp-script-scout/crates/repo-tools/src/exec.rs:31).
- Scout deadlines now propagate through model and repository calls while retaining turn/tool-call stats in [engine.rs](/Users/mac/orca/workspaces/tool/temp-script-scout/crates/core/src/engine.rs:36).
- Canceled `rg` searches terminate their subprocesses via `kill_on_drop` in [grep.rs](/Users/mac/orca/workspaces/tool/temp-script-scout/crates/repo-tools/src/grep.rs:142).
- Added regression tests for sibling retention/cancellation and timeout progress metadata.

`git diff --check` passes. Rust tests could not run because no toolchain is installed; an ephemeral Rust 1.80 install was blocked by sandbox DNS.