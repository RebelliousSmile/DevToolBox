from __future__ import annotations

import io
import json
import unittest
from contextlib import redirect_stderr, redirect_stdout
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

from scripts.app_recommendations.__main__ import build_parser, main
from scripts.app_recommendations.history import AppHistory, UsageHistory
from scripts.app_recommendations.models import Candidate, CommandSuggestion, SizeEvidence
from scripts.app_recommendations.report import build_report


FIXTURE = Path(__file__).parent / "fixtures" / "contract_report_v1.json"
GENERATED_AT = datetime(2026, 4, 2, tzinfo=timezone.utc)
TRACKED_SINCE = datetime(2026, 1, 1, tzinfo=timezone.utc)


def contract_candidate() -> Candidate:
    return Candidate(
        app_id="fixture:editor",
        source="fixture",
        name="Fixture Editor",
        size=SizeEvidence(
            installed_bytes=2 * 1024**3,
            method="fixture_directory",
            scope="fixture hors données utilisateur",
            confidence="high",
        ),
        executable_hints=["/opt/fixture/editor"],
        command=CommandSuggestion("fixture-manager uninstall editor"),
    )


def contract_history() -> UsageHistory:
    coverage = {
        date(2026, 1, 1) + timedelta(days=offset): 1 for offset in range(1, 91)
    }
    return UsageHistory(
        apps={"fixture:editor": AppHistory(TRACKED_SINCE)},
        coverage=coverage,
    )


class CliContractTests(unittest.TestCase):
    def test_success_writes_only_json_to_stdout(self):
        stdout = io.StringIO()
        stderr = io.StringIO()
        with patch(
            "scripts.app_recommendations.__main__.default_collectors",
            return_value={"fixture": lambda: [contract_candidate()]},
        ), redirect_stdout(stdout), redirect_stderr(stderr):
            status = main(["--json"])

        self.assertEqual(status, 0)
        self.assertEqual(stderr.getvalue(), "")
        payload = json.loads(stdout.getvalue())
        self.assertEqual(payload["schema_version"], 1)
        self.assertEqual(payload["candidates"][0]["app_id"], "fixture:editor")

    def test_missing_json_mode_is_a_stderr_diagnostic(self):
        stdout = io.StringIO()
        stderr = io.StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            status = main([])
        self.assertEqual(status, 2)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("--json", stderr.getvalue())

    def test_cli_has_no_mutating_option(self):
        options = {
            option
            for action in build_parser()._actions
            for option in action.option_strings
        }
        self.assertEqual(options, {"-h", "--help", "--json", "--history"})

    def test_shared_contract_fixture_is_generated_by_python_model(self):
        report = build_report(
            {"fixture": lambda: [contract_candidate()]},
            contract_history(),
            generated_at=GENERATED_AT,
            platform_name="contract-fixture",
        )
        expected = json.loads(FIXTURE.read_text(encoding="utf-8"))
        self.assertEqual(report.to_dict(), expected)


if __name__ == "__main__":
    unittest.main()
