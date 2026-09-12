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
    def test_v3_experiment_report_needs_no_legacy_investigation_or_citations(self):
        result = {
            "content": [{"type": "text", "text": "Command: exit 0\nObserved: success"}],
            "structuredContent": {
                "handoff_version": 3,
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
