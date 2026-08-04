"""Tests de la suppression Windows (Phase 2, tâches 1 à 5)."""

from __future__ import annotations

import ctypes
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import remove  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    CleanCandidate,
    CleanResult,
    Level,
    Plan,
    format_plan,
    format_result_report,
    to_json_payload,
)
from scripts.winclean.remove import (  # noqa: E402
    DRIVE_FIXED,
    MAX_PATH,
    RECYCLE_FAILED,
    can_recycle,
    delete_tree,
    is_lock_error,
    long_path,
    measure_freed,
    recycle,
)

PREFIX = "\\\\?\\"
IS_WINDOWS = sys.platform == "win32"


def make_tempdir(case: unittest.TestCase) -> Path:
    """Répertoire temporaire nettoyé par `delete_tree` — seul capable des chemins longs."""
    root = Path(tempfile.mkdtemp(prefix="winclean-")).resolve()
    case.addCleanup(delete_tree, root)
    return root


def make_junction(link: Path, target: Path) -> bool:
    completed = subprocess.run(
        ["cmd", "/c", "mklink", "/J", str(link), str(target)],
        capture_output=True,
        text=True,
    )
    return completed.returncode == 0 and link.exists()


class TestLongPath(unittest.TestCase):
    def test_relative_input_raises(self) -> None:
        with self.assertRaises(ValueError):
            long_path("relatif\\ailleurs")

    def test_unresolved_absolute_input_raises(self) -> None:
        # Le cas qui sépare l'assertion demandée d'un simple test d'absoluité :
        # `C:\a\..\b` est absolu, et le préfixe en change le sens en silence.
        self.assertTrue(os.path.isabs("C:\\a\\..\\b"))
        for bad in ("C:\\a\\..\\b", "C:\\a\\.\\b", "C:\\a\\b.", "C:\\a\\b "):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                long_path(bad)

    def test_resolved_absolute_input_is_prefixed(self) -> None:
        self.assertEqual(long_path("C:\\dev\\projet\\target"), PREFIX + "C:\\dev\\projet\\target")

    def test_unc_gets_the_unc_form(self) -> None:
        self.assertEqual(
            long_path("\\\\serveur\\partage\\dossier"),
            PREFIX + "UNC\\serveur\\partage\\dossier",
        )

    def test_already_prefixed_is_left_alone(self) -> None:
        once = long_path("C:\\dev\\x")
        self.assertEqual(long_path(once), once)


@unittest.skipUnless(IS_WINDOWS, "suppression Windows")
class TestDeleteTree(unittest.TestCase):
    def test_tree_beyond_max_path_is_deleted(self) -> None:
        root = make_tempdir(self)
        deep = root
        while len(str(deep)) < 300:
            deep = deep / ("segment-" + "x" * 24)
        os.makedirs(long_path(deep))
        payload = b"z" * 128
        with open(long_path(deep / "fichier.bin"), "wb") as handle:
            handle.write(payload)
        self.assertGreater(len(str(deep / "fichier.bin")), MAX_PATH)

        deleted, failed, errors = delete_tree(root)

        self.assertEqual(errors, [])
        self.assertEqual(failed, 0)
        self.assertEqual(deleted, len(payload))
        self.assertFalse(root.exists())

    def test_junction_is_removed_as_an_entry_and_its_target_survives(self) -> None:
        root = make_tempdir(self)
        target = root / "cible"
        target.mkdir()
        (target / "precieux.txt").write_text("à garder", encoding="utf-8")
        tree = root / "arbre"
        tree.mkdir()
        if not make_junction(tree / "lien", target):
            self.skipTest("mklink /J indisponible")

        deleted, failed, errors = delete_tree(tree)

        self.assertEqual(errors, [])
        self.assertEqual(failed, 0)
        self.assertEqual(deleted, 0)  # une jonction ne porte aucun octet
        self.assertFalse(tree.exists())
        self.assertTrue(target.is_dir())
        self.assertEqual((target / "precieux.txt").read_text(encoding="utf-8"), "à garder")

    def test_open_file_is_reported_failed_with_winerror_32_without_raising(self) -> None:
        root = make_tempdir(self)
        tree = root / "verrou"
        tree.mkdir()
        locked = tree / "tenu.bin"
        locked.write_bytes(b"w" * 64)

        with open(locked, "rb"):
            deleted, failed, errors = delete_tree(tree)

        self.assertEqual(deleted, 0)
        self.assertEqual(failed, 64)
        locks = [e for e in errors if e.winerror == 32]
        self.assertEqual(len(locks), 1)
        self.assertTrue(is_lock_error(locks[0]))
        self.assertEqual(locks[0].size, 64)
        self.assertTrue(locked.exists())

    def test_dir_not_empty_is_partial_never_a_first_cause(self) -> None:
        """145 ne survient que parce qu'une fille a résisté, et elle est déjà signalée."""
        from scripts.winclean.remove import (
            ERROR_DIR_NOT_EMPTY,
            RemovalError,
            is_partial_error,
        )

        consequence = RemovalError(path="x", winerror=ERROR_DIR_NOT_EMPTY, message="pas vide")
        self.assertFalse(is_lock_error(consequence))
        self.assertTrue(is_partial_error(consequence))
        self.assertTrue(is_partial_error(RemovalError(path="y", winerror=32, message="verrou")))
        self.assertFalse(is_partial_error(RemovalError(path="z", winerror=5, message="refus")))

    def test_missing_root_is_a_result_not_an_error(self) -> None:
        root = make_tempdir(self)
        deleted, failed, errors = delete_tree(root / "jamais-cree")
        self.assertEqual((deleted, failed, errors), (0, 0, []))


