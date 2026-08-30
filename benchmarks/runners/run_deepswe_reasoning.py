#!/usr/bin/env python3
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import random
import shutil
import sqlite3
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
STUDY = ROOT / "benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning"
PRIVATE = STUDY / "private"
PROTOCOL = STUDY / "protocol.json"
DEEPSWE = PRIVATE / "deep-swe"
REPOTRACER = ROOT / "benchmarks/results/runs/2026-08-27-scout-tools/private/behavior-budget/bin/adaptive-repotracer"
AUTH = Path.home() / ".codex/auth.json"
POLICY = PRIVATE / "policy/AGENTS.md"
CODEX = shutil.which("codex")
BILLING_DB = Path.home() / ".codex-lb/store.db"
SEED = 20260836

TASKS = {
    "boa-hierarchical-evaluation-cancellation": {
        "source": PRIVATE / "sources/boa",
        "base": "70409a5052984325dccfdc5f6520818568a81f39",
    },
    "claude-code-by-agents-recursive-delegation": {
        "source": PRIVATE / "sources/claude-code-by-agents",
        "base": "5e0a2247d446c49a9951a06bb83b6e956dc7eb41",
    },
}


def require_inputs() -> None:
    missing = [
        str(path)
        for path in (PROTOCOL, DEEPSWE, REPOTRACER, AUTH, POLICY, BILLING_DB)
        if not path.exists()
    ]
    if not CODEX:
        missing.append("codex")
    for task in TASKS.values():
        if not task["source"].exists():
            missing.append(str(task["source"]))
    if missing:
        raise SystemExit("missing benchmark input: " + ", ".join(missing))


def scout_config(effort: str) -> str:
    return f'''[model]
backend = "codex-cli"
model = "gpt-5.6-luna"
reasoning_effort = "{effort}"
timeout_ms = 180000
temperature = 0.0

[explorer]
max_turns = 6
timeout_seconds = 0
max_tool_calls = 40
tool_timeout_seconds = 30
concurrency = 8
codegraph = false

[notifications]
update_available = false
'''


def parent_config(config: Path) -> str:
    return f'''model = "gpt-5.6-sol"
model_reasoning_effort = "high"
model_provider = "codex-lb"
service_tier = "default"

[model_providers.codex-lb]
name = "openai"
base_url = "http://127.0.0.1:2455/backend-api/codex"
wire_api = "responses"
supports_websockets = true
requires_openai_auth = true

[mcp_servers.repotracer]
command = "{REPOTRACER}"
args = ["--config", "{config}", "serve"]
'''


def run_command(args: list[str], *, cwd: Path, timeout: int = 300) -> subprocess.CompletedProcess:
    return subprocess.run(args, cwd=cwd, check=True, stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout)


def prepare(job: dict) -> Path:
    trial = PRIVATE / "trials" / job["id"]
    result = trial / "result.json"
    if result.exists():
        return trial
    if trial.exists():
        shutil.rmtree(trial)
    trial.mkdir(parents=True)
    source = TASKS[job["task_id"]]["source"]
    source_base = run_command(["git", "rev-parse", "HEAD"], cwd=source).stdout.decode().strip()
    repo = trial / "repo"
    run_command(["git", "clone", "-q", "--shared", str(source), str(repo)], cwd=ROOT, timeout=600)
    run_command(["git", "checkout", "-q", "-B", "benchmark", source_base], cwd=repo)
    run_command(["git", "config", "user.name", "DeepSWE Benchmark"], cwd=repo)
    run_command(["git", "config", "user.email", "benchmark@example.invalid"], cwd=repo)

    home = trial / "home"
    codex_home = home / ".codex"
    codex_home.mkdir(parents=True)
    cargo_home = home / ".cargo"
    cargo_home.mkdir()
    (cargo_home / "env").touch()
    shutil.copy2(AUTH, codex_home / "auth.json")
    shutil.copy2(POLICY, codex_home / "AGENTS.md")
    config = trial / "repotracer.toml"
    config.write_text(scout_config(job["arm"]))
    (codex_home / "config.toml").write_text(parent_config(config))

    task_dir = DEEPSWE / "tasks" / job["task_id"]
    prompt = (task_dir / "instruction.md").read_text()
    (trial / "prompt.txt").write_text(prompt)
    (trial / "job.json").write_text(json.dumps(job, indent=2) + "\n")
    (trial / "source-base.txt").write_text(source_base + "\n")
    return trial


