"""Tests de scripts/winclean/mod_linux_system.py (Part 5 Phase 3)."""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import mod_linux_system  # noqa: E402
from scripts.winclean.common import Level  # noqa: E402


def _completed(returncode: int, stdout: str = "", stderr: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(
        args=list(mod_linux_system.JOURNALCTL_VACUUM_COMMAND),
        returncode=returncode,
        stdout=stdout,
        stderr=stderr,
    )


class JournalVacuumCommandsTest(unittest.TestCase):
    def test_vacuum_vector_uses_time_not_size(self) -> None:
        self.assertEqual(
            ("journalctl", f"--vacuum-time={mod_linux_system._VACUUM_RETENTION_DAYS}d"),
            mod_linux_system.JOURNALCTL_VACUUM_COMMAND,
        )

    def test_probe_is_disk_usage_only(self) -> None:
        self.assertEqual(("journalctl", "--disk-usage"), mod_linux_system.JOURNALCTL_DISK_USAGE_COMMAND)


class JournalctlAvailableTest(unittest.TestCase):
    def test_missing_binary_is_unavailable(self) -> None:
        with mock.patch.object(mod_linux_system.shutil, "which", return_value=None):
            self.assertFalse(mod_linux_system.journalctl_available())

    def test_binary_that_refuses_is_unavailable(self) -> None:
        with mock.patch.object(mod_linux_system.shutil, "which", return_value="journalctl"):
            with mock.patch.object(mod_linux_system, "_run", return_value=_completed(1)):
                self.assertFalse(mod_linux_system.journalctl_available())

    def test_unlaunchable_probe_is_unavailable(self) -> None:
        with mock.patch.object(mod_linux_system.shutil, "which", return_value="journalctl"):
            with mock.patch.object(mod_linux_system, "_run", return_value=None):
                self.assertFalse(mod_linux_system.journalctl_available())

    def test_responsive_binary_is_available(self) -> None:
        with mock.patch.object(mod_linux_system.shutil, "which", return_value="journalctl"):
            with mock.patch.object(mod_linux_system, "_run", return_value=_completed(0, "8.0M\n")):
                self.assertTrue(mod_linux_system.journalctl_available())


class DiscoverJournalVacuumTest(unittest.TestCase):
    def test_missing_binary_yields_nothing(self) -> None:
        with mock.patch.object(mod_linux_system, "journalctl_available", return_value=False):
            found = mod_linux_system.discover_journal_vacuum()
        self.assertEqual([], found)

    def test_available_journal_yields_one_unpriced_pathless_candidate(self) -> None:
        with mock.patch.object(mod_linux_system, "journalctl_available", return_value=True):
            found = mod_linux_system.discover_journal_vacuum()
        self.assertEqual(1, len(found))
        candidate = found[0]
        self.assertIsNone(candidate.path)
        self.assertIsNone(candidate.estimated_bytes)
        self.assertEqual(Level.AGGRESSIVE, candidate.level)
        self.assertTrue(candidate.no_undo)
        self.assertIn(str(mod_linux_system._VACUUM_RETENTION_DAYS), candidate.reason)


class CleanJournalVacuumTest(unittest.TestCase):
    def test_clean_reports_none_and_uses_the_fixed_command(self) -> None:
        with mock.patch.object(mod_linux_system, "_run", return_value=_completed(0)) as spy:
            result = mod_linux_system.clean_journal_vacuum()
        spy.assert_called_once_with(mod_linux_system.JOURNALCTL_VACUUM_COMMAND)
        self.assertIsNone(result.freed)
        self.assertIsNone(result.recycled)
        self.assertIsNone(result.failed)
        self.assertEqual("journal-vacuum", result.module)

    def test_clean_ignores_trash_days_style_kwargs_silently(self) -> None:
        """`apply_plan` n'envoie jamais `trash_days` à `clean()` - voir la docstring
        du module. `clean_journal_vacuum` doit rester appelable avec seulement les
        trois arguments qu'`apply_plan` fournit réellement."""
        with mock.patch.object(mod_linux_system, "_run", return_value=_completed(0)):
            result = mod_linux_system.clean_journal_vacuum(candidates=[], recycle=False, yes=True)
        self.assertEqual("journal-vacuum", result.module)

    def test_non_zero_vacuum_raises(self) -> None:
        with mock.patch.object(
            mod_linux_system, "_run", return_value=_completed(1, "", "permission denied")
        ):
            with self.assertRaises(OSError):
                mod_linux_system.clean_journal_vacuum()

    def test_unlaunchable_vacuum_raises(self) -> None:
        with mock.patch.object(mod_linux_system, "_run", return_value=None):
            with self.assertRaises(OSError):
                mod_linux_system.clean_journal_vacuum()


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
