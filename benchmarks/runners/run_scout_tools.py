#!/usr/bin/env python3
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import random
import shlex
import shutil
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
STUDY = ROOT / "benchmarks/results/runs/2026-08-27-scout-tools"
PROTOCOL = STUDY / "protocol.json"
PRIVATE = STUDY / "private"


def tracked_snapshot(source: Path, destination: Path) -> None:
    paths = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=source,
        check=True,
        capture_output=True,
    ).stdout.split(b"\0")
    for raw in paths:
        if not raw:
            continue
        relative = Path(os.fsdecode(raw))
        src = source / relative
        if not src.is_file():
            continue
        dst = destination / relative
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)


def config_text(codegraph: bool, reasoning_effort: str) -> str:
    return f'''[model]
backend = "codex-cli"
model = "gpt-5.6-luna"
reasoning_effort = "{reasoning_effort}"
timeout_ms = 180000
temperature = 0.0

[explorer]
max_turns = 6
timeout_seconds = 0
max_tool_calls = 40
tool_timeout_seconds = 30
concurrency = 8
codegraph = {str(codegraph).lower()}

[notifications]
update_available = false
'''


def make_placebo(directory: Path) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    executable = directory / "codegraph"
    log = directory / "calls.log"
    executable.write_text(
        "#!/bin/sh\n"
        f"printf '%s\\n' \"$*\" >> {shlex.quote(str(log))}\n"
        "case \"$1\" in\n"
        "  explore) echo 'benchmark placebo: graph evidence unavailable; continue with native tools' >&2; exit 1 ;;\n"
        "  init) mkdir -p \"$2/.codegraph\"; : > \"$2/.codegraph/codegraph.db\" ;;\n"
        "esac\n"
    )
    executable.chmod(0o755)
    return executable


def make_logged_codegraph(directory: Path, real: str) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    executable = directory / "codegraph"
    log = directory / "calls.log"
    executable.write_text(
        "#!/bin/sh\n"
        f"printf '%s\\n' \"$*\" >> {shlex.quote(str(log))}\n"
        f"exec {shlex.quote(real)} \"$@\"\n"
    )
    executable.chmod(0o755)
    return executable


def prepare_repositories(
    phase: str, repeats: int, arms: tuple[str, ...], codegraph: str | None, source: Path
) -> tuple[dict, list]:
    phase_root = PRIVATE / phase
    repos = {}
    index_metrics = []
    for repeat in range(repeats):
        for arm in arms:
            repo = phase_root / "repos" / f"r{repeat}-{arm}"
            if repo.exists():
                shutil.rmtree(repo)
            if (source / ".git").exists():
                tracked_snapshot(source, repo)
            else:
                shutil.copytree(source, repo)
            repos[(repeat, arm)] = repo
            if arm == "placebo":
                index = repo / ".codegraph/codegraph.db"
                index.parent.mkdir(parents=True)
                index.write_text("placebo\n")
            elif arm == "codegraph":
                started = time.monotonic()
                process = subprocess.run(
                    [codegraph, "init", str(repo)],
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    text=True,
                )
                metric = {
                    "repeat": repeat,
                    "operation": "efficacy_warmup_init",
                    "exit_code": process.returncode,
                    "wall_seconds": round(time.monotonic() - started, 3),
                    "stderr": process.stderr[-2048:],
                }
                index_metrics.append(metric)
                if process.returncode:
                    raise RuntimeError(f"CodeGraph warmup failed: {metric}")
    return repos, index_metrics


def citation_metrics(repo: Path, expected_paths: list[str], citations: list[dict]) -> dict:
    cited = {citation.get("path", "") for citation in citations}
    valid = 0
    for citation in citations:
        path = repo / citation.get("path", "")
        try:
            line_count = sum(1 for _ in path.open(errors="replace"))
        except (OSError, ValueError):
            continue
        start = citation.get("start_line", 0)
        end = citation.get("end_line", 0)
        if 1 <= start <= end <= line_count:
            valid += 1
    return {
        "expected_path_recall": round(
            len(cited.intersection(expected_paths)) / len(expected_paths), 4
        ),
        "valid_citation_rate": round(valid / len(citations), 4) if citations else 0.0,
    }