@unittest.skipUnless(IS_WINDOWS, "API Win32")
class TestCanRecycle(unittest.TestCase):
    def test_false_beyond_max_path(self) -> None:
        deep = Path("C:\\") / ("d" * 40)
        while len(long_path(deep)) <= MAX_PATH:
            deep = deep / ("d" * 40)
        with mock.patch.object(remove, "_get_drive_type", return_value=DRIVE_FIXED):
            self.assertFalse(can_recycle(deep))

    def test_false_on_a_network_volume(self) -> None:
        with mock.patch.object(remove, "_get_drive_type", return_value=4) as drive_type:
            self.assertFalse(can_recycle("C:\\dev\\projet\\target"))
        drive_type.assert_called_once()

    def test_true_on_a_fixed_volume(self) -> None:
        with mock.patch.object(remove, "_get_drive_type", return_value=DRIVE_FIXED):
            self.assertTrue(can_recycle("C:\\dev\\projet\\target"))

    def test_unresolved_path_is_refused_rather_than_prefixed(self) -> None:
        self.assertFalse(can_recycle("C:\\a\\..\\b"))


@unittest.skipUnless(IS_WINDOWS, "API Win32")
class TestRecycleFailure(unittest.TestCase):
    """Décision 14 : échec de corbeille = candidat laissé en place, aucun repli."""

    def setUp(self) -> None:
        root = make_tempdir(self)
        self.target = root / "a-jeter.bin"
        self.target.write_bytes(b"k" * 32)

    def _assert_failed(self, outcome: remove.RecycleOutcome, code: int) -> None:
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, RECYCLE_FAILED)
        self.assertIn(str(code), outcome.detail)
        self.assertTrue(self.target.exists())
        # Le détail porte le code brut, pas un message tiré de l'espace d'erreurs
        # système : SHFileOperationW renvoie ses propres codes DE_*.
        system_message = ctypes.WinError(code).strerror
        if system_message:
            self.assertNotIn(system_message, outcome.detail)

    def test_non_zero_return_fails_and_never_falls_back(self) -> None:
        with mock.patch.object(remove, "delete_tree") as spy, mock.patch.object(
            remove, "_sh_file_operation", return_value=124
        ):
            outcome = recycle(self.target)
        spy.assert_not_called()
        self._assert_failed(outcome, 124)

    def test_aborted_flag_fails_even_on_a_zero_return(self) -> None:
        def abort(op: remove.SHFILEOPSTRUCTW) -> int:
            op.fAnyOperationsAborted = 1
            return 0

        with mock.patch.object(remove, "delete_tree") as spy, mock.patch.object(
            remove, "_sh_file_operation", side_effect=abort
        ):
            outcome = recycle(self.target)
        spy.assert_not_called()
        self.assertFalse(outcome.ok)
        self.assertTrue(outcome.aborted)
        self.assertEqual(outcome.reason, RECYCLE_FAILED)
        self.assertIn("fAnyOperationsAborted", outcome.detail)
        self.assertTrue(self.target.exists())

    def test_flag_set_is_exactly_the_four_required_flags(self) -> None:
        seen: dict[str, int] = {}

        def capture(op: remove.SHFILEOPSTRUCTW) -> int:
            seen["flags"] = op.fFlags
            seen["func"] = op.wFunc
            seen["from"] = 0 if op.pFrom is None else len(op.pFrom)
            return 0

        with mock.patch.object(remove, "_sh_file_operation", side_effect=capture):
            recycle(self.target)
        self.assertEqual(seen["func"], remove.FO_DELETE)
        self.assertEqual(
            seen["flags"],
            remove.FOF_ALLOWUNDO
            | remove.FOF_NOCONFIRMATION
            | remove.FOF_NOERRORUI
            | remove.FOF_SILENT,
        )


