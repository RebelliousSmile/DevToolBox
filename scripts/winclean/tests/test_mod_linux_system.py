"""Tests de scripts/winclean/mod_linux_system.py (Part 5 Phase 3)."""

from __future__ import annotations

import subprocess
import sys
import tempfile
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


SNAP_LIST = """Name Version Rev Tracking Publisher Notes
discord 1.0.152 302 latest/stable snapcrafters disabled
discord 1.0.153 303 latest/stable snapcrafters -
glab 1.112.0 6431 latest/stable gitlab-cli classic,disabled
code 1.2.3 253 latest/stable vscode classic
"""


class SnapOldRevisionsTest(unittest.TestCase):
    def test_parser_keeps_only_disabled_revisions(self) -> None:
        self.assertEqual(
            [("discord", "302"), ("glab", "6431")],
            mod_linux_system.parse_disabled_snap_revisions(SNAP_LIST),
        )

    def test_discovery_prices_the_snap_files(self) -> None:
        completed = _completed(0, SNAP_LIST)
        with tempfile.TemporaryDirectory() as raw:
            package_dir = Path(raw)
            (package_dir / "discord_302.snap").write_bytes(b"x" * 20)
            (package_dir / "glab_6431.snap").write_bytes(b"x" * 30)
            with mock.patch.object(mod_linux_system.shutil, "which", return_value="/usr/bin/snap"):
                with mock.patch.object(mod_linux_system, "_run_snap", return_value=completed):
                    with mock.patch.object(mod_linux_system, "SNAP_PACKAGE_DIR", package_dir):
                        found = mod_linux_system.discover_snap_old_revisions()
        self.assertEqual([c.resource_id for c in found], ["discord@302", "glab@6431"])
        self.assertEqual([c.estimated_bytes for c in found], [20, 30])
        self.assertTrue(all(c.level is Level.AGGRESSIVE and c.no_undo for c in found))

    def test_clean_uses_revision_and_purge_for_each_candidate(self) -> None:
        candidates = [
            mod_linux_system.CleanCandidate(
                module="snap-old-revisions",
                path=None,
                label="old",
                estimated_bytes=None,
                level=Level.AGGRESSIVE,
                reason="old",
                resource_id="discord@302",
            )
        ]
        with mock.patch.object(mod_linux_system, "_run_snap", return_value=_completed(0)) as run:
            result = mod_linux_system.clean_snap_old_revisions(candidates=candidates)
        run.assert_called_once_with(
            ("snap", "remove", "discord", "--revision=302", "--purge")
        )
        self.assertEqual([r.resource_id for r in result.completed_resources], ["discord@302"])

    def test_clean_reports_a_revision_failure_without_stopping_the_batch(self) -> None:
        candidates = [
            mod_linux_system.CleanCandidate(
                module="snap-old-revisions",
                path=None,
                label="old",
                estimated_bytes=None,
                level=Level.AGGRESSIVE,
                reason="old",
                resource_id="discord@302",
            )
        ]
        with mock.patch.object(
            mod_linux_system, "_run_snap", return_value=_completed(1, stderr="permission denied")
        ):
            result = mod_linux_system.clean_snap_old_revisions(candidates=candidates)
        self.assertEqual(result.operation_failures[0].resource_id, "discord@302")
        self.assertIn("permission denied", result.operation_failures[0].reason)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
