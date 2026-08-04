"""Tests de la détection de processus (Phase 2b)."""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import procs  # noqa: E402
from scripts.winclean.procs import (  # noqa: E402
    TASKLIST_COMMAND,
    is_running,
    parse_image_names,
    unknown_is_running,
)

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "tasklist.csv"


def captured() -> str:
    return FIXTURE.read_text(encoding="utf-8")


class TestParsing(unittest.TestCase):
    """Parsé contre une capture réelle de `tasklist /FO CSV /NH`."""

    def test_names_are_lowercased_and_complete(self) -> None:
        names = parse_image_names(captured())
        self.assertIn("cargo.exe", names)
        self.assertIn("code.exe", names)
        self.assertIn("msbuild.exe", names)
        self.assertIn("system idle process", names)
        self.assertEqual(len(names), 10)

    def test_a_name_holding_commas_is_not_split(self) -> None:
        # Le CSV est parsé, pas découpé sur les virgules.
        self.assertIn("un, drole, de, nom.exe", parse_image_names(captured()))

    def test_empty_output_parses_to_an_empty_set(self) -> None:
        self.assertEqual(parse_image_names(""), set())


class TestIsRunning(unittest.TestCase):
    def test_match_is_case_insensitive_and_keeps_the_caller_spelling(self) -> None:
        with mock.patch.object(procs, "_run_tasklist", return_value=captured()):
            matched = is_running(["cargo.exe", "MSBuild.exe"])
        self.assertEqual(matched, {"cargo.exe", "MSBuild.exe"})

    def test_successful_query_matching_nothing_is_an_empty_set(self) -> None:
        with mock.patch.object(procs, "_run_tasklist", return_value=captured()):
            matched = is_running(["absent-winclean.exe"])
        # Deux retours distincts, jamais « falsy » : `set()` répond « demandé,
        # rien trouvé », `None` répond « pas pu demander ».
        self.assertIsNotNone(matched)
        self.assertEqual(matched, set())

    def test_failing_subprocess_is_unknown_not_empty(self) -> None:
        for failure in (
            FileNotFoundError(2, "introuvable"),
            subprocess.TimeoutExpired(cmd="tasklist", timeout=15),
            OSError(13, "refusé"),
        ):
            with self.subTest(failure=type(failure).__name__):
                with mock.patch.object(subprocess, "run", side_effect=failure):
                    matched = is_running(["cargo.exe"])
                self.assertIsNone(matched)

    def test_non_zero_return_code_is_unknown(self) -> None:
        completed = subprocess.CompletedProcess(
            args=list(TASKLIST_COMMAND), returncode=1, stdout="", stderr="refus"
        )
        with mock.patch.object(subprocess, "run", return_value=completed):
            self.assertIsNone(is_running(["cargo.exe"]))

    def test_empty_query_answers_without_spawning_anything(self) -> None:
        with mock.patch.object(subprocess, "run") as spawn:
            self.assertEqual(is_running([]), set())
        spawn.assert_not_called()

    def test_real_call_returns_a_set_or_none_and_never_raises(self) -> None:
        matched = is_running(["winclean-inexistant.exe"])
        self.assertTrue(matched is None or isinstance(matched, set))


class TestUnknownIsRunning(unittest.TestCase):
    def test_unknown_counts_as_running(self) -> None:
        self.assertTrue(unknown_is_running(None))

    def test_empty_set_does_not(self) -> None:
        self.assertFalse(unknown_is_running(set()))

    def test_a_match_does(self) -> None:
        self.assertTrue(unknown_is_running({"cargo.exe"}))


class TestNeverCoercive(unittest.TestCase):
    def test_module_source_holds_no_process_control(self) -> None:
        source = Path(procs.__file__).read_text(encoding="utf-8").lower()
        for forbidden in ("taskkill", "terminateprocess", "openprocess", "runas"):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
