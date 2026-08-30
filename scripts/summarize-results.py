#!/usr/bin/env python3
import argparse
import json
from collections import Counter
from pathlib import Path
from statistics import median


def load(path):
    text = path.read_text()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return [json.loads(line) for line in text.splitlines() if line.strip()]


def records(data):
    if isinstance(data, list):
        return [item for item in data if isinstance(item, dict)]
    if isinstance(data, dict) and data and all(isinstance(item, dict) for item in data.values()):
        return list(data.values())
    return []


def value(record, key):
    current = record
    for part in key.split("."):
        if not isinstance(current, dict) or part not in current:
            return None
        current = current[part]
    return current


def summarize(path):
    data = load(path)
    print(f"\n{path}")
    if isinstance(data, dict) and isinstance(data.get("summary"), dict):
        print(json.dumps(data["summary"], indent=2, sort_keys=True))
        return

    rows = records(data)
    if not rows:
        scalars = (
            {
                key: item
                for key, item in data.items()
                if not isinstance(item, dict)
                and (not isinstance(item, list) or len(item) <= 12)
            }
            if isinstance(data, dict)
            else {}
        )
        print(json.dumps(scalars or data, indent=2, sort_keys=True))
        return

    print(f"records: {len(rows)}")
    for key in ("arm", "status", "exit_code", "timed_out", "correct", "expected_scout", "actual_scout"):
        counts = Counter(str(value(row, key)).lower() for row in rows if value(row, key) is not None)
        if counts:
            print(f"{key}: " + ", ".join(f"{name}={count}" for name, count in sorted(counts.items())))

    for key in ("duration_ms", "wall_seconds", "expected_path_recall", "usage.cost"):
        values = [value(row, key) for row in rows]
        values = [number for number in values if isinstance(number, (int, float)) and not isinstance(number, bool)]
        if values:
            suffix = f", total={sum(values):.4g}" if key.endswith("cost") else ""
            print(f"{key}: median={median(values):.4g}, min={min(values):.4g}, max={max(values):.4g}{suffix}")

    failures = []
    for row in rows:
        failed = row.get("correct") is False or row.get("timed_out") is True
        failed |= isinstance(row.get("exit_code"), int) and row["exit_code"] != 0
        failed |= isinstance(row.get("status"), int) and row["status"] >= 400
        if failed:
            label = row.get("id") or row.get("task_id") or row.get("instance_id")
            if not label:
                label = "/".join(str(row[key]) for key in ("arm", "mode") if key in row) or "unknown"
            repeat = row.get("repeat")
            failures.append(f"{label}{f'/r{repeat}' if repeat is not None else ''}")
    if failures:
        unique = list(dict.fromkeys(failures))
        shown = ", ".join(unique[:12])
        more = f" (+{len(unique) - 12} more)" if len(unique) > 12 else ""
        print(f"failures: {shown}{more}")


parser = argparse.ArgumentParser(description="Print compact summaries of JSON or JSONL benchmark results.")
parser.add_argument("paths", nargs="+", type=Path)
args = parser.parse_args()
for input_path in args.paths:
    summarize(input_path)
