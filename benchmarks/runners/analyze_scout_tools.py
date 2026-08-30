#!/usr/bin/env python3
import argparse
from collections import defaultdict
import json
from pathlib import Path
import random
import statistics

from run_scout_tools import PRIVATE

METRICS = (
    "wall_seconds",
    "prompt_tokens",
    "cached_prompt_tokens",
    "uncached_prompt_tokens",
    "completion_tokens",
    "reasoning_output_tokens",
    "tool_calls",
    "expected_path_recall",
    "valid_citation_rate",
    "manual_quality",
)


def enrich(result: dict, grades: dict[str, float]) -> dict:
    row = dict(result)
    stats = result.get("stats", {})
    for key in (
        "prompt_tokens",
        "cached_prompt_tokens",
        "completion_tokens",
        "reasoning_output_tokens",
        "tool_calls",
    ):
        row[key] = stats.get(key, 0) or 0
    row["uncached_prompt_tokens"] = max(
        0, row["prompt_tokens"] - row["cached_prompt_tokens"]
    )
    if result["id"] in grades:
        row["manual_quality"] = grades[result["id"]]
    return row


def summarize(rows: list[dict]) -> dict:
    summary = {"trials": len(rows), "successful": sum(row["exit_code"] == 0 for row in rows)}
    for metric in METRICS:
        values = [row[metric] for row in rows if metric in row]
        if not values:
            continue
        summary[metric] = {
            "mean": round(statistics.fmean(values), 4),
            "median": round(statistics.median(values), 4),
            "sum": round(sum(values), 4),
        }
    return summary


def bootstrap_delta(rows: list[dict], metric: str, left: str, right: str, seed: int) -> dict | None:
    blocks = defaultdict(dict)
    for row in rows:
        if metric in row:
            blocks[(row["repeat"], row["task_id"])][row["arm"]] = row[metric]
    differences = [
        arms[left] - arms[right]
        for arms in blocks.values()
        if left in arms and right in arms
    ]
    if not differences:
        return None
    rng = random.Random(seed)
    means = sorted(
        statistics.fmean(rng.choice(differences) for _ in differences)
        for _ in range(10000)
    )
    return {
        "matched_blocks": len(differences),
        "mean_delta": round(statistics.fmean(differences), 4),
        "median_delta": round(statistics.median(differences), 4),
        "bootstrap_95_percent_ci": [
            round(means[int(0.025 * len(means))], 4),
            round(means[int(0.975 * len(means))], 4),
        ],
    }


def load_grades(root: Path) -> dict[str, float]:
    grade_path = root / "blind-grades.json"
    if not grade_path.is_file():
        return {}
    key = json.loads((root / "blind-key.json").read_text())
    grades = json.loads(grade_path.read_text())
    return {key[item["label"]]: item["score"] for item in grades}


def tool_activity(root: Path) -> dict:
    activity = {}
    for arm in ("placebo", "treatment"):
        path = root / f"{arm}-bin/calls.log"
        lines = path.read_text().splitlines() if path.is_file() else []
        activity["codegraph" if arm == "treatment" else arm] = {
            "sync_calls": sum(line.startswith("sync ") for line in lines),
            "explore_calls": sum(line.startswith("explore ") for line in lines),
            "init_calls": sum(line.startswith("init ") for line in lines),
        }
    return activity


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", required=True)
    parser.add_argument("--study-dir", type=Path, default=PRIVATE.parent)
    args = parser.parse_args()
    root = args.study_dir.resolve() / "private" / args.phase
    raw = json.loads((root / "results.json").read_text())
    arms = tuple(dict.fromkeys(result["arm"] for result in raw))
    grades = load_grades(root)
    rows = [enrich(result, grades) for result in raw]
    by_arm = {
        arm: summarize([row for row in rows if row["arm"] == arm])
        for arm in arms
    }
    by_group = {
        group: {
            arm: summarize(
                [row for row in rows if row["arm"] == arm and row["task_group"] == group]
            )
            for arm in arms
        }
        for group in ("graph_suitable", "negative_control")
    }
    by_task = {
        task_id: {
            arm: summarize(
                [row for row in rows if row["arm"] == arm and row["task_id"] == task_id]
            )
            for arm in arms
        }
        for task_id in sorted({row["task_id"] for row in rows})
    }
    deltas = {}
    for metric in METRICS:
        for left, right in ((left, right) for left in arms for right in arms if left < right):
            value = bootstrap_delta(rows, metric, left, right, 20260827)
            if value:
                deltas[f"{metric}:{left}-{right}"] = value
    analysis = {
        "phase": args.phase,
        "manual_grades_complete": len(grades) == len(rows),
        "by_arm": by_arm,
        "by_task_group": by_group,
        "by_task": by_task,
        "matched_deltas": deltas,
        "tool_activity": tool_activity(root),
        "usage_note": "Subscription usage is reported as prompt, cached prompt, uncached prompt, completion, and reasoning tokens. No fabricated USD conversion is applied.",
    }
    (root / "analysis.json").write_text(json.dumps(analysis, indent=2) + "\n")
    print(json.dumps(analysis, indent=2))


if __name__ == "__main__":
    main()
