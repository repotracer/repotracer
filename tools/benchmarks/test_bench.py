"""No network, models, credentials, or Rust builds required."""
import copy
import importlib.util
import json
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("bench", Path(__file__).with_name("bench.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def fixture():
    data = bench.read_json(Path(__file__).with_name("template") / "manifest.json")
    data["rate_cards"] = {"test-only": {"model": "test-model", "source": "synthetic unit-test rate",
                                      "uncached_input": 2, "cache_read": .2, "cache_write": 3, "output": 10}}
    for i, candidate in enumerate(data["tasks"][0]["candidates"]):
        candidate.update(status="completed", seconds=10 / (i + 1), usage_complete=True,
                         requests=[{"id": f"request-{i}", "model": "test-model", "effort": "medium", "role": "parent",
                                    "rate_card": "test-only", "tokens": {
                                        "uncached_input": 100, "cache_read": 1000, "cache_write": 10, "output": 20}}])
    return data


def grades(data, scores=(8, 9)):
    return {(data["tasks"][0]["id"], c["id"]): {"score": score, "rationale": "Reviewed code and tests"}
            for c, score in zip(data["tasks"][0]["candidates"], scores)}


class ReportingTests(unittest.TestCase):
    def test_cache_and_child_usage_count_once(self):
        data = fixture()
        c = data["tasks"][0]["candidates"][1]
        c["requests"].append(copy.deepcopy(c["requests"][0]))
        child = copy.deepcopy(c["requests"][0])
        child.update(id="child", role="scout")
        c["requests"].append(child)
        result = bench.usage(c, data["rate_cards"])
        self.assertAlmostEqual(result["cost_usd"], .00126)
        self.assertEqual(result["request_count"], 2)
        self.assertEqual(result["tokens"]["scout"]["cache_read"], 1000)

    def test_conflicting_duplicate_fails(self):
        data = fixture()
        c = data["tasks"][0]["candidates"][0]
        duplicate = copy.deepcopy(c["requests"][0])
        duplicate["tokens"]["output"] += 1
        c["requests"].append(duplicate)
        with self.assertRaisesRegex(ValueError, "Conflicting"):
            bench.usage(c, data["rate_cards"])

    def test_unknown_rate_not_free(self):
        data = fixture()
        data["rate_cards"] = {}
        with self.assertRaises(KeyError):
            bench.report(data, grades(data))

    def test_invalid_tokens_and_nan_fail(self):
        for value in (-1, True, float("nan"), .1):
            data = fixture()
            data["tasks"][0]["candidates"][0]["requests"][0]["tokens"]["output"] = value
            with self.assertRaises(ValueError):
                bench.report(data, grades(data))

    def test_missing_usage_and_time_never_zero(self):
        data = fixture()
        c = data["tasks"][0]["candidates"][1]
        c.update(usage_complete=False, seconds=None, usage_missing_reason="stream lost", time_missing_reason="missing start")
        result = bench.report(data, grades(data))
        self.assertIsNone(result["rows"][1]["cost_usd"])
        self.assertIsNone(result["pairs"][0]["cost_ratio"])
        self.assertIsNone(result["pairs"][0]["time_ratio"])
        self.assertEqual(result["aggregates"][0]["quality"]["n"], 1)
        self.assertEqual(result["aggregates"][0]["cost_ratio"]["n"], 0)

    def test_no_grade_no_winner(self):
        data = fixture()
        result = bench.report(data, {})
        self.assertEqual(result["pairs"][0]["excluded"], ["Missing blind quality grade"])
        self.assertIsNone(result["pairs"][0]["cost_ratio"])

    def test_machine_snapshot_and_parent_mismatch(self):
        for field in ("machine", "snapshot", "parent"):
            data = fixture()
            value = {"model": "another", "effort": "medium"} if field == "parent" else "another"
            data["tasks"][0]["candidates"][1][field] = value
            result = bench.report(data, grades(data))
            self.assertIn(f"Mismatched {field}", result["pairs"][0]["excluded"])

    def test_raw_seconds_retained_but_ratios_aggregated(self):
        data = fixture()
        second = copy.deepcopy(data["tasks"][0])
        second["id"] = "on-vps"
        second["candidates"][0].update(machine="vps", seconds=100)
        second["candidates"][1].update(machine="vps", seconds=80)
        data["tasks"].append(second)
        scores = grades(data)
        scores.update({("on-vps", c["id"]): {"score": 8, "rationale": "review"} for c in second["candidates"]})
        result = bench.report(data, scores)
        self.assertEqual(result["aggregates"][0]["time_ratio"]["median"], .65)
        self.assertEqual(result["rows"][3]["seconds"], 80)
        self.assertEqual(result["aggregates"][0]["time_ratio"]["n"], 2)

    def test_tuned_tasks_and_derivatives_are_regressions(self):
        for derivative in (False, True):
            data = fixture()
            data["development_tasks"] = ["old-task" if derivative else "my-task-1"]
            data["tasks"][0]["derived_from"] = ["old-task"] if derivative else []
            result = bench.report(data, grades(data))
            self.assertEqual(result["aggregates"][0]["configuration"]["purpose"], "regression")

    def test_every_loss_marked_for_diagnosis(self):
        data = fixture()
        c = data["tasks"][0]["candidates"][1]
        c["seconds"] = 50
        c["requests"][0]["tokens"]["output"] = 100
        result = bench.report(data, grades(data, (9, 7)))
        self.assertEqual(result["pairs"][0]["diagnosis_required"], ["cost_ratio", "time_ratio", "quality"])

    def test_failed_and_interrupted_runs_retained(self):
        data = fixture()
        data["tasks"][0]["candidates"][1]["status"] = "failed"
        result = bench.report(data, grades(data, (8, 0)))
        self.assertEqual(result["aggregates"][0]["losses"], 1)
        data["tasks"][0]["candidates"][1]["status"] = "interrupted"
        result = bench.report(data, grades(data))
        self.assertEqual(len(result["rows"]), 2)
        self.assertIn("Unsettled run", result["pairs"][0]["excluded"])

    def test_iqr_and_bootstrap(self):
        result = bench.distribution([1, 2, 3, 4], 2000)
        self.assertEqual(result["median"], 2.5)
        self.assertEqual(result["iqr"], [1.75, 3.25])
        self.assertEqual(result["n"], 4)
        self.assertEqual(result, bench.distribution([1, 2, 3, 4], 2000))
        self.assertNotIn("bootstrap_95_ci", bench.distribution([1], 2000))

    def test_history_rejects_duplicate_reports_and_independent_reruns(self):
        data = fixture()
        first = bench.report(data, grades(data))
        with self.assertRaisesRegex(ValueError, "Duplicate run"):
            bench.history([first, first])
        data["experiment"] = "another-day"
        second = bench.report(data, grades(data))
        with self.assertRaisesRegex(ValueError, "Repeated validation task"):
            bench.history([first, second])

    def test_history_preserves_model_and_regression_groups(self):
        data = fixture()
        first = bench.report(data, grades(data))
        data["experiment"] = "another-day"
        data["tasks"][0]["id"] = "another-task"
        second = bench.report(data, grades(data))
        combined = bench.history([first, second])
        self.assertEqual(combined["aggregates"][0]["time_ratio"]["n"], 2)
        combined = bench.history([first, second], ["my-task-1"])
        self.assertEqual(len(combined["aggregates"]), 2)
        self.assertEqual(combined["pairs"][0]["purpose"], "regression")

    def test_actual_efforts_and_total_tokens_exported(self):
        data = fixture()
        result = bench.report(data, grades(data))
        self.assertEqual(result["rows"][0]["total_tokens"], 1130)
        self.assertEqual(result["rows"][0]["actual_settings"][0]["effort"], "medium")


class BlindingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.data = fixture()
        self.data["redact_test_strings"] = ["/private/run-2", "MODEL_NAME"]
        for c in self.data["tasks"][0]["candidates"]:
            (self.root / c["patch"]).write_text(f"diff --git a/code b/code\n+{c['id']}\n")
            for field in ("acceptance_checks", "agent_tests"):
                bench.write_json(self.root / c[field], [{"name": "test_correctness", "status": "passed",
                                                       "evidence": "/private/run-2: MODEL_NAME test result",
                                                       "model": "SECRET", "cost": 1, "runtime": 50,
                                                       "group": "current", "tokens": 9000}])
        self.manifest = self.root / "manifest.json"
        bench.write_json(self.manifest, self.data)
        self.packet = self.root / "packet"
        self.key = self.root / "key.json"

    def pack(self, seed=2):
        return bench.blind(self.manifest, "my-task-1", self.packet, self.key, random.Random(seed))

    def test_packet_contains_only_grading_inputs(self):
        self.pack()
        self.assertFalse((self.packet / "key.json").exists())
        files = [p for p in self.packet.rglob("*") if p.is_file()]
        self.assertEqual(len(files), 9)
        for p in self.packet.rglob("*.json"):
            content = p.read_text()
            for forbidden in ("SECRET", "MODEL_NAME", "/private/run-2", '"tokens"', '"group"', '"cost"', '"runtime"'):
                self.assertNotIn(forbidden, content)
        key = bench.read_json(self.key)
        self.assertEqual(set(key["labels"].values()), {"run-1", "run-2"})

    def test_labels_are_randomized(self):
        results = set()
        for seed in range(8):
            output, key = self.root / f"packet-{seed}", self.root / f"key-{seed}.json"
            bench.blind(self.manifest, "my-task-1", output, key, random.Random(seed))
            results.add(bench.read_json(key)["labels"]["Candidate A"])
        self.assertEqual(results, {"run-1", "run-2"})

    def test_key_cannot_be_inside_packet(self):
        with self.assertRaisesRegex(ValueError, "outside"):
            bench.blind(self.manifest, "my-task-1", self.packet, self.packet / "key.json")

    def test_no_overwrite(self):
        self.pack()
        with self.assertRaisesRegex(ValueError, "already exists"):
            self.pack()

    def test_grade_join_and_tamper_rejection(self):
        self.pack()
        path = self.root / "grades.json"
        bench.write_json(path, {label: {"score": 8, "rationale": "Evidence reviewed"}
                               for label in bench.read_json(self.key)["labels"]})
        joined = bench.load_grades(self.data, [(self.key, path)])
        self.assertEqual(set(joined), {("my-task-1", "run-1"), ("my-task-1", "run-2")})
        self.assertEqual(joined[("my-task-1", "run-1")]["tests"]["acceptance_checks"]["passed"], 1)
        (self.packet / "task.md").write_text("changed")
        with self.assertRaisesRegex(ValueError, "packet changed"):
            bench.load_grades(self.data, [(self.key, path)])

    def test_partial_grades_rejected(self):
        self.pack()
        path = self.root / "grades.json"
        bench.write_json(path, {"Candidate A": {"score": 9, "rationale": "review"}})
        with self.assertRaisesRegex(ValueError, "exactly"):
            bench.load_grades(self.data, [(self.key, path)])

    def test_adding_unrelated_task_does_not_break_finished_review(self):
        self.pack()
        path = self.root / "grades.json"
        bench.write_json(path, {label: {"score": 8, "rationale": "Evidence reviewed"}
                               for label in bench.read_json(self.key)["labels"]})
        task = copy.deepcopy(self.data["tasks"][0])
        task["id"] = "later-task"
        self.data["tasks"].append(task)
        self.assertEqual(len(bench.load_grades(self.data, [(self.key, path)])), 2)
        self.data["tasks"][0]["prompt"] = "changed prompt"
        with self.assertRaisesRegex(ValueError, "Task changed"):
            bench.load_grades(self.data, [(self.key, path)])

    def test_cli_init_and_incomplete_report(self):
        cli = Path(__file__).with_name("bench.py")
        directory = self.root / "new"
        result = subprocess.run([sys.executable, str(cli), "init", str(directory)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = subprocess.run([sys.executable, str(cli), "report", str(directory / "manifest.json")], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNone(json.loads(result.stdout)["rows"][0]["cost_usd"])

    def test_cli_completed_report_includes_all_metrics(self):
        self.pack()
        path = self.root / "grades.json"
        bench.write_json(path, {label: {"score": 8, "rationale": "Evidence reviewed"}
                               for label in bench.read_json(self.key)["labels"]})
        cli = Path(__file__).with_name("bench.py")
        result = subprocess.run([sys.executable, str(cli), "report", str(self.manifest),
                                 "--review", str(self.key), str(path)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        output = json.loads(result.stdout)
        for row in output["rows"]:
            for field in ("group", "cost_usd", "seconds", "grade", "tokens", "total_tokens"):
                self.assertIsNotNone(row[field])
        self.assertEqual(output["aggregates"][0]["ties"], 1)


if __name__ == "__main__":
    unittest.main()
