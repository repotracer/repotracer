"""Check the published pilot records and print aggregates, without model calls."""

import hashlib
import itertools
import json
import re
from pathlib import Path
from statistics import mean


ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads((ROOT / name).read_text())


def require(condition, message):
    if not condition:
        raise ValueError(message)


def main():
    manifest = read("manifest.json")
    rows = read("results.json")
    host_paths = re.compile(r"/Users/|(?:/private)?/var/folders/|repotracer-investigation-scratch-")
    for path in [ROOT / "parent-events.json", *sorted((ROOT / "scouts").glob("*.json"))]:
        require(not host_paths.search(path.read_text()), f"{path.name}: residual host path")
    expected = set(itertools.product(
        manifest["parents"], manifest["arms"],
        (task["id"] for task in manifest["tasks"]),
    ))
    actual = {(row["parent"], row["arm"], row["task"]) for row in rows}
    require(actual == expected and len(rows) == len(expected), "Missing/duplicate cells")
    require(len({row["id"] for row in rows}) == len(rows), "Duplicate trial IDs")
    require(
        [{key: row[key] for key in ("id", "parent", "arm", "task")} for row in rows]
        == manifest["schedule"], "Schedule mismatch",
    )
    for arm in manifest["arms"]:
        data = b"" if arm == "baseline" else (ROOT / "guidance" / f"{arm}.md").read_bytes()
        require(hashlib.sha256(data).hexdigest() == manifest["guidance_sha256"][arm], arm)
        require(len(data.decode().split()) == manifest["guidance_word_counts"][arm], arm)

    for row in rows:
        tid = row["id"]
        patch = read(f"patches/{tid}.json")
        data = patch["patch"].encode()
        require(hashlib.sha256(data).hexdigest() == row["patch_sha256"] == patch["sha256"], tid)
        require(len(data) == row["patch_bytes"] and patch["trial"] == tid, tid)
        grade = read(f"grades/{tid}.json")
        require(grade["reward"] == row["grade"], f"{tid}: reward mismatch")
        for group in ("f2p", "p2p"):
            tests = [t for t in grade["tests"] if t["name"].startswith(f"[{group}]")]
            require(len(tests) == row["grade"][f"{group}_total"], f"{tid}: test count")
            require(sum(t["status"] == "passed" for t in tests)
                    == row["grade"][f"{group}_passed"], f"{tid}: passed count")
        passing = all(row["grade"][f"{g}_passed"] == row["grade"][f"{g}_total"]
                      for g in ("f2p", "p2p"))
        require(passing == bool(row["grade"]["reward"]), f"{tid}: binary reward")
        require(row["termination"] == ("budget_exhausted" if row["timed_out"]
                                       else "normal_exit"), f"{tid}: termination")
        scouts = [] if row["arm"] == "baseline" else read(f"scouts/{tid}.json")
        require(len(scouts) == row["scout_calls"], f"{tid}: scout count")
        require(sum(s["is_error"] for s in scouts) == row["scout_errors"], tid)
        for short, field in (("input", "prompt_tokens"), ("output", "completion_tokens")):
            tokens = sum(s["stats"][field] for s in scouts)
            require(tokens == row[f"scout_{short}_tokens"], f"{tid}: scout tokens")
            if row["parent_usage"] is not None:
                require(row[f"combined_{short}_tokens"]
                        == row["parent_usage"][f"{short}_tokens"] + tokens, tid)

    checks = manifest["calibration"]["pristine_verifier_checks"]
    require(len(checks) == 3, "Missing pristine calibrations")
    for check in checks:
        grade = check["grade"]
        require(grade["f2p_passed"] == 0 and grade["p2p_passed"] == grade["p2p_total"],
                "Pristine calibration failed")

    print(f"Verified {len(rows)} records, patches, grades, scout usage and guidance hashes.")
    for parent in manifest["parents"]:
        for arm in ("long", "short", "baseline"):
            group = [r for r in rows if r["parent"] == parent and r["arm"] == arm]
            passing = sum(r["grade"]["reward"] for r in group)
            inputs = [r["combined_input_tokens"] for r in group]
            outputs = [r["combined_output_tokens"] for r in group]
            print(parent, arm, f"passing={passing}/{len(group)}",
                  f"mean_minutes={mean(r['elapsed_seconds'] for r in group) / 60:.2f}",
                  f"input={sum(inputs) if all(x is not None for x in inputs) else 'unavailable'}",
                  f"output={sum(outputs) if all(x is not None for x in outputs) else 'unavailable'}")


if __name__ == "__main__":
    main()
