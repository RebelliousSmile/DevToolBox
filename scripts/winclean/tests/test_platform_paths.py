"""Tests de scripts/winclean/platform_paths.py (Part 5 Phase 1).

Style aligné sur `scripts/system_inventory/tests/test_xdg_dirs.py` : chaque
fonction accepte un `env` synthétique, jamais `os.environ` réel - le test reste
indépendant de la machine qui l'exécute.
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean.platform_paths import (  # noqa: E402
    APT_ARCHIVES_DIR,
    cache_home,
    config_home,
    data_home,
    home,
    state_home,
    trash_files_dir,
    trash_home,
    trash_info_dir,
)


class HomeResolutionTests(unittest.TestCase):
    def test_home_present_returns_path(self) -> None:
        self.assertEqual(home({"HOME": "/home/someone"}), Path("/home/someone"))

    def test_home_absent_returns_none(self) -> None:
        self.assertIsNone(home({}))

    def test_home_blank_returns_none(self) -> None:
        self.assertIsNone(home({"HOME": ""}))


class XdgDelegationTests(unittest.TestCase):
    """Ne reteste pas la logique de repli de `xdg_dirs` (déjà couverte
    Part 4) - vérifie seulement que ce module la ré-expose sans la dupliquer.
    """

    def test_cache_home_delegates_to_xdg_dirs(self) -> None:
        env = {"XDG_CACHE_HOME": "/custom/cache", "HOME": "/home/someone"}
        self.assertEqual(cache_home(env), Path("/custom/cache"))

    def test_data_home_delegates_to_xdg_dirs(self) -> None:
        env = {"HOME": "/home/someone"}
        self.assertEqual(data_home(env), Path("/home/someone/.local/share"))

    def test_config_home_delegates_to_xdg_dirs(self) -> None:
        env = {"HOME": "/home/someone"}
        self.assertEqual(config_home(env), Path("/home/someone/.config"))

    def test_state_home_delegates_to_xdg_dirs(self) -> None:
        env = {"HOME": "/home/someone"}
        self.assertEqual(state_home(env), Path("/home/someone/.local/state"))


class TrashDirResolutionTests(unittest.TestCase):
    def test_trash_home_is_data_home_slash_trash(self) -> None:
        env = {"HOME": "/home/someone"}
        self.assertEqual(trash_home(env), Path("/home/someone/.local/share/Trash"))

    def test_trash_home_honors_xdg_data_home_override(self) -> None:
        env = {"XDG_DATA_HOME": "/custom/data", "HOME": "/home/someone"}
        self.assertEqual(trash_home(env), Path("/custom/data/Trash"))

    def test_trash_files_and_info_are_trash_home_subdirs(self) -> None:
        env = {"HOME": "/home/someone"}
        base = Path("/home/someone/.local/share/Trash")
        self.assertEqual(trash_files_dir(env), base / "files")
        self.assertEqual(trash_info_dir(env), base / "info")

    def test_trash_dirs_are_none_when_data_home_is_unresolvable(self) -> None:
        self.assertIsNone(trash_home({}))
        self.assertIsNone(trash_files_dir({}))
        self.assertIsNone(trash_info_dir({}))


class AptArchivesDirTests(unittest.TestCase):
    def test_it_is_the_fixed_system_path_not_derived_from_home(self) -> None:
        self.assertEqual(APT_ARCHIVES_DIR, Path("/var/cache/apt/archives"))


if __name__ == "__main__":
    unittest.main()
