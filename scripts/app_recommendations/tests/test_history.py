from __future__ import annotations

import json
import tempfile
import unittest
from datetime import date, timedelta
from pathlib import Path

from scripts.app_recommendations.history import load_history, usage_for


class HistoryTests(unittest.TestCase):
    def test_absent_and_corrupt_history_are_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing = load_history(Path(tmp) / "missing.json")
            self.assertEqual(missing.apps, {})
            corrupt_path = Path(tmp) / "bad.json"
            corrupt_path.write_text("not json", encoding="utf-8")
            corrupt = load_history(corrupt_path)
            self.assertTrue(corrupt.warnings)

    def test_never_observed_uses_only_covered_days_after_tracking_start(self):
        start = date(2025, 1, 1)
        coverage = {
            (start + timedelta(days=offset)).isoformat(): 10 for offset in range(1, 91)
        }
        payload = {
            "version": 1,
            "apps": {"apt:app": {"tracked_since": "2025-01-01T00:00:00+00:00", "last_seen": None}},
            "coverage": coverage,
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "history.json"
            path.write_text(json.dumps(payload), encoding="utf-8")
            usage = usage_for("apt:app", load_history(path))
        self.assertEqual(usage.kind, "not_observed")
        self.assertEqual(usage.covered_days, 90)
        self.assertIsNone(usage.last_seen)

    def test_invalid_entries_are_ignored_not_fatal(self):
        payload = {
            "version": 1,
            "apps": {"apt:bad": {"tracked_since": "yesterday"}},
            "coverage": {"bad-day": 1},
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "history.json"
            path.write_text(json.dumps(payload), encoding="utf-8")
            history = load_history(path)
        self.assertEqual(history.apps, {})
        self.assertGreaterEqual(len(history.warnings), 2)


if __name__ == "__main__":
    unittest.main()