def run_trial(job: dict, tool_bins: dict[str, Path], lock: threading.Lock) -> dict:
    trial = PRIVATE / job["phase"] / "trials" / job["id"]
    result_path = trial / "result.json"
    if result_path.is_file():
        return json.loads(result_path.read_text())
    trial.mkdir(parents=True, exist_ok=True)
    config = trial / "config.toml"
    config.write_text(config_text(job["arm"] in {"placebo", "codegraph"}, job["reasoning_effort"]))
    command = [
        str(job["binary"]),
        "--config", str(config),
        "--root", str(job["repo"]),
        "--json",
        "scout",
        job["task"]["query"],
    ]
    env = os.environ.copy()
    env["NO_COLOR"] = "1"
    if job["arm"] in tool_bins:
        tool_bin = tool_bins[job["arm"]]
        env["PATH"] = f"{tool_bin.parent}{os.pathsep}{env.get('PATH', '')}"
    # CodeGraph sync mutates its local cache; serialize only calls sharing one index.
    guard = lock if job["arm"] == "codegraph" else threading.Lock()
    with guard:
        started = time.monotonic()
        process = subprocess.run(
            command,
            cwd=job["repo"],
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
        )
        wall_seconds = round(time.monotonic() - started, 3)
    (trial / "stdout.json").write_text(process.stdout)
    (trial / "stderr.log").write_text(process.stderr)
    try:
        scout = json.loads(process.stdout) if process.stdout.strip() else {}
    except json.JSONDecodeError:
        scout = {}
    citations = scout.get("citations", [])
    result = {
        "id": job["id"],
        "phase": job["phase"],
        "repeat": job["repeat"],
        "task_id": job["task"]["id"],
        "task_group": job["task"]["group"],
        "arm": job["arm"],
        "exit_code": process.returncode,
        "wall_seconds": wall_seconds,
        "stdout_bytes": len(process.stdout.encode()),
        "stderr_bytes": len(process.stderr.encode()),
        "summary": scout.get("summary", ""),
        "citations": citations,
        "stats": scout.get("stats", {}),
        **citation_metrics(job["repo"], job["task"]["expected_paths"], citations),
    }
    result_path.write_text(json.dumps(result, indent=2) + "\n")
    (trial / "command.json").write_text(
        json.dumps({"argv": command, "cwd": str(job["repo"]), "secret_material": "not retained"}, indent=2) + "\n"
    )
    return result


def blind(results: list[dict], tasks: dict, seed: int, phase: str) -> None:
    rng = random.Random(seed + 991)
    order = list(results)
    rng.shuffle(order)
    review = []
    key = {}
    for index, result in enumerate(order, 1):
        label = f"S{index:03d}"
        key[label] = result["id"]
        repo_prefix = PRIVATE / phase / "repos" / f"r{result['repeat']}-{result['arm']}"
        summary = result["summary"].replace(f"{repo_prefix}/", "")
        review.append({
            "label": label,
            "task_id": result["task_id"],
            "query": tasks[result["task_id"]]["query"],
            "summary": summary,
            "citations": result["citations"],
        })
    root = PRIVATE / phase
    (root / "blind-review.json").write_text(json.dumps(review, indent=2) + "\n")
    (root / "blind-key.json").write_text(json.dumps(key, indent=2) + "\n")


