from __future__ import annotations

import unittest

from scripts.app_recommendations.models import Candidate, Protection, SizeEvidence, UsageEvidence
from scripts.app_recommendations.scoring import GIB, MIB, inactivity_points, score_candidate, size_points


class ScoringTests(unittest.TestCase):
    def test_size_boundaries(self):
        cases = [
            (None, 0),
            (250 * MIB - 1, 0),
            (250 * MIB, 10),
            (GIB, 25),
            (5 * GIB, 35),
            (10 * GIB, 50),
        ]
        for value, expected in cases:
            with self.subTest(value=value):
                self.assertEqual(size_points(value), expected)

    def test_inactivity_boundaries(self):
        cases = [(None, 0), (29, 0), (30, 10), (90, 25), (180, 40), (365, 50)]
        for value, expected in cases:
            with self.subTest(value=value):
                self.assertEqual(inactivity_points(value), expected)

    def test_unknown_usage_never_adds_inactivity(self):
        candidate = Candidate(
            app_id="apt:large",
            source="apt",
            name="Large",
            size=SizeEvidence(10 * GIB, confidence="high"),
            usage=UsageEvidence(kind="unknown", covered_days=400),
        )
        scored = score_candidate(candidate)
        self.assertEqual(scored.score, 50)
        self.assertIn("Usage inconnu", scored.reasons[-1])

    def test_protection_overrides_score(self):
        candidate = Candidate(
            app_id="apt:protected",
            source="apt",
            name="Protected",
            size=SizeEvidence(10 * GIB, confidence="high"),
            usage=UsageEvidence(kind="not_observed", covered_days=365, confidence="medium"),
            protection=Protection(True, ["runtime"]),
        )
        self.assertEqual(score_candidate(candidate).score, 0)


if __name__ == "__main__":
    unittest.main()
