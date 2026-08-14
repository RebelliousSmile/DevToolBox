"""Tests de scripts/winclean/mod_linux_pkg.py (Part 5 Phase 2)."""

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

from scripts.winclean import mod_linux_pkg  # noqa: E402
from scripts.winclean.common import Level  # noqa: E402


def tempdir(case: unittest.TestCase) -> Path:
    root = Path(tempfile.mkdtemp(prefix="winclean-mod-linux-pkg-"))
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


def write(path: Path, size: int = 0) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"x" * size)
    return path


class TestResolveCachePath(unittest.TestCase):
    def test_the_tool_reported_path_wins_over_the_fallback(self) -> None:
        spec = mod_linux_pkg.CACHE_SPECS["pip-cache-linux"]
        with mock.patch.object(
            mod_linux_pkg, "_ask_tool", return_value=Path("/opt/pip-cache")
        ):
            resolved = mod_linux_pkg.resolve_cache_path(
                spec, env={"XDG_CACHE_HOME": "/custom/cache"}
            )
        self.assertEqual(resolved, Path("/opt/pip-cache"))

    def test_the_xdg_fallback_is_used_when_the_tool_is_silent(self) -> None:
        spec = mod_linux_pkg.CACHE_SPECS["pip-cache-linux"]
        with mock.patch.object(mod_linux_pkg, "_ask_tool", return_value=None):
            resolved = mod_linux_pkg.resolve_cache_path(
                spec, env={"XDG_CACHE_HOME": "/custom/cache"}
            )
        self.assertEqual(resolved, Path("/custom/cache/pip"))

    def test_pnpm_falls_back_under_data_home_not_cache_home(self) -> None:
        spec = mod_linux_pkg.CACHE_SPECS["pnpm-store-linux"]
        with mock.patch.object(mod_linux_pkg, "_ask_tool", return_value=None):
            resolved = mod_linux_pkg.resolve_cache_path(
                spec, env={"HOME": "/home/someone"}
            )
        self.assertEqual(resolved, Path("/home/someone/.local/share/pnpm/store"))

    def test_no_base_at_all_yields_no_path(self) -> None:
        spec = mod_linux_pkg.CACHE_SPECS["pip-cache-linux"]
        with mock.patch.object(mod_linux_pkg, "_ask_tool", return_value=None):
            self.assertIsNone(mod_linux_pkg.resolve_cache_path(spec, env={}))


class TestDiscoverCache(unittest.TestCase):
    def test_an_absent_cache_directory_yields_no_candidate(self) -> None:
        root = tempdir(self)
        with mock.patch.object(
            mod_linux_pkg, "resolve_cache_path", return_value=root / "jamais"
        ):
            self.assertEqual(mod_linux_pkg.discover_cache("pip-cache-linux"), [])

    def test_an_existing_cache_directory_yields_one_sized_candidate(self) -> None:
        root = tempdir(self)
        write(root / "wheels" / "a.whl", 700)
        with mock.patch.object(mod_linux_pkg, "resolve_cache_path", return_value=root):
            found = mod_linux_pkg.discover_cache("pip-cache-linux")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].estimated_bytes, 700)
        self.assertEqual(found[0].module, "pip-cache-linux")
        self.assertEqual(found[0].level, Level.SAFE)
        # `needs_network` n'est pas décidé ici : l'appelant de découverte
        # l'estampille depuis la déclaration du registre.
        self.assertFalse(found[0].needs_network)


class TestDiscoverAptArchives(unittest.TestCase):
    def test_an_absent_archives_directory_yields_no_candidate(self) -> None:
        with mock.patch.object(
            mod_linux_pkg.platform_paths, "APT_ARCHIVES_DIR", Path("/nonexistent/apt/archives")
        ):
            self.assertEqual(mod_linux_pkg.discover_apt_archives(), [])

    def test_an_existing_archives_directory_yields_one_sized_candidate(self) -> None:
        root = tempdir(self)
        write(root / "paquet.deb", 1234)
        with mock.patch.object(mod_linux_pkg.platform_paths, "APT_ARCHIVES_DIR", root):
            found = mod_linux_pkg.discover_apt_archives()
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].estimated_bytes, 1234)
        self.assertEqual(found[0].module, "apt-cache")
        self.assertEqual(found[0].level, Level.SAFE)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
