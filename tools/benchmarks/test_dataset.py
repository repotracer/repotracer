import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import dataset


class DatasetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.row = {"repo": "org/repo", "base_commit": "a" * 40,
                    "instance_id": "task-1", "problem_statement": "Fix this exact problem.",
                    "patch": "SECRET GOLD PATCH", "test_patch": "hidden tests",
                    "FAIL_TO_PASS": '["test_1"]', "PASS_TO_PASS": '[]'}
        self.path = self.root / "tasks.json"
        self.path.write_text(json.dumps([self.row]))

    def test_local_preserves_prompt_but_excludes_gold(self):
        result = dataset.resolve_task({"id": "external-1", "dataset_path": str(self.path)}, self.root, 42)
        self.assertEqual(result["prompt"], self.row["problem_statement"])
        self.assertNotIn("SECRET GOLD PATCH", json.dumps(result))
        self.assertFalse(result["apply_allowed"])
        self.assertEqual(result["acceptance_tests"]["FAIL_TO_PASS"], ["test_1"])

    def test_random_selection_and_specific_instance(self):
        result = dataset.resolve_task({"id": "task", "dataset_path": str(self.path), "instance_id": "task-1"}, self.root, 42)
        self.assertEqual(result["instance_id"], "task-1")
        with self.assertRaises(ValueError):
            dataset.resolve_task({"id": "task", "dataset_path": str(self.path), "instance_id": "absent"}, self.root, 42)

    def test_repository_and_revision_validated(self):
        for field, bad in (("repo", "--config=bad"), ("base_commit", "main")):
            row = dict(self.row, **{field: bad})
            self.path.write_text(json.dumps([row]))
            with self.assertRaises(ValueError):
                dataset.resolve_task({"id": "task", "dataset_path": str(self.path)}, self.root, 42)

    def test_remote_records_revision_and_caches(self):
        metadata = {"sha": "a" * 40}
        page = {"rows": [{"row": self.row, "truncated_cells": []}], "partial": False, "num_rows_total": 1}
        with patch.object(dataset, "fetch_json", side_effect=[metadata, page, metadata]) as fetch:
            rows, revision = dataset.load_remote(self.root)
            self.assertEqual(fetch.call_count, 3)
            self.assertEqual(revision, "a" * 40)
            self.assertNotIn("SECRET GOLD PATCH", json.dumps(rows))
            self.assertNotIn("SECRET GOLD PATCH", next(self.root.glob("swebench-*.json")).read_text())
        with patch.object(dataset, "fetch_json", return_value=metadata) as fetch:
            self.assertEqual(dataset.load_remote(self.root)[0], rows)
            self.assertEqual(fetch.call_count, 1)

    def test_truncated_remote_task_refused(self):
        with patch.object(dataset, "fetch_json", side_effect=[{"sha": "a" * 40}, {
            "rows": [{"row": self.row, "truncated_cells": ["patch"]}], "partial": False}]):
            with self.assertRaisesRegex(ValueError, "incomplete"):
                dataset.load_remote(self.root)


if __name__ == "__main__":
    unittest.main()
