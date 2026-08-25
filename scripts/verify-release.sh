#!/usr/bin/env bash
# Verify a build the way a new user and Codex will actually meet it.
#
# Run this before publishing. It installs into a throwaway HOME so your real
# ~/.codex and ~/.repotracer are never touched, then drives the MCP server over
# stdio exactly as Codex does.
#
#   scripts/verify-release.sh              # test the local debug build
#   scripts/verify-release.sh --published  # test what npx actually serves
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
PUBLISHED=0
[[ "${1:-}" == "--published" ]] && PUBLISHED=1

pass() { printf '\033[1;32m  ok\033[0m   %s\n' "$*"; }
fail() { printf '\033[1;31m  FAIL\033[0m %s\n' "$*"; exit 1; }
step() { printf '\n\033[1m%s\033[0m\n' "$*"; }

mkdir -p "$SANDBOX/home/.codex" "$SANDBOX/work"
# Reuse the real Codex credentials read-only; never the real config.
cp ~/.codex/auth.json "$SANDBOX/home/.codex/" 2>/dev/null || fail "no Codex login found; run: codex login"

if [[ "$PUBLISHED" -eq 1 ]]; then
  step "Installing from npm"
  ( cd "$SANDBOX/work" && HOME="$SANDBOX/home" npm_config_cache="$SANDBOX/npm" \
      npx -y repotracer version >/dev/null 2>&1 ) || fail "npx install failed"
  BIN="$(find "$SANDBOX/npm/_npx" -name 'repotracer' -type f -perm -u+x 2>/dev/null | head -1)"
  [[ -n "$BIN" ]] || fail "npx did not vendor a binary"
  pass "npx installed $("$BIN" version)"
else
  step "Building"
  cargo build -q -p repotracer --manifest-path "$REPO/Cargo.toml" || fail "build failed"
  BIN="$REPO/target/debug/repotracer"
  pass "$("$BIN" version)"
fi

step "Setup runs fast and stays out of the working directory"
started=$(date +%s)
out=$(cd "$SANDBOX/work" && HOME="$SANDBOX/home" CODEX_HOME="$SANDBOX/home/.codex" \
  NO_COLOR=1 "$BIN" setup </dev/null 2>&1) || fail "setup failed:\n$out"
elapsed=$(( $(date +%s) - started ))
[[ "$elapsed" -lt 15 ]] || fail "setup took ${elapsed}s; it must not scan the cwd or call a model"
grep -q "Git repository" <<<"$out" && fail "setup inspected the working directory"
pass "finished in ${elapsed}s"

step "Codex registration"
grep -q 'mcp_servers.repotracer' "$SANDBOX/home/.codex/config.toml" || fail "no MCP entry written"
grep -q 'repotracer:start' "$SANDBOX/home/.codex/AGENTS.md" || fail "no routing block written"
pass "MCP entry and routing block written"

step "MCP handshake and live tool call"
BIN="$BIN" REPO="$REPO" python3 - <<'PY' || exit 1
import json, os, subprocess, sys, time
p = subprocess.Popen([os.environ["BIN"], "serve"], cwd=os.environ["REPO"],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, text=True, bufsize=1)
def send(o): p.stdin.write(json.dumps(o) + "\n"); p.stdin.flush()
def die(m): print(f"\033[1;31m  FAIL\033[0m {m}"); p.terminate(); sys.exit(1)

send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
    "protocolVersion":"2024-11-05","capabilities":{},
    "clientInfo":{"name":"verify","version":"1"}}})
info = json.loads(p.stdout.readline()).get("result", {})
if not info.get("serverInfo"): die("initialize returned no serverInfo")
print(f"\033[1;32m  ok\033[0m   initialize: {info['serverInfo']['name']} {info['serverInfo']['version']}")

send({"jsonrpc":"2.0","method":"notifications/initialized"})
send({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
names = [t["name"] for t in json.loads(p.stdout.readline()).get("result", {}).get("tools", [])]
if "repo_scout" not in names: die(f"repo_scout not advertised, got {names}")
print("\033[1;32m  ok\033[0m   tools/list advertises repo_scout")

t0 = time.time()
send({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"repo_scout",
      "arguments":{"query":"where are citations validated before the handoff?"}}})
res = json.loads(p.stdout.readline()).get("result", {})
p.terminate()
if res.get("isError"): die("tools/call returned isError")
cites = (res.get("structuredContent") or {}).get("citations") or []
if not cites: die("no validated citations returned")
print(f"\033[1;32m  ok\033[0m   tools/call returned {len(cites)} citations in {time.time()-t0:.1f}s")

for c in cites:
    path = os.path.join(os.environ["REPO"], c["path"])
    if not os.path.isfile(path): die(f"citation points at a missing file: {c['path']}")
    total = sum(1 for _ in open(path, encoding="utf-8", errors="replace"))
    if c["end_line"] > total: die(f"{c['path']} cites line {c['end_line']} of {total}")
print("\033[1;32m  ok\033[0m   every citation resolves to real lines")
PY

step "Uninstall reverses it"
out=$(cd "$SANDBOX/work" && HOME="$SANDBOX/home" CODEX_HOME="$SANDBOX/home/.codex" \
  NO_COLOR=1 "$BIN" uninstall --yes 2>&1) || fail "uninstall failed:\n$out"
grep -q 'mcp_servers.repotracer' "$SANDBOX/home/.codex/config.toml" && fail "MCP entry survived uninstall"
[[ -f "$SANDBOX/home/.codex/auth.json" ]] || fail "uninstall removed the Codex login"
pass "MCP entry removed, Codex login intact"

printf '\n\033[1;32mAll checks passed.\033[0m Your real ~/.codex was never touched.\n'
