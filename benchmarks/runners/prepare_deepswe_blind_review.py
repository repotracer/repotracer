#!/usr/bin/env python3
import json
from pathlib import Path
import random
import shutil

ROOT = Path(__file__).resolve().parents[2]
PRIVATE = ROOT / "benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning/private"
SEED = 20260839


def main() -> None:
    trials = sorted(path.parent for path in (PRIVATE / "trials").glob("*/result.json"))
    random.Random(SEED).shuffle(trials)
    blind = PRIVATE / "blind"
    if blind.exists():
        shutil.rmtree(blind)
    (blind / "cases").mkdir(parents=True)
    key = {}
    for index, trial in enumerate(trials, 1):
        alias = f"case-{index:02d}"
        key[alias] = trial.name
        case = blind / "cases" / alias
        case.mkdir()
        for source, target in (("prompt.txt", "prompt.txt"), ("model.patch", "model.patch"), ("final.md", "final.md")):
            path = trial / source
            if path.exists():
                shutil.copy2(path, case / target)
        result = json.loads((trial / "result.json").read_text())
        reward = result.get("verifier", {}).get("reward")
        (case / "reward.json").write_text(json.dumps(reward, indent=2) + "\n")
        events = []
        for line in (trial / "trajectory.jsonl").read_text(errors="replace").splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            item = event.get("item", {})
            if item.get("type") not in {"agent_message", "mcp_tool_call"}:
                continue
            scrubbed = json.loads(json.dumps(event).replace(str(trial), "<trial>"))
            events.append(scrubbed)
        (case / "review-events.json").write_text(json.dumps(events, indent=2) + "\n")
    (blind / "key.json").write_text(json.dumps(key, indent=2) + "\n")


if __name__ == "__main__":
    main()