def parse_events(path: Path) -> dict:
    events = []
    if path.exists():
        for line in path.read_text(errors="replace").splitlines():
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    started = [
        event.get("item", {})
        for event in events
        if event.get("type") == "item.started"
    ]
    scout_calls = [
        item
        for item in started
        if item.get("type") == "mcp_tool_call"
        and item.get("server") == "repotracer"
        and item.get("tool") == "repo_scout"
    ]
    first_scout = next((index for index, item in enumerate(started) if item in scout_calls), None)
    usage = next(
        (event.get("usage", {}) for event in reversed(events) if event.get("type") == "turn.completed"),
        {},
    )
    thread_id = next(
        (event.get("thread_id") for event in events if event.get("type") == "thread.started"),
        None,
    )
    return {
        "scout_calls": len(scout_calls),
        "operations_before_scout": first_scout,
        "main_usage": usage,
        "parent_thread_id": thread_id,
    }


def session_ids(home: Path) -> set[str]:
    ids = set()
    for path in (home / ".codex/sessions").glob("**/*.jsonl"):
        try:
            first = path.open(errors="replace").readline()
            event = json.loads(first)
        except (OSError, json.JSONDecodeError):
            continue
        if event.get("type") == "session_meta":
            value = event.get("payload", {}).get("id")
            if value:
                ids.add(value)
    return ids


def billing(ids: set[str], connection: sqlite3.Connection) -> dict:
    keys = ["requests", "input_tokens", "cached_input_tokens", "output_tokens", "reasoning_tokens", "cost_usd"]
    if not ids:
        return dict.fromkeys(keys, 0)
    placeholders = ",".join("?" for _ in ids)
    row = connection.execute(
        f'''SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0),
                   COALESCE(SUM(output_tokens), 0), COALESCE(SUM(reasoning_tokens), 0), COALESCE(SUM(cost_usd), 0)
            FROM request_logs WHERE conversation_id IN ({placeholders}) AND deleted_at IS NULL''',
        tuple(ids),
    ).fetchone()
    return dict(zip(keys, row))


def run_trial(job: dict) -> dict:
    trial = prepare(job)
    result_path = trial / "result.json"
    if result_path.exists():
        return json.loads(result_path.read_text())
    repo = trial / "repo"
    target_dir = Path("/tmp/repotracer-deepswe-target") / job["id"]
    target_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update({
        "HOME": str(trial / "home"),
        "CODEX_HOME": str(trial / "home/.codex"),
        "NO_COLOR": "1",
        "CARGO_TARGET_DIR": str(target_dir),
    })
    started = time.monotonic()
    timed_out = False
    try:
        with (trial / "trajectory.jsonl").open("w") as stdout, (trial / "stderr.log").open("w") as stderr:
            process = subprocess.run(
                [
                    CODEX,
                    "exec",
                    "--skip-git-repo-check",
                    "--sandbox", "workspace-write",
                    "--add-dir", str(repo / ".git"),
                    "--json",
                    "-o", str(trial / "final.md"),
                    "-C", str(repo),
                    (trial / "prompt.txt").read_text(),
                ],
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                text=True,
                timeout=10800,
            )
        exit_code = process.returncode
    except subprocess.TimeoutExpired:
        exit_code = 124
        timed_out = True
    wall = round(time.monotonic() - started, 3)
    shutil.rmtree(target_dir, ignore_errors=True)

    source_base = (trial / "source-base.txt").read_text().strip()
    subprocess.run(["git", "add", "-N", "."], cwd=repo, stdin=subprocess.DEVNULL, capture_output=True)
    patch = subprocess.run(
        ["git", "diff", "--binary", source_base],
        cwd=repo,
        stdin=subprocess.DEVNULL,
        capture_output=True,
    ).stdout
    (trial / "model.patch").write_bytes(patch)
    changed = subprocess.run(
        ["git", "diff", "--name-only", source_base],
        cwd=repo,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
    ).stdout.splitlines()
    event_metrics = parse_events(trial / "trajectory.jsonl")
    ids = session_ids(trial / "home")
    parent_id = event_metrics.pop("parent_thread_id")
    with sqlite3.connect(f"file:{BILLING_DB}?mode=ro", uri=True) as connection:
        complete_billing = billing(ids, connection)
        parent_billing = billing({parent_id} if parent_id else set(), connection)
        scout_billing = billing(ids - ({parent_id} if parent_id else set()), connection)
    result = {
        **job,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "wall_seconds": wall,
        "changed_paths": changed,
        "patch_bytes": len(patch),
        **event_metrics,
        "billing": complete_billing,
        "parent_billing": parent_billing,
        "scout_billing": scout_billing,
    }
    result_path.write_text(json.dumps(result, indent=2) + "\n")
    return result


