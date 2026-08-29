#!/usr/bin/env python3
import json
from pathlib import Path
import sqlite3
from statistics import mean, median

ROOT = Path(__file__).resolve().parents[2]
STUDY = ROOT / "benchmarks/results/runs/2026-08-28-deepswe-hard-reasoning"
PRIVATE = STUDY / "private"
BILLING_DB = Path.home() / ".codex-lb/store.db"


def summarize(rows: list[dict]) -> dict:
    rewards = [row["verifier"]["reward"] for row in rows if row.get("verifier", {}).get("reward")]
    costs = [row["billing"]["cost_usd"] for row in rows]
    return {
        "trials": len(rows),
        "verifier_passes": sum(reward["reward"] for reward in rewards),
        "mean_f2p": mean(reward["f2p"] for reward in rewards) if rewards else None,
        "mean_p2p": mean(reward["p2p"] for reward in rewards) if rewards else None,
        "timeouts": sum(row["timed_out"] for row in rows),
        "nonzero_exits": sum(row["exit_code"] != 0 for row in rows),
        "scout_call_rate": mean(row["scout_calls"] > 0 for row in rows),
        "median_scout_calls": median(row["scout_calls"] for row in rows),
        "median_wall_seconds": median(row["wall_seconds"] for row in rows),
        "median_solver_cost_usd": median(costs),
        "total_solver_cost_usd": sum(costs),
        "median_solver_requests": median(row["billing"]["requests"] for row in rows),
        "median_reasoning_tokens": median(row["billing"]["reasoning_tokens"] for row in rows),
        "median_input_tokens": median(row["billing"]["input_tokens"] for row in rows),
        "median_cached_input_tokens": median(row["billing"]["cached_input_tokens"] for row in rows),
        "median_output_tokens": median(row["billing"]["output_tokens"] for row in rows),
        "median_patch_bytes": median(row["patch_bytes"] for row in rows),
    }

def thread_id(trial_id: str) -> str:
    first = (PRIVATE / "trials" / trial_id / "trajectory.jsonl").open().readline()
    return json.loads(first)["thread_id"]


def scout_billing(rows: list[dict]) -> dict:
    ids = [thread_id(row["id"]) for row in rows]
    placeholders = ",".join("?" for _ in ids)
    with sqlite3.connect(f"file:{BILLING_DB}?mode=ro", uri=True) as connection:
        start, end = connection.execute(
            f"SELECT MIN(requested_at), MAX(requested_at) FROM request_logs WHERE conversation_id IN ({placeholders})",
            ids,
        ).fetchone()
        query_start, query_end = connection.execute(
            "SELECT datetime(?, '-5 minutes'), datetime(?, '+5 minutes')", (start, end)
        ).fetchone()
        records = connection.execute(
            """SELECT reasoning_effort, COUNT(*), COALESCE(SUM(input_tokens), 0),
                      COALESCE(SUM(cached_input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                      COALESCE(SUM(reasoning_tokens), 0), COALESCE(SUM(cost_usd), 0)
               FROM request_logs
               WHERE model = 'gpt-5.6-luna' AND requested_at BETWEEN ? AND ? AND deleted_at IS NULL
               GROUP BY reasoning_effort""",
            (query_start, query_end),
        ).fetchall()
    keys = ["requests", "input_tokens", "cached_input_tokens", "output_tokens", "reasoning_tokens", "cost_usd"]
    return {
        "source": "codex-lb request logs for Luna inside the parent trial request envelope",
        "window": {"parent_start": start, "parent_end": end, "query_start": query_start, "query_end": query_end},
        "by_reasoning_effort": {effort: dict(zip(keys, values)) for effort, *values in records},
        "assignment_limit": "Concurrent Scout requests can be assigned to reasoning arm, not reliably to one task or trial.",
    }



