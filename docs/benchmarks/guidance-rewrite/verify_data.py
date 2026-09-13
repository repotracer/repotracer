"""Check the published guidance studies and print aggregates, without model calls."""

import hashlib
import itertools
import json
import math
import re
from pathlib import Path
from statistics import mean


ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads((ROOT / name).read_text())


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify_native_followup():
    data = read("native-claude.json")
    rows = data["conditions"]
    require({row["group"] for row in rows} == {"baseline", "current", "changed"}
            and len(rows) == 3, "Native follow-up: missing/duplicate conditions")
    require(data["task"]["independent_tasks"] == 1
            and data["task"]["solves_per_condition"] == 1, "Native follow-up: sample size")
    require(not data["task"]["influenced_submitted_guidance"], "Native task was used for tuning")
    require(data["runtime"]["conditions_share_binary"]
            and data["runtime"]["conditions_share_snapshot"], "Native conditions differ")
    require(data["runtime"]["whole_task_deadline"] is None, "Unexpected native cutoff")
    proposed = next(row for row in rows if row["group"] == "changed")
    source = (ROOT.parents[2] / "crates/cli/src/agents.rs").read_text()
    body = source.split("pub(crate) const ROUTING_INSTRUCTIONS: &str = concat!(", 1)[1]
    body = body.split("\n);", 1)[0]
    routing = "".join(json.loads(part) for part in re.findall(r'"(?:[^"\\]|\\.)*"', body))
    require(hashlib.sha256(routing.encode()).hexdigest() == proposed["guidance_sha256"],
            "Proposed guidance changed since the native benchmark")
    require(len(routing.split()) == proposed["guidance_words"] == 132,
            "Native proposed guidance word count")
    for row in rows:
        group = row["group"]
        require(row["status"] == "completed" and row["solving_launches"] == 1,
                f"Native {group}: solving status")
        require(row["parent"] == {"model": "claude-opus-5", "effort": "native-default"},
                f"Native {group}: parent settings")
        require(row["usage_complete"], f"Native {group}: incomplete usage")
        for field in ("elapsed_seconds", "cost_usd"):
            value = row[field]
            require(type(value) in (int, float) and math.isfinite(value) and value > 0,
                    f"Native {group}: invalid {field}")
        for field in ("guidance_sha256", "candidate_patch_sha256", "native_trace_sha256"):
            require(re.fullmatch(r"[0-9a-f]{64}", row[field]), f"Native {group}: {field}")
        require(abs(row["cost_usd"] - sum(row["cost_by_role_usd"].values())) < 1e-8,
                f"Native {group}: cost does not reconcile")
        for role in ("parent", "scout"):
            require(set(row["tokens"][role]) == {
                "uncached_input", "cache_read", "cache_write", "output"
            }, f"Native {group}: token buckets")
            require(all(type(value) is int and value >= 0 for value in row["tokens"][role].values()),
                    f"Native {group}: invalid token count")
            require(row["cost_by_role_usd"][role] >= 0, f"Native {group}: negative role cost")
        if row["scout_calls"] == 0:
            require(sum(row["tokens"]["scout"].values()) == 0
                    and row["cost_by_role_usd"]["scout"] == 0, f"Native {group}: unused scout")
        require(row["independent_hidden_assertions"] == "passed", f"Native {group}: acceptance")
        require(0 <= row["grade"]["score"] <= 10 and row["grade"]["rationale"],
                f"Native {group}: grade")
    current = next(row for row in rows if row["group"] == "current")
    baseline = next(row for row in rows if row["group"] == "baseline")
    require(baseline["scout_configured"] is None and baseline["guidance_words"] == 0
            and baseline["guidance_sha256"] == hashlib.sha256(b"").hexdigest(),
            "Native baseline has guidance or scout")
    require(current["guidance_words"] == 349 and current["scout_calls"] == 1
            and proposed["scout_calls"] == 0, "Native routing observations")
    diagnosis = data["diagnosis"]
    require(abs(sum(call["elapsed_seconds"] for call in diagnosis["changed"]["full_suite_calls"])
                - diagnosis["changed"]["full_suite_elapsed_seconds"]) < 1e-8,
            "Native full-suite timing does not reconcile")
    require(abs(diagnosis["current"]["scout_cost_usd"]
                - current["cost_by_role_usd"]["scout"]) < 1e-8, "Native scout cost mismatch")
    grader = data["grader"]
    require(grader["model"] == "gpt-6-astra" and grader["effort"] == "high"
            and grader["blind"] and grader["quality_fixed_before_diagnosis"]
            and grader["tool_calls"] == 0, "Native grader conditions")
    require(grader["cost_usd"] is None and grader["cost_missing_reason"],
            "Native missing grader cost must stay unknown")
    for name in ("native-claude.json", "native-claude.md"):
        require(not re.search(r"/Users/|/home/|(?:/private)?/var/folders/|repotracer-investigation-scratch-|sk-ant-",
                              (ROOT / name).read_text()), f"{name}: private data")
    print("Verified 3 native conditions, exact proposed guidance, costs, token buckets and grading metadata.")
    print(f"Native proposed/current: cost={proposed['cost_usd'] / current['cost_usd']:.4f}, "
          f"time={proposed['elapsed_seconds'] / current['elapsed_seconds']:.4f}, "
          f"quality_delta={proposed['grade']['score'] - current['grade']['score']}")


def main():
    verify_native_followup()
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
