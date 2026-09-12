#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import sys
import unittest


SCRIPT = Path(__file__).with_name("subscription-smoke.py")
SPEC = importlib.util.spec_from_file_location("subscription_smoke", SCRIPT)
SUBSCRIPTION_SMOKE = importlib.util.module_from_spec(SPEC)
sys.dont_write_bytecode = True
SPEC.loader.exec_module(SUBSCRIPTION_SMOKE)


class HandoffReportTests(unittest.TestCase):
    def test_legacy_reports_remain_readable(self):
        for structured in [
            {"investigation": {"findings": [{"answer": "legacy report"}]}},
            {"handoff_version": 2, "report_ref": {
                "content_index": 0, "start_byte": 0, "end_byte": 13}},
            {"handoff_version": 3, "report": "legacy report",
             "investigation": {"status": "complete"}},
        ]:
            with self.subTest(version=structured.get("handoff_version", 1)):
                result = {"content": [{"type": "text", "text": "legacy report"}],
                          "structuredContent": structured}
                self.assertEqual(SUBSCRIPTION_SMOKE.handoff_report(result), "legacy report")

    def test_v4_experiment_report_needs_no_legacy_investigation_or_citations(self):
        result = {
            "content": [{"type": "text", "text": "Command: exit 0\nObserved: success"}],
            "structuredContent": {
                "handoff_version": 4,
                "report": "Command: exit 0\nObserved: success",
                "citations": [],
                "evidence": [],
            },
        }

        self.assertEqual(
            SUBSCRIPTION_SMOKE.handoff_report(result),
            "Command: exit 0\nObserved: success",
        )


if __name__ == "__main__":
    unittest.main()
