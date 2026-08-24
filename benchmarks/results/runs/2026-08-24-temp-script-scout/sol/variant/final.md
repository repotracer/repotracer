Implemented the scout batch deadline fix.

- Uses one shared batch deadline, retains completed siblings, cancels outstanding futures, and returns ordered timeout failures with consistent duration metadata in [exec.rs](/tmp/repotracer-temp-script-bench.w5jUSa/sol/variant/repository/crates/repo-tools/src/exec.rs:15).
- Ensures canceled Grep searches terminate their `rg` subprocess in [grep.rs](/tmp/repotracer-temp-script-bench.w5jUSa/sol/variant/repository/crates/repo-tools/src/grep.rs:142).
- Added regression coverage for completed, active, queued, and unstarted searches.

Validation passed:

- `cargo test --workspace`
- `cargo clippy -p repotracer-repo-tools --all-targets -- -D warnings`
- `cargo fmt --check`
- `git diff --check`