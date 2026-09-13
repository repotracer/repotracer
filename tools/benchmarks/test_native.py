"""Synthetic tests for native accounting and command construction."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

import native


CARD = {"card": {"model": "gpt-test", "source": "test",
                 "uncached_input": 1, "cache_read": 1,
                 "cache_write": 1, "output": 1}}


class NativeTests(unittest.TestCase):
    def test_codex_input_is_split_into_disjoint_buckets(self):
        self.assertEqual(
            native.normalize_tokens({
                "input_tokens": 120, "cached_input_tokens": 70,
                "cache_write_input_tokens": 10, "output_tokens": 5}, "codex"),
            {"uncached_input": 40, "cache_read": 70,
             "cache_write": 10, "output": 5})
        self.assertIsNone(native.normalize_tokens({
            "input_tokens": 2, "cached_input_tokens": 3,
            "output_tokens": 1}, "codex"))

    def test_claude_cache_counts_are_already_separate(self):
        self.assertEqual(
            native.normalize_tokens({
                "input_tokens": 12, "cache_read_input_tokens": 70,
                "cache_creation_input_tokens": 10, "output_tokens": 5}, "claude"),
            {"uncached_input": 12, "cache_read": 70,
             "cache_write": 10, "output": 5})

    def test_codex_rollout_deduplicates_resume_replays(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo = root / "repo"
            repo.mkdir()
            rollout = root / "rollout.jsonl"
            events = [
                {"type": "turn_context", "payload": {
                    "cwd": str(repo), "model": "gpt-test",
                    "reasoning_effort": "medium"}},
                {"type": "token_usage_record", "payload": {
                    "response_id": "same", "usage": {
                        "input_tokens": 20, "cached_input_tokens": 10,
                        "cache_write_input_tokens": 0, "output_tokens": 2}}},
            ]
            rollout.write_text("\n".join(json.dumps(row) for row in events + events))
            requests, problems = native.parse_codex_rollouts(
                [rollout], repo, CARD)
            self.assertEqual(len(requests), 1)
            self.assertEqual(requests[0]["tokens"]["uncached_input"], 10)
            self.assertEqual(requests[0]["effort"], "medium")
            self.assertEqual(problems, [])

    def test_claude_stream_repeats_use_max_not_sum(self):
        with tempfile.TemporaryDirectory() as temporary:
            trace = Path(temporary) / "trace.jsonl"
            rows = []
            for output in (1, 9):
                rows.append({"type": "assistant", "request_id": "req",
                             "message": {"id": "message", "model": "gpt-test",
                                         "usage": {"input_tokens": 2,
                                                   "cache_read_input_tokens": 3,
                                                   "cache_creation_input_tokens": 4,
                                                   "output_tokens": output}}})
            rows.append({"type": "result", "total_cost_usd": 0.25,
                         "usage": {"input_tokens": 2,
                                   "cache_read_input_tokens": 3,
                                   "cache_creation_input_tokens": 4,
                                   "output_tokens": 9}})
            trace.write_text("\n".join(json.dumps(row) for row in rows))
            requests, problems, cost = native.parse_claude_events(
                trace, CARD, "medium")
            self.assertEqual(len(requests), 1)
            self.assertEqual(requests[0]["tokens"]["output"], 9)
            self.assertEqual(cost, 0.25)
            self.assertEqual(problems, [])

    def test_missing_scout_response_is_not_complete_or_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / "usage.jsonl"
            log.write_text(json.dumps({"event": "request", "id": "7"}) + "\n")
            requests, problems, cost = native.parse_scout_log(log, CARD)
            self.assertEqual(requests, [])
            self.assertIn("no response", problems[0])
            self.assertIsNone(cost)

    def test_partial_scout_usage_is_never_presented_as_complete_cost(self):
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / "usage.jsonl"
            rows = [
                {"event": "request", "id": "7"},
                {"event": "response", "id": "7", "request": {},
                 "stats": {"model": "gpt-test", "reasoning_effort": "medium",
                           "usage_status": "partial", "reported_cost_usd": 0.1,
                           "usage": {"input_tokens": 4, "cached_input_tokens": 1,
                                     "cache_write_input_tokens": 0, "output_tokens": 1}}},
            ]
            log.write_text("\n".join(json.dumps(row) for row in rows))
            requests, problems, cost = native.parse_scout_log(log, CARD)
            self.assertEqual(len(requests), 1)
            self.assertIn("partial usage", problems[0])
            self.assertIsNone(cost)

    def test_commands_preserve_native_config_and_sessions(self):
        repo = Path("/tmp/project")
        codex = native.codex_command(
            "codex", repo, "exact prompt", "gpt-test", "medium",
            Path("/tmp/final"), False)
        self.assertIn("exact prompt", codex)
        self.assertIn("mcp_servers.repotracer.enabled=false", codex)
        self.assertNotIn("--ignore-user-config", codex)
        self.assertFalse(any("base_url" in value for value in codex))

        claude = native.claude_command(
            "claude", repo, "exact prompt", "opus", None,
            Path("/tmp/final"), False)
        self.assertIn("--strict-mcp-config", claude)
        self.assertNotIn("--no-session-persistence", claude)
        self.assertNotIn("--settings", claude)

        resumed = native.claude_command(
            "claude", repo, "ignored", "opus", "medium",
            Path("/tmp/final"), False, session_id="thread-id")
        self.assertEqual(resumed[-4:], ["--resume", "thread-id", "-p",
                                       "Continue the original task."])

    def test_process_adapter_writes_directly_to_durable_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            trace, errors = root / "trace.jsonl", root / "stderr.log"
            events, pids = [], []
            code = native.NativeAdapter().run(
                [sys.executable, "-c",
                 "import json,sys; print(json.dumps({'type':'done'})); print('warning', file=sys.stderr)"],
                root, trace, errors, events.append, pids.append)
            self.assertEqual(code, 0)
            self.assertEqual(events, [{"type": "done"}])
            self.assertTrue(pids[0] > 0)
            self.assertIn("warning", errors.read_text())

    def test_large_prompt_uses_stdin_without_an_argument_limit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prompt = root / "input.txt"
            prompt.write_text("x" * 200000)
            trace = root / "trace.jsonl"
            code = native.NativeAdapter().run(
                [sys.executable, "-c", "import sys; print(len(sys.stdin.read()))"],
                root, trace, root / "stderr", stdin_path=prompt)
            self.assertEqual(code, 0)
            self.assertEqual(trace.read_text().strip(), "200000")


if __name__ == "__main__":
    unittest.main()