def main() -> None:
    rows = [json.loads(path.read_text()) for path in sorted((PRIVATE / "trials").glob("*/result.json"))]
    groups = {}
    for task in sorted({row["task_id"] for row in rows}):
        groups[task] = {}
        for arm in ("medium", "high"):
            groups[task][arm] = summarize([row for row in rows if row["task_id"] == task and row["arm"] == arm])
    key = json.loads((PRIVATE / "blind/key.json").read_text())
    reviews = json.loads((PRIVATE / "blind/reviews.json").read_text())["cases"]
    by_id = {row["id"]: row for row in rows}
    for review in reviews:
        review["trial_id"] = key[review["case_id"]]
        trial = by_id[review["trial_id"]]
        review["task_id"] = trial["task_id"]
        review["arm"] = trial["arm"]
        review["repeat"] = trial["repeat"]
    review_groups = {}
    for task in groups:
        review_groups[task] = {}
        for arm in ("medium", "high"):
            selected = [review for review in reviews if review["task_id"] == task and review["arm"] == arm]
            review_groups[task][arm] = {
                "mean_quality_score": mean(review["quality_score"] for review in selected),
                "score_counts": {str(score): sum(review["quality_score"] == score for review in selected) for score in range(5)},
                "strong_scout_evidence_use": sum(review["scout_evidence_use"] == "strong" for review in selected),
                "mixed_or_scout_attribution": sum(review["outcome_attribution"] in {"mixed", "scout_evidence"} for review in selected),
            }
    pairs = []
    for task in sorted({row["task_id"] for row in rows}):
        for repeat in range(3):
            pair = {row["arm"]: row for row in rows if row["task_id"] == task and row["repeat"] == repeat}
            if set(pair) != {"medium", "high"}:
                continue
            medium_reward = pair["medium"].get("verifier", {}).get("reward") or {}
            high_reward = pair["high"].get("verifier", {}).get("reward") or {}
            pairs.append({
                "task_id": task,
                "repeat": repeat,
                "medium_reward": medium_reward.get("reward"),
                "high_reward": high_reward.get("reward"),
                "f2p_delta": (
                    high_reward.get("f2p", 0) - medium_reward.get("f2p", 0)
                    if medium_reward and high_reward else None
                ),
                "p2p_delta": (
                    high_reward.get("p2p", 0) - medium_reward.get("p2p", 0)
                    if medium_reward and high_reward else None
                ),
                "wall_delta_seconds": pair["high"]["wall_seconds"] - pair["medium"]["wall_seconds"],
                "solver_cost_delta_usd": pair["high"]["billing"]["cost_usd"] - pair["medium"]["billing"]["cost_usd"],
                "medium_scout_calls": pair["medium"]["scout_calls"],
                "high_scout_calls": pair["high"]["scout_calls"],
            })
    solver_cost = sum(row["billing"]["cost_usd"] for row in rows)
    scouts = scout_billing(rows)
    scout_cost = sum(arm["cost_usd"] for arm in scouts["by_reasoning_effort"].values())
    output = {
        "status": "completed",
        "decision": {
            "selected_scout_reasoning": "medium",
            "adaptive_high": False,
            "reason": "High produced one additional pass on recursive delegation but did not rescue two of three repeats on either task; Boa pass counts tied 1/3 with opposite repeat winners. Blind review attributed failed runs to parent execution despite generally strong Scout evidence.",
        },
        "groups": groups,
        "pairs": pairs,
        "blind_review": {
            "arm_hidden_during_scoring": True,
            "groups": review_groups,
            "cases": reviews,
        },
        "economics": {
            "solver_cost_usd": solver_cost,
            "scout": scouts,
            "complete_study_cost_usd": solver_cost + scout_cost,
        },
        "limitations": [
            "Two repositories and three repeats per arm; enough to apply the preregistered gate, not to estimate general pass rates.",
            "One reviewer per blinded case; case sets used the same rubric but no overlapping inter-rater sample.",
            "Concurrent Scout requests are assigned to reasoning arm by model effort and benchmark time window, not to an individual task or trial.",
            "The complete-task solver cost includes every session retained under the isolated trial home; Scout cost comes separately from Luna request logs.",
        ],
    }
    (STUDY / "summary.json").write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps(output, indent=2))


if __name__ == "__main__":
    main()
