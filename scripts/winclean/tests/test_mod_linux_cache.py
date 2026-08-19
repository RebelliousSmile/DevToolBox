"""Tests de scripts/winclean/mod_linux_cache.py (Part 5 Phase 2)."""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import mod_linux_cache, procs  # noqa: E402
from scripts.winclean.common import Level  # noqa: E402


def tempdir(case: unittest.TestCase) -> Path:
    root = Path(tempfile.mkdtemp(prefix="winclean-mod-linux-cache-"))
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


def mkdirs(base: Path, names: list[str]) -> None:
    for name in names:
        (base / name).mkdir(parents=True, exist_ok=True)


def write(path: Path, size: int = 8) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"x" * size)
    return path


def names_of(candidates) -> set[str]:
    return {Path(c.path or "").name for c in candidates}


class BrowserCacheTest(unittest.TestCase):
    def setUp(self) -> None:
        patcher = mock.patch.object(procs, "is_running", return_value=set())
        patcher.start()
        self.addCleanup(patcher.stop)

    def _env(self, home: Path) -> dict[str, str]:
        return {"HOME": str(home)}

    def test_chromium_profile_yields_cache_only(self) -> None:
        home = tempdir(self)
        profile = home / ".cache" / "google-chrome" / "Default"
        mkdirs(profile, ["Cache", "Code Cache"])

        found = mod_linux_cache.discover_browser_cache(env=self._env(home))

        self.assertEqual(names_of(found), {"Cache", "Code Cache"})
        self.assertTrue(all(c.level == Level.MODERATE for c in found))
        self.assertTrue(all("Chrome" in c.label for c in found))

    def test_firefox_profile_uses_its_own_name_list(self) -> None:
        home = tempdir(self)
        profile = home / ".cache" / "mozilla" / "firefox" / "abc123.default"
        mkdirs(profile, ["cache2", "startupCache", "thumbnails"])

        found = mod_linux_cache.discover_browser_cache(env=self._env(home))

        self.assertEqual(names_of(found), {"cache2", "startupCache", "thumbnails"})
        self.assertTrue(all("Firefox" in c.label for c in found))

    def test_profile_without_allowlisted_name_yields_nothing(self) -> None:
        home = tempdir(self)
        profile = home / ".cache" / "google-chrome" / "Stale"
        mkdirs(profile, ["Session Storage"])

        found = mod_linux_cache.discover_browser_cache(env=self._env(home))

        self.assertEqual([], found)

    def test_absent_cache_home_yields_nothing(self) -> None:
        home = tempdir(self)
        found = mod_linux_cache.discover_browser_cache(env=self._env(home))
        self.assertEqual([], found)

    def test_owner_warning_is_attached_when_a_browser_runs(self) -> None:
        home = tempdir(self)
        profile = home / ".cache" / "google-chrome" / "Default"
        mkdirs(profile, ["Cache"])
        with mock.patch.object(procs, "is_running", return_value={"chrome"}):
            found = mod_linux_cache.discover_browser_cache(env=self._env(home))
        self.assertEqual(1, len(found))
        self.assertIn("chrome", found[0].reason)


class UserCacheTest(unittest.TestCase):
    def _env(self, home: Path) -> dict[str, str]:
        return {"HOME": str(home)}

    def test_first_level_entries_are_reported(self) -> None:
        home = tempdir(self)
        mkdirs(home / ".cache", ["some-app", "another-app"])

        found = mod_linux_cache.discover_user_cache(env=self._env(home))

        self.assertEqual(names_of(found), {"some-app", "another-app"})
        self.assertTrue(all(c.level == Level.MODERATE for c in found))

    def test_names_already_covered_elsewhere_are_excluded(self) -> None:
        home = tempdir(self)
        mkdirs(
            home / ".cache",
            [
                "pip",
                "ms-playwright",
                "ms-playwright-go",
                "ms-playwright-mcp",
                "google-chrome",
                "chromium",
                "BraveSoftware",
                "vivaldi",
                "mozilla",
                "some-app",
            ],
        )

        found = mod_linux_cache.discover_user_cache(env=self._env(home))

        self.assertEqual(names_of(found), {"some-app"})

    def test_absent_cache_home_yields_nothing(self) -> None:
        home = tempdir(self)
        found = mod_linux_cache.discover_user_cache(env=self._env(home))
        self.assertEqual([], found)

    def test_a_file_directly_under_cache_home_is_not_swept(self) -> None:
        home = tempdir(self)
        write(home / ".cache" / "some-app" / "sub" / "data.bin", 42)
        write(home / ".cache" / "stray-file.txt", 3)

        found = mod_linux_cache.discover_user_cache(env=self._env(home))

        self.assertEqual(names_of(found), {"some-app"})


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
