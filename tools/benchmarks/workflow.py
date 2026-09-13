#!/usr/bin/env python3
"""Durable local workflow for native RepoTracer benchmark comparisons."""
from __future__ import annotations

import argparse
import contextlib
import copy
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import re
import shutil
import socket
import subprocess
import sys
import time
import uuid

if os.name == "nt":
    import msvcrt
else:
    import fcntl

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
import bench  # noqa: E402
import native  # noqa: E402

TERMINAL = {"completed", "failed", "interrupted"}
ACTIVE = {"queued", "preparing", "running", "grading"}
TEST_COMMAND = re.compile(
    r"(^|[;&|]\s*|\s)(cargo\s+test|pytest|python(?:3)?\s+-m\s+(?:pytest|unittest)|"
    r"npm\s+(?:test|run\s+test)|pnpm\s+(?:test|run\s+test)|yarn\s+test|"
    r"go\s+test|make\s+(?:test|check)|bun\s+test)(\s|$)", re.I)


def default_config() -> dict:
    custom = [{"id": f"custom-{n}", "origin": "custom", "project_path": ".",
               "prompt": "", "apply_allowed": False, "parent": "codex",
               "acceptance_command": ""} for n in range(1, 4)]
    return {
        "schema_version": 1,
        "seed": random.SystemRandom().randrange(1, 2**31),
        "tasks": custom + [{"id": "external-1", "origin": "external",
                            "source": "swebench-lite", "dataset_path": "",
                            "instance_id": "", "apply_allowed": False,
                            "parent": "claude"}],
        "arms": [
            {"name": "baseline", "enabled": True, "repotracer": False,
             "version": "parent-only"},
            {"name": "current", "enabled": True, "repotracer": True,
             "version": "working"},
            {"name": "changed", "enabled": False, "repotracer": True,
             "version": "changed", "binary": "", "source_path": ""},
        ],
        "models": {
            "codex_parent_model": "gpt-5.6-sol", "codex_parent_effort": "medium",
            "codex_scout_model": "gpt-5.6-luna", "codex_scout_effort": "auto",
            "claude_parent_model": "opus", "claude_parent_effort": "natural",
            "claude_scout_model": "opus", "claude_scout_effort": "auto-low-medium",
        },
        "grader": {"model": "gpt-6-astra", "effort": "high"},
        "investigator": {"model": "gpt-5.6-sol", "effort": "high"},
        "repotracer": {"binary": "", "source_path": ""},
        "acceptance": {"command": ""},
        "presets": [{"name": "daily", "schedule": "manual",
                     "arms": ["baseline", "current"]}],
        "rate_cards": {},
        "machine": "local",
    }


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def atomic_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    with temporary.open("w", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def create_json(path: Path, value) -> None:
    """Atomically create a JSON file without replacing a concurrent winner."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        with temporary.open("x", encoding="utf-8") as stream:
            json.dump(value, stream, indent=2, allow_nan=False)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.link(temporary, path)
        except FileExistsError:
            pass
    finally:
        temporary.unlink(missing_ok=True)


def read_json(path: Path, default=None):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return copy.deepcopy(default)


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_json(value) -> str:
    return digest_bytes(json.dumps(value, sort_keys=True, allow_nan=False).encode())


def run_command(args, cwd: Path | None = None, *, input_text: str | None = None,
                check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(args, cwd=cwd, input=input_text, text=True,
                          capture_output=True, check=check)


def safe_id(value: str, label: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}", value):
        raise ValueError(f"invalid {label} id")
    return value


def public_run(run: dict) -> dict:
    keys = ("id", "state", "created_at", "started_at", "finished_at",
            "progress", "tasks", "error", "worker_pid")
    result = {key: copy.deepcopy(run[key]) for key in keys if key in run}
    captured = {task["id"]: task for task in run.get("selected_tasks", [])}
    for task in result.get("tasks", []):
        source = captured.get(task["id"], {})
        task["origin"] = source.get("origin", task.get("origin"))
        task["apply_allowed"] = bool(source.get("apply_allowed", False))
    return result


class Store:
    def __init__(self, root: Path):
        self.root = root.resolve()
        self.config_path = self.root / "config.json"

    def initialize(self) -> None:
        self.root.mkdir(parents=True, exist_ok=True)
        for name in ("runs", "investigations", "results", "cache"):
            (self.root / name).mkdir(exist_ok=True)
        if not self.config_path.exists():
            create_json(self.config_path, default_config())

    @contextlib.contextmanager
    def lock(self, name: str = "state"):
        self.root.mkdir(parents=True, exist_ok=True)
        with (self.root / f".{name}.lock").open("a+") as stream:
            if os.name == "nt":
                if stream.tell() == 0:
                    stream.write("0")
                    stream.flush()
                stream.seek(0)
                msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
            else:
                fcntl.flock(stream, fcntl.LOCK_EX)
            try:
                yield
            finally:
                if os.name == "nt":
                    stream.seek(0)
                    msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)

    def run_path(self, identity: str) -> Path:
        safe_id(identity, "run")
        return self.root / "runs" / identity / "run.json"

    def investigation_path(self, identity: str) -> Path:
        safe_id(identity, "investigation")
        return self.root / "investigations" / identity / "investigation.json"

    def state(self) -> dict:
        self.initialize()
        runs = [read_json(p) for p in sorted((self.root / "runs").glob("*/run.json"), reverse=True)]
        jobs = [read_json(p) for p in sorted(
            (self.root / "investigations").glob("*/investigation.json"), reverse=True)]
        results = [read_json(p) for p in sorted((self.root / "results").glob("*.json"), reverse=True)]
        keyed = {(row.get("run_id"), row.get("task_id"), row.get("group")): row
                 for row in jobs if row}
        investigations = []
        for result in results:
            for pair in (result or {}).get("pairs", []):
                if not pair.get("diagnosis_required"):
                    continue
                key = (result.get("run_id"), pair.get("task"), pair.get("group"))
                row = keyed.pop(key, None) or {
                    "run_id": key[0], "task_id": key[1], "group": key[2],
                    "state": "eligible", "diagnosis": "diagnosis required",
                }
                if row.get("report"):
                    diagnosis = read_json(Path(row["report"]), {}).get("classification")
                    if diagnosis:
                        row["diagnosis"] = diagnosis
                investigations.append(row)
        investigations.extend(keyed.values())
        return {"config": read_json(self.config_path),
                "runs": [public_run(row) for row in runs if row],
                "investigations": investigations,
                "results": [row for row in results if row]}


def validate_config(config: dict) -> dict:
    if not isinstance(config, dict) or config.get("schema_version") != 1:
        raise ValueError("config schema_version must be 1")
    if config.get("machine", "local") != "local":
        raise ValueError("remote execution is not configured; run the full pair on that machine")
    try:
        seed = int(config.get("seed"))
    except (TypeError, ValueError) as error:
        raise ValueError("seed must be an integer") from error
    if not 0 <= seed < 2**63:
        raise ValueError("seed must be between 0 and 2^63-1")
    config["seed"] = seed
    for preset in config.get("presets", []):
        if preset.get("schedule") != "manual":
            raise ValueError("saved presets are manual; this workflow does not install a scheduler")
    tasks = config.get("tasks")
    if not isinstance(tasks, list):
        raise ValueError("tasks must be a list")
    seen = set()
    for task in tasks:
        identity = safe_id(task.get("id"), "task")
        if identity in seen:
            raise ValueError(f"duplicate task id: {identity}")
        seen.add(identity)
        if task.get("origin") not in {"custom", "external"}:
            raise ValueError(f"task {identity} has an invalid origin")
        if task.get("parent") not in {"codex", "claude"}:
            raise ValueError(f"task {identity} has an invalid parent")
        if type(task.get("apply_allowed", False)) is not bool:
            raise ValueError(f"task {identity} apply_allowed must be boolean")
        if task["origin"] == "external" and task.get("source", "swebench-lite") not in {"swebench-lite", "local"}:
            raise ValueError(f"task {identity} has an unsupported external source")
    arms = config.get("arms")
    if not isinstance(arms, list) or not any(row.get("enabled") for row in arms):
        raise ValueError("at least one arm must be enabled")
    names = set()
    for arm in arms:
        name = safe_id(arm.get("name"), "arm")
        if name in names:
            raise ValueError(f"duplicate arm: {name}")
        names.add(name)
        if type(arm.get("enabled")) is not bool or type(arm.get("repotracer")) is not bool:
            raise ValueError(f"arm {name} flags must be boolean")
    baseline = [row for row in arms if row.get("enabled") and row["name"] == "baseline"]
    if len(baseline) != 1 or baseline[0]["repotracer"]:
        raise ValueError("enabled baseline arm must be parent-only")
    if len([row for row in arms if row.get("enabled")]) < 2:
        raise ValueError("a comparison needs at least two enabled arms")
    changed = next((row for row in arms
                    if row["name"] == "changed" and row["enabled"]), None)
    if changed is not None:
        current = next((row for row in arms
                        if row["name"] == "current" and row["enabled"]), None)
        if current is None:
            raise ValueError("the changed arm requires an enabled current arm")
        if not changed.get("binary") and not changed.get("source_path"):
            raise ValueError(
                "the changed arm needs an explicit binary or source_path; "
                "it cannot reuse current silently")
    for provider in ("codex", "claude"):
        settings = model_settings(config, provider)
        for field in ("parent_model", "parent_effort", "scout_model", "scout_effort"):
            if not isinstance(settings.get(field), str) or not settings[field].strip():
                raise ValueError(f"missing {provider} {field}")
    if not isinstance(config.get("rate_cards", {}), dict):
        raise ValueError("rate_cards must be an object")
    special_settings(config, "grader")
    special_settings(config, "investigator")
    return config


def model_settings(config: dict, provider: str) -> dict:
    models = config.get("models") or {}
    nested = models.get(provider)
    if isinstance(nested, dict):
        return nested
    return {field: models.get(f"{provider}_{field}") for field in
            ("parent_model", "parent_effort", "scout_model", "scout_effort")}


def special_settings(config: dict, role: str) -> dict:
    defaults = {"grader": {"model": "gpt-6-astra", "effort": "high"},
                "investigator": {"model": "gpt-5.6-sol", "effort": "high"}}
    value = config.get(role, defaults[role])
    if not isinstance(value, dict) or not all(
            isinstance(value.get(key), str) and value[key].strip()
            for key in ("model", "effort")):
        raise ValueError(f"invalid {role} model settings")
    return value


def configured_tasks(config: dict) -> list[dict]:
    selected = []
    for task in config["tasks"]:
        if task["origin"] == "custom":
            if not task.get("prompt"):
                continue
            if not task.get("project_path") or not isinstance(task.get("prompt"), str) or not task["prompt"].strip():
                raise ValueError(f"custom task {task['id']} needs project_path and prompt")
        elif task.get("source") == "local" and not task.get("dataset_path"):
            raise ValueError(f"external task {task['id']} needs dataset_path for local source")
        selected.append(copy.deepcopy(task))
    if not selected:
        raise ValueError("configure at least one task")
    return selected


def validate_cost_preflight(config: dict, tasks: list[dict]) -> None:
    """Reject Codex solving runs whose native client cannot report a bill."""
    if not any(task["parent"] == "codex" for task in tasks):
        return
    cards = config.get("rate_cards", {})
    settings = model_settings(config, "codex")
    required = [(settings["parent_model"], "Codex parent")]
    if any(arm["enabled"] and arm["repotracer"] for arm in config["arms"]):
        required.append((settings["scout_model"], "Codex scout"))
    for model, label in required:
        matches = [card for card in cards.values()
                   if isinstance(card, dict) and card.get("model") == model]
        if len(matches) != 1:
            raise ValueError(
                f"{label} model {model!r} needs exactly one rate card before launch")
        card = matches[0]
        if not isinstance(card.get("source"), str) or not card["source"].strip():
            raise ValueError(f"{label} rate card needs a source")
        for field in native.TOKEN_FIELDS:
            value = card.get(field)
            if (isinstance(value, bool) or not isinstance(value, (int, float))
                    or not math.isfinite(value) or value < 0):
                raise ValueError(f"{label} rate card has an invalid {field} rate")


def pid_alive(pid) -> bool:
    if type(pid) is not int or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def spawn_worker(store: Store, run_id: str, action: str = "worker") -> int:
    command = [sys.executable, str(Path(__file__).resolve()), "--state-dir", str(store.root),
               action, "--run", run_id]
    kind = "runs" if action == "worker" else "investigations"
    log_dir = store.root / kind / run_id
    log_dir.mkdir(parents=True, exist_ok=True)
    output = (log_dir / "worker.log").open("a", encoding="utf-8", buffering=1)
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=output,
                               stderr=subprocess.STDOUT, start_new_session=True, close_fds=True)
    output.close()
    return process.pid


def start(store: Store) -> dict:
    store.initialize()
    with store.lock():
        config = validate_config(read_json(store.config_path))
        tasks = configured_tasks(config)
        validate_cost_preflight(config, tasks)
        for task in tasks:
            if task["origin"] == "custom":
                task["project_path"] = str(Path(task["project_path"]).expanduser().resolve())
            elif task.get("source") == "local":
                task["dataset_path"] = str(Path(task["dataset_path"]).expanduser().resolve())
        run_config = copy.deepcopy(config)
        repotracer_config = run_config.get("repotracer") or {}
        if not repotracer_config.get("binary"):
            repotracer_config["binary"] = (
                os.environ.get("REPOTRACER_BENCH_BINARY")
                or os.environ.get("REPOTRACER_BENCHMARK_BINARY")
                or "")
        for key in ("binary", "source_path"):
            if repotracer_config.get(key):
                repotracer_config[key] = str(
                    Path(repotracer_config[key]).expanduser().resolve())
        run_config["repotracer"] = repotracer_config
        for arm in run_config["arms"]:
            for key in ("binary", "source_path"):
                if arm.get(key):
                    arm[key] = str(Path(arm[key]).expanduser().resolve())
        for path in sorted((store.root / "runs").glob("*/run.json"), reverse=True):
            existing = read_json(path)
            resumable = (existing.get("state") in ACTIVE
                         or (existing.get("state") == "interrupted" and any(
                             arm.get("state") == "running"
                             for task in existing.get("tasks", [])
                             for arm in task.get("arms", []))))
            if resumable:
                if pid_alive(existing.get("worker_pid")):
                    return {"id": existing["id"], "state": existing["state"], "existing": True}
                pid = spawn_worker(store, existing["id"])
                latest = read_json(path, existing)
                latest.update(worker_pid=pid, recovered_at=now())
                atomic_json(path, latest)
                return {"id": existing["id"], "state": existing["state"], "recovered": True}
        identity = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ-") + uuid.uuid4().hex[:8]
        arms = [row for row in config["arms"] if row["enabled"]]
        run = {
            "schema_version": 1, "id": identity, "state": "queued", "created_at": now(),
            "progress": {"done": 0, "total": len(tasks) * len(arms)},
            "tasks": [{"id": task["id"], "origin": task["origin"],
                       "apply_allowed": task.get("apply_allowed", False), "state": "queued",
                       "arms": [{"group": arm["name"], "state": "queued", "seconds": None,
                                 "cost_usd": None, "tokens": {"parent": None, "scout": None},
                                 "usage_complete": False, "usage_missing_reason": "Not run yet"}
                                for arm in arms]} for task in tasks],
            "config": run_config, "selected_tasks": tasks,
        }
        path = store.run_path(identity)
        atomic_json(path, run)
        pid = spawn_worker(store, identity)
        latest = read_json(path, run)
        latest["worker_pid"] = pid
        atomic_json(path, latest)
        return {"id": identity, "state": "queued"}


def git(repo: Path, *args, input_text: str | None = None, check: bool = True):
    return run_command(["git", *args], repo, input_text=input_text, check=check)


def untracked(repo: Path) -> list[str]:
    return [name for name in git(
        repo, "ls-files", "--others", "--exclude-standard", "-z").stdout.split("\0") if name]


def source_fingerprint(repo: Path) -> str:
    head = git(repo, "rev-parse", "HEAD").stdout.strip().encode()
    tracked = git(repo, "diff", "--binary", "HEAD", "--").stdout.encode()
    checksum = hashlib.sha256(head + b"\0" + tracked)
    for name in sorted(untracked(repo)):
        path = repo / name
        checksum.update(name.encode() + b"\0")
        if path.is_symlink():
            checksum.update(b"link\0" + os.readlink(path).encode())
        elif path.is_file():
            checksum.update(path.read_bytes())
        checksum.update(b"\0")
    return checksum.hexdigest()


def snapshot_local(source: Path, destination: Path) -> tuple[str, str]:
    source = source.resolve()
    git(source, "rev-parse", "--show-toplevel")
    fingerprint = source_fingerprint(source)
    head = git(source, "rev-parse", "HEAD").stdout.strip()
    run_command(["git", "clone", "--no-hardlinks", "--no-checkout",
                 str(source), str(destination)])
    git(destination, "checkout", "--detach", head)
    patch = git(source, "diff", "--binary", "HEAD", "--").stdout
    if patch:
        git(destination, "apply", "--binary", "-", input_text=patch)
    for name in untracked(source):
        origin, target = source / name, destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        if origin.is_symlink():
            target.symlink_to(os.readlink(origin))
        elif origin.is_file():
            shutil.copy2(origin, target)
    git(destination, "add", "-A")
    git(destination, "-c", "user.name=RepoTracer Benchmark", "-c",
        "user.email=benchmark@localhost", "commit", "--allow-empty",
        "-m", "Captured benchmark source")
    return fingerprint, git(destination, "rev-parse", "HEAD^{tree}").stdout.strip()


def snapshot_external(repository: str, revision: str, destination: Path) -> str:
    run_command(["git", "clone", "--no-checkout", repository, str(destination)])
    git(destination, "checkout", "--detach", revision)
    return git(destination, "rev-parse", "HEAD^{tree}").stdout.strip()


def resolve(task: dict, store: Store, seed: int) -> dict:
    if task["origin"] == "custom":
        return {"id": task["id"], "prompt": task["prompt"],
                "project_path": str(Path(task["project_path"]).resolve()),
                "origin": "custom", "acceptance_patch": "",
                "acceptance_tests": {},
                "acceptance_command": task.get("acceptance_command", ""),
                "apply_allowed": task.get("apply_allowed", False)}
    try:
        from dataset import resolve_task
    except ImportError as error:
        raise ValueError("external task resolver is unavailable") from error
    resolved = resolve_task(task, store.root / "cache" / "datasets", seed)
    resolved["apply_allowed"] = False
    resolved["acceptance_command"] = task.get("acceptance_command") or resolved.get("acceptance_command", "")
    return resolved


def extract_routing(source: Path) -> str:
    path = source / "crates" / "cli" / "src" / "agents.rs"
    text = path.read_text(encoding="utf-8")
    try:
        body = text.split("ROUTING_INSTRUCTIONS: &str = concat!(", 1)[1].split("\n);", 1)[0]
        return "".join(json.loads(piece) for piece in re.findall(r'"(?:[^"\\]|\\.)*"', body))
    except (IndexError, ValueError) as error:
        raise ValueError(f"could not pin routing instructions from {path}") from error


def pin_repotracer(config: dict, run_dir: Path,
                   arm: dict | None = None) -> tuple[Path, str, str]:
    arm = arm or {"name": "current"}
    suffix = "" if arm["name"] == "current" else f"-{arm['name']}"
    executable_name = f"repotracer{suffix}" + (".exe" if os.name == "nt" else "")
    pinned = run_dir / "inputs" / executable_name
    routing_path = run_dir / "inputs" / f"routing{suffix}.txt"
    if pinned.exists() and routing_path.exists():
        routing = routing_path.read_text(encoding="utf-8")
        return pinned, digest_bytes(pinned.read_bytes()), routing
    repotracer_config = config.get("repotracer") or {}
    configured = arm.get("binary") or repotracer_config.get("binary")
    found = (configured or os.environ.get("REPOTRACER_BENCH_BINARY")
             or os.environ.get("REPOTRACER_BENCHMARK_BINARY")
             or shutil.which("repotracer"))
    if not found:
        raise ValueError("set repotracer.binary to the executable under test")
    source = Path(found).expanduser().resolve()
    if not source.is_file():
        raise ValueError(f"RepoTracer executable not found: {source}")
    pinned.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, pinned)
    pinned.chmod(pinned.stat().st_mode | 0o100)
    routing_source = arm.get("source_path") or repotracer_config.get("source_path")
    if routing_source:
        # Explicit source is authoritative. A typo must not silently switch
        # this arm to the packaged or working-tree instructions.
        routing = extract_routing(Path(routing_source).expanduser().resolve())
    elif (HERE / "routing.txt").is_file():
        routing = (HERE / "routing.txt").read_text(encoding="utf-8")
    elif (HERE.parents[1] / "crates" / "cli" / "src" / "agents.rs").is_file():
        routing = extract_routing(HERE.parents[1])
    else:
        raise ValueError(
            "routing instructions are unavailable; install routing.txt beside "
            "workflow.py or set repotracer.source_path")
    routing_path.write_text(routing, encoding="utf-8")
    return pinned, digest_bytes(pinned.read_bytes()), routing


def candidate_patch(repo: Path) -> str:
    pieces = [git(repo, "diff", "--binary", "HEAD", "--").stdout]
    for name in untracked(repo):
        result = git(repo, "diff", "--binary", "--no-index",
                     "/dev/null", name, check=False)
        if result.returncode not in (0, 1):
            raise ValueError(f"could not capture untracked file {name}: {result.stderr.strip()}")
        pieces.append(result.stdout)
    return "".join(pieces)


def parse_agent_tests(trace: Path, redactions=()) -> list[dict]:
    def clean(value) -> str:
        text = str(value or "")
        for private in redactions:
            text = text.replace(str(private), "[workspace]")
        return text

    tests = []
    claude_commands = {}
    for event in native._json_lines(trace):
        item = event.get("item") or {}
        if event.get("type") == "item.completed" and item.get("type") == "command_execution":
            command = clean(item.get("command"))
            if TEST_COMMAND.search(command):
                code = item.get("exit_code")
                status = "passed" if code == 0 else "failed" if type(code) is int else "not_run"
                output = clean(item.get("aggregated_output"))[-2000:]
                tests.append({"name": command[:300], "status": status,
                              "evidence": f"exit_code={code}\n{output}".strip()})
        message = event.get("message") or {}
        for content in message.get("content", []) if isinstance(message, dict) else []:
            if content.get("type") == "tool_use" and content.get("name") == "Bash":
                command = clean((content.get("input") or {}).get("command"))
                if TEST_COMMAND.search(command):
                    claude_commands[content.get("id")] = command
            if content.get("type") == "tool_result" and content.get("tool_use_id") in claude_commands:
                command = claude_commands.pop(content["tool_use_id"])
                code = content.get("exit_code")
                status = "passed" if code == 0 else "failed" if type(code) is int else "not_run"
                tests.append({"name": command[:300], "status": status,
                              "evidence": f"exit_code={code}\n"
                                          f"{clean(content.get('content'))[-2000:]}".strip()})
    return tests


def acceptance_records(resolved: dict, snapshot: Path, patch_path: Path,
                       destination: Path) -> list[dict]:
    command = resolved.get("acceptance_command", "")
    hidden = resolved.get("acceptance_tests") or {}
    names = [str(name) for values in hidden.values()
             if isinstance(values, list) for name in values]
    if not command:
        return [{"name": name, "status": "not_run",
                 "evidence": "No native acceptance command was configured."} for name in names]
    if destination.exists():
        # This directory contains only a generated acceptance checkout. A
        # controller may disappear after cloning but before writing evidence.
        shutil.rmtree(destination)
    repo = destination / "repo"
    run_command(["git", "clone", "--no-hardlinks", str(snapshot), str(repo)])
    patch = patch_path.read_text(encoding="utf-8")
    if patch:
        checked = git(repo, "apply", "--check", "-", input_text=patch, check=False)
        if checked.returncode:
            return [{"name": command, "status": "error",
                     "evidence": "Candidate patch did not apply to the acceptance checkout."}]
        git(repo, "apply", "-", input_text=patch)
    acceptance_patch = resolved.get("acceptance_patch") or ""
    if acceptance_patch:
        applied = git(repo, "apply", "--check", "-", input_text=acceptance_patch, check=False)
        if applied.returncode:
            return [{"name": command, "status": "error",
                     "evidence": "Hidden acceptance patch did not apply."}]
        git(repo, "apply", "-", input_text=acceptance_patch)
    result = subprocess.run(command, cwd=repo, shell=True, text=True, capture_output=True)
    evidence = (result.stdout + "\n" + result.stderr)[-4000:].strip()
    return [{"name": command,
             "status": "passed" if result.returncode == 0 else "failed",
             "evidence": f"exit_code={result.returncode}\n{evidence}".strip()}]


def token_totals(requests: list[dict]) -> dict:
    totals = {"parent": 0, "scout": 0}
    seen = set()
    for request in requests:
        if request["id"] not in seen:
            seen.add(request["id"])
            totals[request["role"]] += sum(request["tokens"].values())
    return totals


def update_arm(run: dict, task_id: str, group: str, **changes) -> None:
    task = next(row for row in run["tasks"] if row["id"] == task_id)
    arm = next(row for row in task["arms"] if row["group"] == group)
    arm.update(changes)


def prepare_task(store: Store, run: dict, task_config: dict,
                 run_dir: Path) -> tuple[dict, Path]:
    resolved = resolve(task_config, store, int(run["config"]["seed"]))
    if not resolved.get("acceptance_command"):
        resolved["acceptance_command"] = (run["config"].get("acceptance") or {}).get("command", "")
    task_dir = run_dir / "tasks" / task_config["id"]
    task_dir.mkdir(parents=True, exist_ok=True)
    snapshot = task_dir / "snapshot"
    if not snapshot.exists():
        if resolved["origin"] == "custom":
            fingerprint, tree = snapshot_local(Path(resolved["project_path"]), snapshot)
            resolved["source_fingerprint"] = fingerprint
        else:
            tree = snapshot_external(resolved["repository"], resolved["revision"], snapshot)
        resolved["snapshot"] = tree
        atomic_json(task_dir / "acceptance-private.json",
                    {key: resolved.get(key) for key in ("acceptance_patch", "acceptance_tests")})
        atomic_json(task_dir / "task.json",
                    {key: value for key, value in resolved.items()
                     if key not in {"acceptance_patch", "acceptance_tests"}})
        (task_dir / "prompt.txt").write_text(resolved["prompt"], encoding="utf-8")
    else:
        resolved = {**read_json(task_dir / "task.json"),
                    **read_json(task_dir / "acceptance-private.json", {})}
    prompt_sha = digest_bytes(resolved["prompt"].encode())
    public_id = (resolved.get("instance_id") or resolved.get("id")
                 or task_config["id"])
    resolved["prompt_sha256"] = prompt_sha
    resolved["benchmark_id"] = f"{public_id}-{prompt_sha[:12]}"
    task_record = read_json(task_dir / "task.json", {})
    if (task_record.get("prompt_sha256") != prompt_sha
            or task_record.get("benchmark_id") != resolved["benchmark_id"]):
        task_record.update(prompt_sha256=prompt_sha,
                           benchmark_id=resolved["benchmark_id"])
        atomic_json(task_dir / "task.json", task_record)
    return resolved, snapshot


def run_one_arm(run: dict, resolved: dict, snapshot: Path, task_config: dict,
                arm_config: dict, repotracer: Path | None, routing: str | None,
                run_path: Path) -> None:
    task_id, group = task_config["id"], arm_config["name"]
    trial = run_path.parent / "tasks" / task_id / "arms" / group
    repo = trial / "repo"
    trial.mkdir(parents=True, exist_ok=True)
    task_row = next(row for row in run["tasks"] if row["id"] == task_id)
    current = next(row for row in task_row["arms"] if row["group"] == group)
    resume = current["state"] == "running" and (trial / "trajectory.jsonl").exists()
    if not repo.exists():
        git(snapshot, "worktree", "add", "--detach", str(repo), "HEAD")
    update_arm(run, task_id, group, state="running",
               started_at=current.get("started_at") or now(), error=None)
    atomic_json(run_path, run)

    def status(values: dict) -> None:
        update_arm(run, task_id, group, **values)
        atomic_json(run_path, run)

    settings = model_settings(run["config"], task_config["parent"])
    evidence = native.run_arm(
        task_config["parent"], repo, resolved["prompt"], settings, trial,
        arm_config["repotracer"], repotracer, routing,
        run["config"].get("rate_cards", {}), resume=resume,
        status_callback=status)
    try:
        first_started = dt.datetime.fromisoformat(current["started_at"])
        evidence["seconds"] = round(
            (dt.datetime.now(dt.timezone.utc) - first_started).total_seconds(), 3)
    except (KeyError, TypeError, ValueError):
        if resume:
            evidence["seconds"] = None
    patch_path = trial / "candidate.patch"
    patch_path.write_text(candidate_patch(repo), encoding="utf-8")
    tests_path = trial / "agent-tests.json"
    atomic_json(tests_path, parse_agent_tests(
        Path(evidence["trace"]), (repo, resolved.get("project_path", ""))))
    acceptance_path = trial / "acceptance.json"
    if not acceptance_path.exists():
        atomic_json(acceptance_path, acceptance_records(
            resolved, snapshot, patch_path, trial / "acceptance-workspace"))
    state = "completed" if evidence["terminal_success"] else "failed"
    update_arm(
        run, task_id, group, state=state, finished_at=now(),
        seconds=evidence["seconds"], requests=evidence["requests"],
        tokens=token_totals(evidence["requests"]),
        usage_complete=evidence["usage_complete"],
        usage_missing_reason=evidence["usage_missing_reason"],
        cost_usd=evidence["reported_cost_usd"],
        reported_cost_usd=evidence["reported_cost_usd"],
        reported_cost_by_role_usd=evidence.get("reported_cost_by_role_usd"),
        reported_cost_sources=evidence.get("reported_cost_sources"),
        cost_missing_reason=evidence.get("cost_missing_reason"),
        time_missing_reason=(None if evidence["seconds"] is not None
                             else "Full elapsed time could not be reconstructed after resume."),
        session_id=evidence["session_id"],
        exit_code=evidence["exit_code"], patch=str(patch_path),
        acceptance_checks=str(acceptance_path), agent_tests=str(tests_path),
        trace=evidence["trace"], error=None if evidence["terminal_success"]
        else f"native {task_config['parent']} exited {evidence['exit_code']}")
    run["progress"]["done"] = sum(
        arm["state"] in TERMINAL for task in run["tasks"] for arm in task["arms"])
    atomic_json(run_path, run)


def build_manifest(run: dict, run_dir: Path) -> Path:
    tasks = []
    machine = f"{platform.system()}-{platform.machine()}-{socket.gethostname()}"
    arm_configs = {row["name"]: row for row in run["config"]["arms"]}
    task_configs = {row["id"]: row for row in run["selected_tasks"]}
    for task in run["tasks"]:
        task_dir = run_dir / "tasks" / task["id"]
        resolved = read_json(task_dir / "task.json")
        config = task_configs[task["id"]]
        candidates = []
        benchmark_id = resolved.get("benchmark_id") or task.get("benchmark_id") or task["id"]
        for arm in task["arms"]:
            arm_config = arm_configs[arm["group"]]
            settings = model_settings(run["config"], config["parent"])
            candidates.append({
                "id": f"{benchmark_id}-{arm['group']}", "group": arm["group"],
                "version": arm.get("version", arm_config["version"]),
                "parent": {"model": settings["parent_model"],
                           "effort": settings["parent_effort"]},
                "scout": ({"model": settings["scout_model"],
                           "effort": settings["scout_effort"]}
                          if arm_config["repotracer"] else None),
                "machine": machine, "snapshot": resolved["snapshot"],
                "status": arm["state"], "seconds": arm.get("seconds"),
                "time_missing_reason": (None if arm.get("seconds") is not None
                                        else "Native elapsed time unavailable"),
                "usage_complete": arm.get("usage_complete", False),
                "usage_missing_reason": arm.get("usage_missing_reason"),
                "requests": arm.get("requests", []),
                "reported_cost_by_role_usd": arm.get("reported_cost_by_role_usd"),
                "reported_cost_sources": arm.get("reported_cost_sources"),
                "patch": os.path.relpath(arm["patch"], run_dir),
                "acceptance_checks": os.path.relpath(arm["acceptance_checks"], run_dir),
                "agent_tests": os.path.relpath(arm["agent_tests"], run_dir),
            })
        tasks.append({
            "id": benchmark_id,
            "origin": "external" if config["origin"] == "external" else "user",
            "purpose": "validation", "derived_from": [],
            "prompt": resolved["prompt"],
            "context": (f"Starting source tree {resolved['snapshot']}. "
                        "Acceptance checks are reported separately."),
            "candidates": candidates,
        })
    manifest = {
        "schema_version": 1, "experiment": run["id"],
        "development_tasks": [], "redact_test_strings": [],
        "rate_cards": run["config"].get("rate_cards", {}), "tasks": tasks,
    }
    path = run_dir / "manifest.json"
    atomic_json(path, manifest)
    return path


def grader_prompt(packet: Path) -> str:
    files = sorted(path for path in packet.rglob("*") if path.is_file())
    chunks = ["Use only the benchmark packet below. Do not inspect other files or use RepoTracer.\n"]
    for path in files:
        chunks += [f"\n--- {path.relative_to(packet).as_posix()} ---\n",
                   path.read_text(encoding="utf-8")]
    return "".join(chunks)


def parse_json_answer(text: str):
    value = text.strip()
    fence = chr(96) * 3
    if value.startswith(fence):
        value = re.sub(r"^.{3}(?:json)?\s*|\s*.{3}$", "", value, flags=re.S)
    return json.loads(value)


def grade_run(run: dict, run_path: Path, repotracer: Path | None,
              routing: str | None) -> dict:
    run_dir = run_path.parent
    manifest_path = build_manifest(run, run_dir)
    reviews = []
    grading_usage = []
    for task in run["tasks"]:
        task_id = task["id"]
        benchmark_id = task.get("benchmark_id", task_id)
        grade_dir = run_dir / "grading" / task_id
        packet, key = grade_dir / "packet", grade_dir / "key.json"
        grades_path = grade_dir / "grades.json"
        if not packet.exists():
            bench.blind(manifest_path, benchmark_id, packet, key)
        if not grades_path.exists():
            session = grade_dir / "session"
            session.mkdir(parents=True, exist_ok=True)
            grader = special_settings(run["config"], "grader")
            settings = {
                "parent_model": grader["model"],
                "parent_effort": grader["effort"],
                "scout_model": "unused", "scout_effort": "unused",
            }
            result = native.run_arm(
                "codex", session, grader_prompt(packet), settings,
                grade_dir / "native", False, repotracer, routing,
                run["config"].get("rate_cards", {}),
                resume=(grade_dir / "native" / "trajectory.jsonl").exists())
            grading_usage.append(result)
            grades = parse_json_answer(
                Path(result["final"]).read_text(encoding="utf-8"))
            expected = set(read_json(key)["labels"])
            if set(grades) != expected:
                raise ValueError(f"grader omitted candidates for {benchmark_id}")
            atomic_json(grades_path, grades)
        reviews.append((key, grades_path))
    manifest = bench.read_json(manifest_path)
    report = bench.report(manifest, bench.load_grades(manifest, reviews))
    atomic_json(run_dir / "report.json", report)
    run["overhead"] = {
        "grading": grading_usage,
        "note": "Grading and investigation spend is separate from solving arms.",
    }
    return report


def result_summary(run: dict, report: dict) -> dict:
    pairs = copy.deepcopy(report.get("pairs", []))
    for pair in pairs:
        task = next(row for row in run["tasks"]
                    if row["id"] == pair["task"]
                    or row.get("benchmark_id") == pair["task"])
        arm = next(row for row in task["arms"] if row["group"] == pair["group"])
        pair.update({
            "run_id": run["id"], "task_id": pair["task"],
            "cost_usd": arm.get("cost_usd"), "seconds": arm.get("seconds"),
            "tokens": arm.get("tokens"),
            "usage_complete": arm.get("usage_complete", False),
            "usage_missing_reason": arm.get("usage_missing_reason"),
            "state": arm.get("state"),
        })
    return {"run_id": run["id"], "state": "completed",
            "report": run["report"], "finished_at": run["finished_at"],
            "pairs": pairs}


def worker(store: Store, run_id: str) -> None:
    path = store.run_path(run_id)
    run_dir = path.parent
    with store.lock(f"worker-{run_id}"):
        with store.lock():
            run = read_json(path)
            if not run or run.get("state") in {"completed", "failed"}:
                return
            run.update(state="preparing",
                       started_at=run.get("started_at") or now(),
                       worker_pid=os.getpid())
            atomic_json(path, run)
        try:
            arm_configs = [row for row in run["config"]["arms"] if row["enabled"]]
            arm_pins = {}
            for arm_config in arm_configs:
                if not arm_config["repotracer"]:
                    continue
                binary, binary_sha, routing = pin_repotracer(
                    run["config"], run_dir, arm_config)
                routing_sha = digest_bytes(routing.encode())
                arm_pins[arm_config["name"]] = {
                    "binary": binary, "routing": routing,
                    "repotracer_sha256": binary_sha,
                    "routing_sha256": routing_sha,
                }
            if "changed" in arm_pins:
                current = arm_pins["current"]
                changed = arm_pins["changed"]
                if ((current["repotracer_sha256"], current["routing_sha256"])
                        == (changed["repotracer_sha256"], changed["routing_sha256"])):
                    raise ValueError(
                        "the changed arm resolves to the same binary and routing as current")
            run["pinned"] = {
                "config_sha256": digest_json(run["config"]),
                "arms": {name: {key: value for key, value in values.items()
                                 if key not in {"binary", "routing"}}
                         for name, values in arm_pins.items()},
            }
            rng = random.Random(f"{run['config']['seed']}:{run_id}")
            for task_config in run["selected_tasks"]:
                task_row = next(row for row in run["tasks"]
                                if row["id"] == task_config["id"])
                if all(row["state"] in TERMINAL for row in task_row["arms"]):
                    continue
                task_row["state"] = "running"
                resolved, snapshot = prepare_task(
                    store, run, task_config, run_dir)
                task_row["benchmark_id"] = resolved["benchmark_id"]
                task_row["prompt_sha256"] = resolved["prompt_sha256"]
                if task_row.get("arm_order"):
                    by_name = {row["name"]: row for row in arm_configs}
                    order = [by_name[name] for name in task_row["arm_order"]]
                else:
                    order = list(arm_configs)
                    rng.shuffle(order)
                    task_row["arm_order"] = [row["name"] for row in order]
                run["state"] = "running"
                atomic_json(path, run)
                for arm_config in order:
                    arm = next(row for row in task_row["arms"]
                               if row["group"] == arm_config["name"])
                    if arm["state"] not in TERMINAL:
                        pin = arm_pins.get(arm_config["name"])
                        if pin:
                            arm["version"] = (
                                f"{arm_config['version']}+binary-"
                                f"{pin['repotracer_sha256'][:12]}+routing-"
                                f"{pin['routing_sha256'][:12]}")
                        else:
                            arm["version"] = arm_config["version"]
                        run_one_arm(run, resolved, snapshot, task_config,
                                    arm_config,
                                    pin["binary"] if pin else None,
                                    pin["routing"] if pin else None, path)
                task_row["state"] = ("completed" if all(
                    row["state"] == "completed" for row in task_row["arms"])
                                     else "failed")
                atomic_json(path, run)
            run["state"] = "grading"
            atomic_json(path, run)
            report = grade_run(run, path, None, None)
            run.update(state="completed", finished_at=now(),
                       report=str(run_dir / "report.json"))
            atomic_json(store.root / "results" / f"{run_id}.json",
                        result_summary(run, report))
        except (OSError, ValueError, KeyError, TypeError,
                subprocess.SubprocessError) as error:
            run["state"] = ("interrupted" if any(
                arm["state"] == "running" for task in run.get("tasks", [])
                for arm in task.get("arms", [])) else "failed")
            run.update(error=str(error), finished_at=now())
        finally:
            atomic_json(path, run)


def eligible_pair(run: dict, task_id: str, group: str) -> dict:
    if not run.get("report"):
        raise ValueError("run has no graded report")
    report = read_json(Path(run["report"]))
    task = next((row for row in run.get("tasks", [])
                 if row["id"] == task_id or row.get("benchmark_id") == task_id), None)
    report_id = task.get("benchmark_id", task_id) if task else task_id
    pair = next((row for row in report["pairs"]
                 if row["task"] == report_id and row["group"] == group), None)
    if pair is None or not pair.get("diagnosis_required"):
        raise ValueError("investigation is available only for a graded loss")
    return pair


def start_investigation(store: Store, run_id: str,
                        task_id: str, group: str) -> dict:
    run = read_json(store.run_path(run_id))
    if not run:
        raise ValueError("unknown run")
    eligible_pair(run, task_id, group)
    task = next((row for row in run["tasks"]
                 if row["id"] == task_id or row.get("benchmark_id") == task_id), None)
    if task is None:
        raise ValueError("unknown task")
    report_task_id = task.get("benchmark_id", task["id"])
    safe_id(report_task_id, "task")
    safe_id(group, "group")
    identity = f"{run_id}-{report_task_id}-{group}"
    path = store.investigation_path(identity)
    with store.lock():
        existing = read_json(path)
        if existing and existing.get("state") in TERMINAL:
            return {"id": identity, "state": existing["state"], "existing": True}
        if existing and existing.get("state") in ACTIVE:
            if pid_alive(existing.get("worker_pid")):
                return {"id": identity, "state": existing["state"], "existing": True}
            pid = spawn_worker(store, identity, "investigation-worker")
            latest = read_json(path, existing)
            latest.update(worker_pid=pid, recovered_at=now())
            atomic_json(path, latest)
            return {"id": identity, "state": existing["state"], "recovered": True}
        record = {"id": identity, "run_id": run_id, "task_id": report_task_id,
                  "run_task_id": task["id"],
                  "group": group, "state": "queued", "created_at": now()}
        atomic_json(path, record)
        record["worker_pid"] = spawn_worker(
            store, identity, "investigation-worker")
        atomic_json(path, record)
    return {"id": identity, "state": "queued"}


def investigation_worker(store: Store, identity: str) -> None:
    path = store.investigation_path(identity)
    record = read_json(path)
    if not record or record.get("state") in TERMINAL:
        return
    with store.lock(f"worker-{identity}"):
        with store.lock():
            record = read_json(path)
            record.update(state="running", started_at=record.get("started_at") or now(),
                          worker_pid=os.getpid())
            atomic_json(path, record)
        try:
            run = read_json(store.run_path(record["run_id"]))
            pair = eligible_pair(run, record["task_id"], record["group"])
            task = next(row for row in run["tasks"]
                        if row["id"] == record.get("run_task_id", record["task_id"])
                        or row.get("benchmark_id") == record["task_id"])
            arm = next(row for row in task["arms"]
                       if row["group"] == record["group"])
            baseline = next(row for row in task["arms"]
                            if row["group"] == "baseline")
            evidence_dir = path.parent / "evidence"
            evidence_dir.mkdir(parents=True, exist_ok=True)
            evidence = {
                "pair": pair, "task": record["task_id"],
                "group": record["group"],
                "native_final": Path(arm["trace"]).with_name(
                    "final.md").read_text(encoding="utf-8", errors="replace"),
                "patch": Path(arm["patch"]).read_text(
                    encoding="utf-8", errors="replace"),
                "trace": Path(arm["trace"]).read_text(
                    encoding="utf-8", errors="replace"),
                "baseline_final": Path(baseline["trace"]).with_name(
                    "final.md").read_text(encoding="utf-8", errors="replace"),
                "baseline_patch": Path(baseline["patch"]).read_text(
                    encoding="utf-8", errors="replace"),
                "baseline_trace": Path(baseline["trace"]).read_text(
                    encoding="utf-8", errors="replace"),
            }
            atomic_json(evidence_dir / "evidence.json", evidence)
            prompt = (
                "Diagnose the graded benchmark loss recorded in evidence.json in the "
                "current directory. Read that file as needed. "
                "Do not edit files, rerun the task, or change the grade. Classify it as "
                "bug, tradeoff, or unresolved. Cite concrete trace or patch evidence and "
                "give a proposed fix only when supported. Return only JSON with "
                "classification, evidence, and proposed_fix.")
            investigator = special_settings(run["config"], "investigator")
            settings = {
                "parent_model": investigator["model"],
                "parent_effort": investigator["effort"],
                "scout_model": "unused", "scout_effort": "unused",
            }
            result = native.run_arm(
                "codex", evidence_dir, prompt, settings,
                path.parent / "native", False, None, None,
                run["config"].get("rate_cards", {}),
                resume=(path.parent / "native" / "trajectory.jsonl").exists())
            answer = parse_json_answer(
                Path(result["final"]).read_text(encoding="utf-8"))
            if answer.get("classification") not in {"bug", "tradeoff", "unresolved"}:
                raise ValueError("investigator returned an invalid classification")
            report_path = path.parent / "report.json"
            atomic_json(report_path, answer)
            record.update(state="completed", finished_at=now(),
                          report=str(report_path), usage=result)
        except (OSError, ValueError, KeyError, TypeError,
                subprocess.SubprocessError) as error:
            record.update(state="failed", finished_at=now(), error=str(error))
        atomic_json(path, record)


def apply_candidate(store: Store, run_id: str,
                    task_id: str, group: str) -> dict:
    run = read_json(store.run_path(run_id))
    if not run:
        raise ValueError("unknown run")
    task = next((row for row in run["tasks"]
                 if row["id"] == task_id or row.get("benchmark_id") == task_id), None)
    internal_id = task["id"] if task else task_id
    task_config = next((row for row in run["selected_tasks"]
                        if row["id"] == internal_id), None)
    if (not task_config or task_config["origin"] != "custom"
            or not task_config.get("apply_allowed")):
        raise ValueError("only an explicitly allowed custom task can be applied")
    task = next(row for row in run["tasks"] if row["id"] == internal_id)
    arm = next((row for row in task["arms"] if row["group"] == group), None)
    if not arm or arm["state"] != "completed":
        raise ValueError("candidate is not completed")
    resolved = read_json(
        store.root / "runs" / run_id / "tasks" / internal_id / "task.json")
    source = Path(resolved["project_path"])
    patch = Path(arm["patch"]).read_text(encoding="utf-8")
    with store.lock(f"apply-{run_id}-{internal_id}"):
        if source_fingerprint(source) != resolved["source_fingerprint"]:
            raise ValueError("source changed since capture; candidate was not applied")
        if not patch:
            return {"run_id": run_id, "task_id": task_id, "group": group,
                    "state": "applied", "committed": False,
                    "upstream_changed": False, "no_changes": True}
        checked = git(source, "apply", "--check", "-", input_text=patch, check=False)
        if checked.returncode:
            raise ValueError(
                f"candidate no longer applies: {checked.stderr.strip()}")
        git(source, "apply", "-", input_text=patch)
    return {"run_id": run_id, "task_id": task_id, "group": group,
            "state": "applied", "committed": False,
            "upstream_changed": False}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", type=Path, required=True)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("init", "get", "save", "start"):
        commands.add_parser(name)
    investigate = commands.add_parser("investigate")
    apply = commands.add_parser("apply")
    for command in (investigate, apply):
        command.add_argument("--run", required=True)
        command.add_argument("--task", required=True)
        command.add_argument("--group", required=True)
    worker_command = commands.add_parser("worker", help=argparse.SUPPRESS)
    worker_command.add_argument("--run", required=True)
    investigation_command = commands.add_parser(
        "investigation-worker", help=argparse.SUPPRESS)
    investigation_command.add_argument("--run", required=True)
    args = parser.parse_args()
    store = Store(args.state_dir)
    try:
        if args.command in {"init", "get"}:
            store.initialize()
            result = store.state()
        elif args.command == "save":
            value = json.load(sys.stdin)
            config = value.get("config") if set(value) == {"config"} else value
            validate_config(config)
            store.initialize()
            with store.lock():
                atomic_json(store.config_path, config)
            result = store.state()
        elif args.command == "start":
            result = start(store)
        elif args.command == "investigate":
            result = start_investigation(
                store, args.run, args.task, args.group)
        elif args.command == "apply":
            result = apply_candidate(
                store, args.run, args.task, args.group)
        elif args.command == "worker":
            worker(store, args.run)
            result = {"id": args.run,
                      "state": read_json(store.run_path(args.run))["state"]}
        else:
            investigation_worker(store, args.run)
            result = {"id": args.run,
                      "state": read_json(
                          store.investigation_path(args.run))["state"]}
        print(json.dumps(result, allow_nan=False))
    except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError,
            subprocess.SubprocessError) as error:
        print(json.dumps({"error": str(error)}, allow_nan=False),
              file=sys.stderr)
        raise SystemExit(2)


if __name__ == "__main__":
    main()
