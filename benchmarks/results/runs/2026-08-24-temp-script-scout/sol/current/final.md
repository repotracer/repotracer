Implemented deadline-safe repository search batching.

- [exec.rs](/tmp/repotracer-temp-script-bench.w5jUSa/sol/current/repository/crates/repo-tools/src/exec.rs:15): enforces one shared batch deadline, retains ready sibling results, cancels outstanding futures, and emits ordered timeout failures with consistent duration metadata.
- [grep.rs](/tmp/repotracer-temp-script-bench.w5jUSa/sol/current/repository/crates/repo-tools/src/grep.rs:142): enables `kill_on_drop` so cancelled ripgrep processes terminate.
- Added regression tests for partial-result retention, future cancellation, metadata, and subprocess termination.

Validation passed: 55 workspace tests, Clippy with warnings denied, and formatting checks. One unrelated authenticated live test remains ignored.