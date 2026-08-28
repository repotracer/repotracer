#!/usr/bin/env python3
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import time
import sys

from run_scout_tools import PRIVATE, ROOT, tracked_snapshot

OUT = PRIVATE / "operations"
QUERY = "Trace an MCP repo_scout request through dispatch, Scout execution, and response validation."


def directory_bytes(path: Path) -> int:
    return sum(item.stat().st_size for item in path.rglob("*") if item.is_file())


def run(name: str, command: list[str], cwd: Path) -> dict:
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.monotonic()
    process = subprocess.run(
        command,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
    )
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    (OUT / f"{name}.stdout").write_text(process.stdout)
    (OUT / f"{name}.stderr").write_text(process.stderr)
    return {
        "name": name,
        "argv": command,
        "exit_code": process.returncode,
        "wall_seconds": round(time.monotonic() - started, 3),
        "user_cpu_seconds": round(after.ru_utime - before.ru_utime, 3),
        "system_cpu_seconds": round(after.ru_stime - before.ru_stime, 3),
        "max_rss_bytes": after.ru_maxrss if sys.platform == "darwin" else after.ru_maxrss * 1024,
        "stdout_bytes": len(process.stdout.encode()),
        "stderr_bytes": len(process.stderr.encode()),
    }


def main() -> None:
    codegraph = shutil.which("codegraph")
    if not codegraph:
        raise SystemExit("codegraph not found")
    if OUT.exists():
        shutil.rmtree(OUT)
    repo = OUT / "repo"
    tracked_snapshot(ROOT, repo)
    operations = []
    operations.append(run("cold-init", [codegraph, "init", str(repo)], repo))
    operations.append(
        run("first-explore", [codegraph, "explore", "--path", str(repo), "--", QUERY], repo)
    )
    operations.append(
        run("warm-explore", [codegraph, "explore", "--path", str(repo), "--", QUERY], repo)
    )
    operations.append(run("unchanged-sync", [codegraph, "sync", "-q", str(repo)], repo))
    changed = repo / "crates/core/src/types.rs"
    changed.write_text(changed.read_text() + "\n// benchmark-only state change\n")
    operations.append(run("changed-sync", [codegraph, "sync", "-q", str(repo)], repo))
    operations.append(
        run("post-sync-explore", [codegraph, "explore", "--path", str(repo), "--", QUERY], repo)
    )
    for operation in operations:
        if operation["exit_code"]:
            raise RuntimeError(f"CodeGraph operation failed: {operation}")
    result = {
        "codegraph_version": subprocess.run(
            [codegraph, "--version"], capture_output=True, text=True
        ).stdout.strip(),
        "repository_files": sum(1 for path in repo.rglob("*") if path.is_file()),
        "index_bytes": directory_bytes(repo / ".codegraph"),
        "operations": operations,
        "secret_material": "not used or retained",
    }
    (OUT / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
