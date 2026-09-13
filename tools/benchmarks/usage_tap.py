#!/usr/bin/env python3
"""Transparent RepoTracer MCP tap with append-only scout usage records."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
import uuid


def _write(stream, value: dict) -> None:
    stream.write(json.dumps(value, sort_keys=True, allow_nan=False) + "\n")
    stream.flush()


def main() -> None:
    if len(sys.argv) < 2:
        raise SystemExit("usage: usage_tap.py COMMAND [ARG ...]")
    log_path = os.environ.get("REPOTRACER_USAGE_LOG")
    if not log_path:
        raise SystemExit("REPOTRACER_USAGE_LOG is required")
    path = Path(log_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    child = subprocess.Popen(
        sys.argv[1:], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=sys.stderr, text=True, bufsize=1,
    )
    pending: dict[str, dict] = {}
    invocation = uuid.uuid4().hex
    sequence = 0
    lock = threading.Lock()
    log = path.open("a", encoding="utf-8", buffering=1)

    def receive() -> None:
        assert child.stdout is not None
        for line in child.stdout:
            try:
                message = json.loads(line)
                rpc_id = str(message.get("id"))
                with lock:
                    request = pending.pop(rpc_id, None)
                    if request is not None:
                        result = message.get("result") or {}
                        structured = result.get("structuredContent") or {}
                        _write(log, {
                            "event": "response", "id": request["id"],
                            "rpc_id": rpc_id,
                            "recorded_unix": time.time(),
                            "request": request,
                            "stats": structured.get("stats"),
                            "failed": bool(message.get("error") or result.get("isError")),
                            "error": message.get("error"),
                        })
            except (AttributeError, TypeError, ValueError):
                pass
            sys.stdout.write(line)
            sys.stdout.flush()

    reader = threading.Thread(target=receive, daemon=True)
    reader.start()
    try:
        assert child.stdin is not None
        for line in sys.stdin:
            try:
                message = json.loads(line)
                params = message.get("params") or {}
                if message.get("method") == "tools/call" and params.get("name") == "repo_scout":
                    rpc_id = str(message.get("id"))
                    sequence += 1
                    identity = f"{invocation}:{sequence}:{rpc_id}"
                    request = {
                        "id": identity,
                        "arguments": params.get("arguments") or {},
                        "recorded_unix": time.time(),
                    }
                    with lock:
                        pending[rpc_id] = request
                        _write(log, {"event": "request", "rpc_id": rpc_id, **request})
            except (AttributeError, TypeError, ValueError):
                pass
            child.stdin.write(line)
            child.stdin.flush()
    finally:
        if child.stdin is not None:
            try:
                child.stdin.close()
            except BrokenPipeError:
                pass
        child.wait()
        reader.join()
        with lock:
            for rpc_id, request in pending.items():
                _write(log, {"event": "missing_response", "id": request["id"], "rpc_id": rpc_id,
                             "request": request, "recorded_unix": time.time()})
        log.close()


if __name__ == "__main__":
    main()
