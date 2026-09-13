"""Resolve native SWE-bench Lite or local SWE-style tasks without leaking solutions."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import random
import re
import shlex
import urllib.parse
import urllib.request

DATASET = "princeton-nlp/SWE-bench_Lite"


def fetch_json(url):
    request = urllib.request.Request(url, headers={"User-Agent": "repotracer-benchmarks"})
    with urllib.request.urlopen(request, timeout=60) as response:
        # Dataset transport timeout only, never an agent/task deadline.
        data = response.read(32 * 1024 * 1024 + 1)
        if len(data) > 32 * 1024 * 1024:
            raise ValueError("Dataset page exceeds 32 MiB")
        return json.loads(data)


def load_remote(cache: Path):
    """Cache the exact dataset content and revision before selecting tasks."""
    metadata_url = f"https://huggingface.co/api/datasets/{DATASET}"
    revision = fetch_json(metadata_url)["sha"]
    if not re.fullmatch(r"[a-f0-9]{40,64}", revision):
        raise ValueError("Dataset revision is not a commit id")
    path = cache / f"swebench-lite-{revision}.json"
    if path.exists():
        return json.loads(path.read_text(encoding="utf-8")), revision
    rows, offset, total = [], 0, None
    while total is None or offset < total:
        query = urllib.parse.urlencode({"dataset": DATASET, "config": "default", "split": "test",
                                        "offset": offset, "length": 100})
        page = fetch_json("https://datasets-server.huggingface.co/rows?" + query)
        if page.get("partial") or any(row.get("truncated_cells") for row in page["rows"]):
            raise ValueError("Dataset server returned incomplete task content")
        total = page["num_rows_total"]
        # Keep the reference solution out of our local benchmark artifacts.
        chunk = [{key: value for key, value in row["row"].items() if key != "patch"}
                 for row in page["rows"]]
        if not chunk or total > 10000:
            raise ValueError("Unexpected dataset size or missing page")
        rows.extend(chunk)
        offset += len(chunk)
    if fetch_json(metadata_url)["sha"] != revision:
        raise ValueError("Dataset changed during download; retry to capture one revision")
    cache.mkdir(parents=True, exist_ok=True)
    # Concurrent resolutions can reuse a complete file, never a half-written one.
    import tempfile
    with tempfile.NamedTemporaryFile(mode="w", dir=cache, delete=False, encoding="utf-8") as stream:
        json.dump(rows, stream)
        temporary = Path(stream.name)
    temporary.replace(path)
    return rows, revision


def test_names(value):
    result = json.loads(value) if isinstance(value, str) else value
    if not isinstance(result, list) or not all(isinstance(name, str) for name in result):
        raise ValueError("Dataset test names must be a list of strings")
    return result


def resolve_task(task: dict, cache: Path, seed: int) -> dict:
    local = task.get("dataset_path")
    if local:
        path = Path(local).expanduser().resolve()
        raw = path.read_text(encoding="utf-8")
        try:
            rows = json.loads(raw)
        except json.JSONDecodeError:
            rows = [json.loads(line) for line in raw.splitlines() if line.strip()]
        if isinstance(rows, dict):
            rows = rows.get("tasks", [rows])
        revision = hashlib.sha256(raw.encode()).hexdigest()
        source = "local"
    elif task.get("source", "swebench-lite") == "swebench-lite":
        rows, revision = load_remote(Path(cache))
        source = DATASET
    else:
        raise ValueError("Select SWE-bench Lite or provide a local SWE-style JSON/JSONL dataset")
    if not isinstance(rows, list) or not rows:
        raise ValueError("Dataset contains no tasks")
    wanted = task.get("instance_id", "").strip()
    if wanted:
        matches = [row for row in rows if row.get("instance_id") == wanted]
        if len(matches) != 1:
            raise ValueError("Dataset instance is missing or duplicated")
        row = matches[0]
    else:
        # The seed is persisted by the batch; resumption keeps the selected task.
        row = random.Random(seed).choice(rows)
    repo, commit = row["repo"], row["base_commit"]
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo) or ".." in repo:
        raise ValueError("Dataset repository must be a GitHub owner/name")
    if not re.fullmatch(r"[a-fA-F0-9]{40,64}", commit):
        raise ValueError("Dataset base_commit must be a full commit id")
    prompt = row["problem_statement"]
    if not isinstance(prompt, str) or not prompt.strip():
        raise ValueError("Dataset problem statement is empty")
    checks = {"FAIL_TO_PASS": test_names(row.get("FAIL_TO_PASS", [])),
              "PASS_TO_PASS": test_names(row.get("PASS_TO_PASS", []))}
    names = list(dict.fromkeys(checks["FAIL_TO_PASS"] + checks["PASS_TO_PASS"]))
    # Standard pytest node ids need no benchmark-specific solver instructions.
    acceptance_command = ("python3 -m pytest " + " ".join(shlex.quote(name) for name in names)
                          if names and all(".py" in name and not name.startswith("-") for name in names)
                          else "")
    return {
        "id": task["id"], "instance_id": row["instance_id"], "origin": "external",
        "prompt": prompt, "repository": f"https://github.com/{repo}.git", "revision": commit,
        "dataset": source, "dataset_revision": revision, "selection_seed": seed,
        "task_sha256": hashlib.sha256(json.dumps(row, sort_keys=True).encode()).hexdigest(),
        "acceptance_patch": row.get("test_patch", ""),
        "acceptance_tests": checks, "acceptance_command": acceptance_command,
        "apply_allowed": False,
    }
