"""No model, network, or provider access is used by these workflow tests."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

import workflow


def card(model):
    return {"model": model, "source": "test", "uncached_input": 1,
            "cache_read": 1, "cache_write": 1, "output": 1}


def command(*args, cwd):
    return subprocess.run(args, cwd=cwd, check=True, text=True,
                          capture_output=True)


def repository(path: Path) -> Path:
    path.mkdir()
    command("git", "init", "-q", cwd=path)
    command("git", "config", "user.name", "Test", cwd=path)
    command("git", "config", "user.email", "test@example.invalid", cwd=path)
    (path / "tracked.txt").write_text("original\n")
    command("git", "add", ".", cwd=path)
    command("git", "commit", "-qm", "initial", cwd=path)
    return path


class WorkflowTests(unittest.TestCase):
    def test_init_is_idempotent_and_does_not_overwrite_config(self):
        with tempfile.TemporaryDirectory() as temporary:
            store = workflow.Store(Path(temporary) / "state")
            store.initialize()
            config = workflow.read_json(store.config_path)
            config["seed"] = 42
            workflow.atomic_json(store.config_path, config)
            store.initialize()
            state = store.state()
            self.assertEqual(state["config"]["seed"], 42)
            self.assertEqual(state["runs"], [])

    def test_validation_requires_parent_only_baseline_and_pair(self):
        config = workflow.default_config()
        config["arms"][0]["repotracer"] = True
        with self.assertRaisesRegex(ValueError, "parent-only"):
            workflow.validate_config(config)
        config = workflow.default_config()
        config["arms"][1]["enabled"] = False
        with self.assertRaisesRegex(ValueError, "at least two"):
            workflow.validate_config(config)

    def test_changed_arm_requires_and_resolves_a_distinct_artifact(self):
        config = workflow.default_config()
        changed = next(arm for arm in config["arms"] if arm["name"] == "changed")
        changed["enabled"] = True
        with self.assertRaisesRegex(ValueError, "explicit binary or source_path"):
            workflow.validate_config(config)
        changed["binary"] = "/tmp/changed-repotracer"
        workflow.validate_config(config)

    def test_codex_cost_preflight_requires_both_models(self):
        config = workflow.default_config()
        tasks = [{"id": "task", "origin": "custom", "parent": "codex",
                  "prompt": "do it"}]
        with self.assertRaisesRegex(ValueError, "Codex parent"):
            workflow.validate_cost_preflight(config, tasks)
        config["rate_cards"] = {"parent": card("gpt-5.6-sol")}
        with self.assertRaisesRegex(ValueError, "Codex scout"):
            workflow.validate_cost_preflight(config, tasks)
        config["rate_cards"]["scout"] = card("gpt-5.6-luna")
        workflow.validate_cost_preflight(config, tasks)

    def test_snapshot_captures_dirty_tracked_staged_and_untracked(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = repository(root / "source")
            (source / "tracked.txt").write_text("dirty\n")
            command("git", "add", "tracked.txt", cwd=source)
            (source / "new.txt").write_text("new\n")
            before = workflow.source_fingerprint(source)
            fingerprint, tree = workflow.snapshot_local(
                source, root / "snapshot")
            self.assertEqual(fingerprint, before)
            self.assertEqual((root / "snapshot" / "tracked.txt").read_text(),
                             "dirty\n")
            self.assertEqual((root / "snapshot" / "new.txt").read_text(),
                             "new\n")
            self.assertTrue(tree)
            self.assertEqual((source / "tracked.txt").read_text(), "dirty\n")

    def test_candidate_patch_includes_untracked_and_applies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = repository(root / "source")
            candidate = root / "candidate"
            command("git", "clone", "-q", str(source), str(candidate),
                    cwd=root)
            (candidate / "tracked.txt").write_text("changed\n")
            (candidate / "added.txt").write_text("added\n")
            patch = workflow.candidate_patch(candidate)
            self.assertIn("added.txt", patch)
            clean = root / "clean"
            command("git", "clone", "-q", str(source), str(clean), cwd=root)
            subprocess.run(["git", "apply", "-"], cwd=clean, input=patch,
                           text=True, check=True)
            self.assertEqual((clean / "added.txt").read_text(), "added\n")

    def test_agent_tests_need_an_explicit_exit_code(self):
        with tempfile.TemporaryDirectory() as temporary:
            trace = Path(temporary) / "trace.jsonl"
            rows = [
                {"type": "item.completed", "item": {
                    "type": "command_execution", "command": "pytest -q",
                    "exit_code": 0, "aggregated_output": "2 passed"}},
                {"type": "assistant", "message": {"content": [{
                    "type": "tool_use", "id": "x", "name": "Bash",
                    "input": {"command": "cargo test"}}]}},
                {"type": "assistant", "message": {"content": [{
                    "type": "tool_result", "tool_use_id": "x",
                    "content": "looks fine"}]}},
            ]
            trace.write_text("\n".join(json.dumps(row) for row in rows))
            tests = workflow.parse_agent_tests(trace)
            self.assertEqual([row["status"] for row in tests],
                             ["passed", "not_run"])

    def test_start_is_detached_and_idempotent(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = repository(root / "project")
            store = workflow.Store(root / "state")
            store.initialize()
            config = workflow.read_json(store.config_path)
            config["tasks"] = [{
                "id": "task", "origin": "custom",
                "project_path": str(project), "prompt": "Do it",
                "apply_allowed": False, "parent": "codex",
            }]
            config["rate_cards"] = {
                "parent": card("gpt-5.6-sol"),
                "scout": card("gpt-5.6-luna"),
            }
            workflow.atomic_json(store.config_path, config)
            with mock.patch.object(workflow, "spawn_worker", return_value=12345):
                first = workflow.start(store)
                with mock.patch.object(workflow, "pid_alive", return_value=True):
                    second = workflow.start(store)
            self.assertEqual(first["state"], "queued")
            self.assertEqual(second["id"], first["id"])
            self.assertTrue(second["existing"])


if __name__ == "__main__":
    unittest.main()