def verifier_image(task_id: str) -> str:
    return "repotracer-deepswe-verifier:" + task_id


def build_verifier(task_id: str) -> None:
    tests = DEEPSWE / "tasks" / task_id / "tests"
    subprocess.run(
        ["docker", "build", "--platform", "linux/amd64", "-q", "-t", verifier_image(task_id), str(tests)],
        cwd=ROOT,
        check=True,
        stdin=subprocess.DEVNULL,
        timeout=3600,
    )


def verify_trial(trial: Path) -> dict:
    result_path = trial / "result.json"
    result = json.loads(result_path.read_text())
    if "verifier" in result:
        return result
    verifier_dir = trial / "verifier"
    verifier_dir.mkdir(exist_ok=True)
    stdout_path = verifier_dir / "stdout.log"
    stderr_path = verifier_dir / "stderr.log"
    started = time.monotonic()
    with stdout_path.open("w") as stdout, stderr_path.open("w") as stderr:
        process = subprocess.run(
            [
                "docker", "run", "--rm", "--platform", "linux/amd64", "--cpus", "2", "--memory", "8g",
                "-v", f"{trial / 'model.patch'}:/logs/artifacts/model.patch:ro",
                "-v", f"{verifier_dir}:/logs/verifier",
                verifier_image(result["task_id"]),
                "/tests/test.sh",
            ],
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            text=True,
            timeout=1800,
        )
    reward_path = verifier_dir / "reward.json"
    reward = json.loads(reward_path.read_text()) if reward_path.exists() else None
    result["verifier"] = {
        "exit_code": process.returncode,
        "wall_seconds": round(time.monotonic() - started, 3),
        "reward": reward,
    }
    result_path.write_text(json.dumps(result, indent=2) + "\n")
    return result


def jobs(selected_task: str | None) -> list[dict]:
    planned = []
    for task_id, task in TASKS.items():
        if selected_task and task_id != selected_task:
            continue
        for repeat in range(3):
            for arm in ("medium", "high"):
                planned.append({
                    "id": f"{task_id}-r{repeat}-{arm}",
                    "task_id": task_id,
                    "base": task["base"],
                    "repeat": repeat,
                    "arm": arm,
                })
    random.Random(SEED).shuffle(planned)
    return planned


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", choices=("run", "verify", "all"), default="all")
    parser.add_argument("--task")
    parser.add_argument("--workers", type=int, default=4)
    args = parser.parse_args()
    require_inputs()
    planned = jobs(args.task)
    PRIVATE.mkdir(parents=True, exist_ok=True)
    (PRIVATE / "run-order.json").write_text(json.dumps([job["id"] for job in planned], indent=2) + "\n")
    results = []
    if args.phase in {"run", "all"}:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
            futures = {pool.submit(run_trial, job): job for job in planned}
            for future in concurrent.futures.as_completed(futures):
                result = future.result()
                results.append(result)
                print(json.dumps({key: result[key] for key in ("id", "exit_code", "wall_seconds", "scout_calls")}), flush=True)
    if args.phase in {"verify", "all"}:
        for task_id in dict.fromkeys(job["task_id"] for job in planned):
            build_verifier(task_id)
            for job in planned:
                if job["task_id"] != task_id:
                    continue
                result = verify_trial(PRIVATE / "trials" / job["id"])
                print(json.dumps({"id": result["id"], "verifier": result["verifier"]}), flush=True)
    all_results = [
        json.loads(path.read_text())
        for path in sorted((PRIVATE / "trials").glob("*/result.json"))
    ]
    (PRIVATE / "results.json").write_text(json.dumps(all_results, indent=2) + "\n")


if __name__ == "__main__":
    main()
