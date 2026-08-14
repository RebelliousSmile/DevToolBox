"""Tests de scripts/winclean/trash_linux.py (Part 5 Phase 2)."""

from __future__ import annotations

import datetime
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import trash_linux  # noqa: E402


def tempdir(case: unittest.TestCase) -> Path:
    root = Path(tempfile.mkdtemp(prefix="winclean-trash-linux-"))
    case.addCleanup(_rmtree, root)
    return root


def _rmtree(root: Path) -> None:
    for current, directories, files in os.walk(root, topdown=False):
        for name in files:
            try:
                os.unlink(os.path.join(current, name))
            except OSError:
                pass
        for name in directories:
            try:
                os.rmdir(os.path.join(current, name))
            except OSError:
                pass
    try:
        os.rmdir(root)
    except OSError:
        pass


def write(path: Path, size: int = 8) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"x" * size)
    return path


class CanTrashTest(unittest.TestCase):
    def test_a_relative_path_is_refused(self) -> None:
        self.assertFalse(trash_linux.can_trash("relative/file", env={"HOME": "/home/someone"}))

    def test_an_absent_path_is_refused(self) -> None:
        home = tempdir(self)
        self.assertFalse(trash_linux.can_trash(home / "jamais", env={"HOME": str(home)}))

    def test_an_existing_absolute_path_with_a_resolvable_home_is_accepted(self) -> None:
        home = tempdir(self)
        target = write(home / "some-file.txt")
        self.assertTrue(trash_linux.can_trash(target, env={"HOME": str(home)}))

    def test_an_unresolvable_home_is_refused(self) -> None:
        home = tempdir(self)
        target = write(home / "some-file.txt")
        self.assertFalse(trash_linux.can_trash(target, env={}))


class InfoFileContentTest(unittest.TestCase):
    def test_the_format_matches_the_freedesktop_spec(self) -> None:
        when = datetime.datetime(2026, 8, 6, 10, 30, 15)
        content = trash_linux.info_file_content(Path("/home/someone/some file.txt"), when)
        self.assertEqual(
            content,
            "[Trash Info]\nPath=/home/someone/some%20file.txt\nDeletionDate=2026-08-06T10:30:15\n",
        )

    def test_percent_and_reserved_characters_are_encoded(self) -> None:
        when = datetime.datetime(2026, 1, 1, 0, 0, 0)
        content = trash_linux.info_file_content(Path("/home/someone/100%done.txt"), when)
        self.assertIn("Path=/home/someone/100%25done.txt", content)


class MoveToTrashTest(unittest.TestCase):
    def _env(self, home: Path) -> dict[str, str]:
        return {"HOME": str(home)}

    def test_a_file_is_moved_and_its_trashinfo_is_written(self) -> None:
        home = tempdir(self)
        source = write(home / "project" / "cache.bin", 32)
        when = datetime.datetime(2026, 8, 6, 9, 0, 0)

        outcome = trash_linux.move_to_trash(source, env=self._env(home), now=when)

        self.assertTrue(outcome.ok)
        self.assertFalse(source.exists())
        trashed = Path(outcome.trashed_path or "")
        self.assertTrue(trashed.is_file())
        self.assertEqual(trashed.parent, home / ".local" / "share" / "Trash" / "files")
        info_path = home / ".local" / "share" / "Trash" / "info" / f"{trashed.name}.trashinfo"
        self.assertTrue(info_path.is_file())
        self.assertIn(f"Path={str(source)}", info_path.read_text(encoding="utf-8"))
        self.assertIn("DeletionDate=2026-08-06T09:00:00", info_path.read_text(encoding="utf-8"))

    def test_a_directory_is_moved_whole(self) -> None:
        home = tempdir(self)
        source = home / "project" / "node_modules"
        write(source / "a" / "b.txt", 4)

        outcome = trash_linux.move_to_trash(source, env=self._env(home))

        self.assertTrue(outcome.ok)
        self.assertFalse(source.exists())
        trashed = Path(outcome.trashed_path or "")
        self.assertTrue((trashed / "a" / "b.txt").is_file())

    def test_a_name_collision_gets_a_numeric_suffix(self) -> None:
        home = tempdir(self)
        first = write(home / "cache.bin", 4)
        first_outcome = trash_linux.move_to_trash(first, env=self._env(home))
        self.assertTrue(first_outcome.ok)

        second = write(home / "cache.bin", 8)
        second_outcome = trash_linux.move_to_trash(second, env=self._env(home))

        self.assertTrue(second_outcome.ok)
        self.assertNotEqual(first_outcome.trashed_path, second_outcome.trashed_path)
        self.assertTrue(Path(second_outcome.trashed_path or "").name.startswith("cache.bin ("))

    def test_an_orphan_trashinfo_still_blocks_the_name(self) -> None:
        home = tempdir(self)
        info_dir = home / ".local" / "share" / "Trash" / "info"
        info_dir.mkdir(parents=True)
        (info_dir / "cache.bin.trashinfo").write_text("[Trash Info]\n", encoding="utf-8")
        source = write(home / "cache.bin", 4)

        outcome = trash_linux.move_to_trash(source, env=self._env(home))

        self.assertTrue(outcome.ok)
        self.assertTrue(Path(outcome.trashed_path or "").name.startswith("cache.bin ("))

    def test_an_absent_source_fails_without_creating_anything(self) -> None:
        home = tempdir(self)
        outcome = trash_linux.move_to_trash(home / "jamais", env=self._env(home))

        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, trash_linux.TRASH_FAILED)
        self.assertFalse((home / ".local" / "share" / "Trash").exists())

    def test_a_relative_path_is_refused(self) -> None:
        outcome = trash_linux.move_to_trash("relative/file", env=self._env(tempdir(self)))
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, trash_linux.TRASH_FAILED)

    def test_an_unresolvable_home_fails(self) -> None:
        home = tempdir(self)
        source = write(home / "cache.bin", 4)
        outcome = trash_linux.move_to_trash(source, env={})
        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, trash_linux.TRASH_FAILED)

    def test_a_move_failure_removes_the_orphaned_trashinfo(self) -> None:
        home = tempdir(self)
        source = write(home / "cache.bin", 4)

        with mock.patch.object(trash_linux.shutil, "move", side_effect=OSError("boom")):
            outcome = trash_linux.move_to_trash(source, env=self._env(home))

        self.assertFalse(outcome.ok)
        self.assertEqual(outcome.reason, trash_linux.TRASH_FAILED)
        info_dir = home / ".local" / "share" / "Trash" / "info"
        self.assertEqual([], list(info_dir.iterdir()))


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