def main() -> None:
    global PRIVATE, PROTOCOL
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", required=True)
    parser.add_argument("--study-dir", type=Path, default=STUDY)
    parser.add_argument("--protocol", type=Path)
    parser.add_argument("--repeats", type=int, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--candidate-binary", type=Path)
    parser.add_argument("--source", type=Path, default=ROOT)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--arms", default="control,placebo,codegraph")
    parser.add_argument("--reasoning-effort", choices=("low", "medium", "high"), default="medium")
    parser.add_argument("--seed", type=int)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.candidate_binary = args.candidate_binary.resolve() if args.candidate_binary else None
    args.source = args.source.resolve()
    args.study_dir = args.study_dir.resolve()
    PRIVATE = args.study_dir / "private"
    PROTOCOL = (args.protocol or args.study_dir / "protocol.json").resolve()
    arms = tuple(dict.fromkeys(args.arms.split(",")))
    allowed_arms = {"control", "placebo", "codegraph", "candidate", "low", "medium", "high"}
    if not arms or any(arm not in allowed_arms for arm in arms):
        raise SystemExit(f"--arms must contain only: {', '.join(sorted(allowed_arms))}")
    protocol = json.loads(PROTOCOL.read_text())
    codex = shutil.which("codex")
    codegraph = shutil.which("codegraph") if {"placebo", "codegraph"} & set(arms) else None
    if not codex or not args.binary.is_file():
        raise SystemExit("missing codex or control repotracer binary")
    if "candidate" in arms and (not args.candidate_binary or not args.candidate_binary.is_file()):
        raise SystemExit("candidate arm requires --candidate-binary")
    if {"placebo", "codegraph"} & set(arms) and not codegraph:
        raise SystemExit("placebo and codegraph arms require codegraph")
    if not args.source.is_dir():
        raise SystemExit("benchmark source must be a directory")
    phase_root = PRIVATE / args.phase
    phase_root.mkdir(parents=True, exist_ok=True)
    tool_bins = {}
    if "placebo" in arms:
        tool_bins["placebo"] = make_placebo(phase_root / "placebo-bin")
    if "codegraph" in arms:
        tool_bins["codegraph"] = make_logged_codegraph(phase_root / "treatment-bin", codegraph)
    repos, index_metrics = prepare_repositories(args.phase, args.repeats, arms, codegraph, args.source)
    jobs = []
    for repeat in range(args.repeats):
        for task in protocol["tasks"]:
            for arm in arms:
                jobs.append({
                    "id": f"r{repeat}-{task['id']}-{arm}",
                    "phase": args.phase,
                    "repeat": repeat,
                    "task": task,
                    "arm": arm,
                    "repo": repos[(repeat, arm)],
                    "reasoning_effort": arm if arm in {"low", "medium", "high"} else args.reasoning_effort,
                    "binary": args.candidate_binary if arm == "candidate" else args.binary,
                })
    seed = args.seed or protocol["execution"]["randomization_seed"]
    random.Random(seed).shuffle(jobs)
    locks = {repeat: threading.Lock() for repeat in range(args.repeats)}
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = [
            pool.submit(run_trial, job, tool_bins, locks[job["repeat"]])
            for job in jobs
        ]
        results = []
        for future in concurrent.futures.as_completed(futures):
            result = future.result()
            results.append(result)
            print(json.dumps({key: result[key] for key in ("id", "exit_code", "wall_seconds")}), flush=True)
    results.sort(key=lambda result: result["id"])
    (phase_root / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    (phase_root / "index-warmup.json").write_text(json.dumps(index_metrics, indent=2) + "\n")
    tasks = {task["id"]: task for task in protocol["tasks"]}
    blind(results, tasks, seed, args.phase)
    metadata = {
        "phase": args.phase,
        "repeats": args.repeats,
        "workers": args.workers,
        "randomization_seed": seed,
        "arms": arms,
        "reasoning_effort": args.reasoning_effort,
        "reasoning_efforts_by_arm": {
            arm: (arm if arm in {"low", "medium", "high"} else args.reasoning_effort)
            for arm in arms
        },
        "study_dir": str(args.study_dir),
        "protocol": str(PROTOCOL),
        "source": str(args.source),
        "binary": str(args.binary),
        "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        "candidate_binary": str(args.candidate_binary) if args.candidate_binary else None,
        "candidate_binary_sha256": (
            hashlib.sha256(args.candidate_binary.read_bytes()).hexdigest()
            if args.candidate_binary else None
        ),
        "codegraph": codegraph,
        "codegraph_version": (
            subprocess.run([codegraph, "--version"], capture_output=True, text=True).stdout.strip()
            if codegraph else None
        ),
        "secret_material": "not retained",
    }
    (phase_root / "run.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()
