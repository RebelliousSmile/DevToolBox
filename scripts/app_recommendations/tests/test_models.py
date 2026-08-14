from __future__ import annotations

import unittest

from scripts.app_recommendations.models import (
    Candidate,
    CommandSuggestion,
    Protection,
    RecommendationReport,
    SizeEvidence,
)


class ModelTests(unittest.TestCase):
    def test_schema_is_explicit_and_protected_candidate_loses_command(self):
        candidate = Candidate(
            app_id="apt:core",
            source="apt",
            name="Core",
            protection=Protection(True, ["system"]),
            command=CommandSuggestion("remove core"),
        )
        report = RecommendationReport("2026-01-01T00:00:00+00:00", "linux", [candidate])
        payload = report.to_dict()
        self.assertEqual(payload["schema_version"], 1)
        self.assertIsNone(payload["candidates"][0]["command"])

    def test_source_prefix_is_required(self):
        with self.assertRaises(ValueError):
            Candidate(app_id="wrong:id", source="apt", name="App")

    def test_negative_sizes_are_rejected(self):
        with self.assertRaises(ValueError):
            SizeEvidence(installed_bytes=-1)


if __name__ == "__main__":
    unittest.main()