@unittest.skipUnless(IS_WINDOWS, "corbeille Windows")
class TestRecycleReal(unittest.TestCase):
    def test_temp_file_leaves_its_original_location(self) -> None:
        root = make_tempdir(self)
        target = root / "corbeille-reelle.bin"
        target.write_bytes(b"r" * 16)
        if not can_recycle(target):
            self.skipTest("volume non fixe ou chemin trop long pour la corbeille")

        outcome = recycle(target)

        self.assertTrue(outcome.ok, msg=outcome.detail)
        self.assertEqual(outcome.code, 0)
        self.assertFalse(target.exists())


class TestNoPrefixLeaks(unittest.TestCase):
    """La forme préfixée ne sort pas de `remove.py`."""

    def test_no_report_or_payload_path_carries_the_prefix(self) -> None:
        root = make_tempdir(self)
        tree = root / "fixture"
        tree.mkdir()
        locked = tree / "tenu.bin"
        locked.write_bytes(b"p" * 8)

        if IS_WINDOWS:
            with open(locked, "rb"):
                _, failed, errors = delete_tree(tree)
        else:  # pragma: no cover - la suite cible Windows
            _, failed, errors = delete_tree(tree)

        candidate = CleanCandidate(
            module="fixture",
            path=str(tree),
            label="arbre de test",
            estimated_bytes=8,
            level=Level.SAFE,
            reason="régénérable",
        )
        result = CleanResult(module="fixture", estimated=8, failed=failed or None)
        surfaces = [
            format_plan(Plan(candidates=[candidate])),
            format_result_report([result]),
            repr(to_json_payload(Plan(candidates=[candidate]))),
            repr(to_json_payload(result)),
            repr([e.to_json_payload() for e in errors]),
        ]
        for text in surfaces:
            self.assertNotIn(PREFIX, text)

    @unittest.skipUnless(IS_WINDOWS, "verrou de partage Windows")
    def test_lock_error_path_is_the_plain_form(self) -> None:
        root = make_tempdir(self)
        locked = root / "tenu.bin"
        locked.write_bytes(b"l" * 4)
        with open(locked, "rb"):
            _, _, errors = delete_tree(root)
        locks = [e for e in errors if e.winerror == 32]
        self.assertEqual([e.path for e in locks], [str(locked)])


class TestMeasureFreed(unittest.TestCase):
    def test_gone_path_without_a_prior_measure_is_unmeasurable(self) -> None:
        gone = Path(tempfile.gettempdir()).resolve() / "winclean-absent-mesure"
        self.assertIsNone(measure_freed(None, gone))

    def test_gone_path_with_a_known_measure_returns_that_number(self) -> None:
        gone = Path(tempfile.gettempdir()).resolve() / "winclean-absent-mesure"
        self.assertEqual(measure_freed(4096, gone), 4096)

    def test_still_present_path_returns_the_difference(self) -> None:
        root = make_tempdir(self)
        (root / "reste.bin").write_bytes(b"s" * 100)
        self.assertEqual(measure_freed(500, root), 400)

    def test_negative_difference_is_reported_as_zero(self) -> None:
        root = make_tempdir(self)
        (root / "grossi.bin").write_bytes(b"g" * 400)
        self.assertEqual(measure_freed(100, root), 0)

    def test_unmeasurable_after_state_is_none_not_a_guess(self) -> None:
        root = make_tempdir(self)
        (root / "x.bin").write_bytes(b"x" * 10)
        with mock.patch("os.scandir", side_effect=PermissionError(5, "denied")):
            self.assertIsNone(measure_freed(500, root))


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
