from __future__ import annotations

import json
import time
import unittest
from datetime import datetime, timezone

from scripts.app_recommendations.models import Candidate, SizeEvidence
from scripts.app_recommendations.report import build_report, to_json


NOW = datetime(2026, 1, 1, tzinfo=timezone.utc)


def candidate(size: int = 100) -> Candidate:
    return Candidate("fixture:app", "fixture", "Fixture App", SizeEvidence(size, confidence="high"))


class ReportTests(unittest.TestCase):
    def test_deterministic_json_and_deduplication_without_size_sum(self):
        collectors = {"fixture": lambda: [candidate(100), candidate(200)]}
        first = to_json(build_report(collectors, generated_at=NOW, platform_name="test"))
        second = to_json(build_report(collectors, generated_at=NOW, platform_name="test"))
        self.assertEqual(first, second)
        payload = json.loads(first)
        self.assertEqual(len(payload["candidates"]), 1)
        self.assertEqual(payload["candidates"][0]["size"]["installed_bytes"], 200)

    def test_failure_and_timeout_are_isolated(self):
        def failed():
            raise RuntimeError("boom")

        def slow():
            time.sleep(0.2)
            return []

        report = build_report(
            {"good": lambda: [candidate()], "failed": failed, "slow": slow},
            timeout_seconds=0.01,
            generated_at=NOW,
        )
        self.assertEqual(len(report.candidates), 1)
        self.assertEqual({error.code for error in report.source_errors}, {"collector_error", "timeout"})


if __name__ == "__main__":
    unittest.main()
