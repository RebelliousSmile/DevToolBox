"""Tests des modules `moderate` d'applications (Part 2, phases 1 et 2)."""

from __future__ import annotations

import io
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import guards, mod_apps, mod_dev, procs  # noqa: E402
from scripts.winclean.common import CleanCandidate, Level  # noqa: E402

_PACKAGE_DIR = Path(__file__).resolve().parent.parent


def tempdir(case: unittest.TestCase) -> Path:
    root = Path(tempfile.mkdtemp(prefix="winclean-apps-"))
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


def names_of(candidates: list[CleanCandidate]) -> set[str]:
    return {Path(c.path or "").name for c in candidates}


class SingleProcessQueryTest(unittest.TestCase):
    """Phase 1 : `is_running` n'est défini qu'une fois, dans `procs.py`.

    L'assertion porte sur les **sources** du paquet et pas sur un nombre de
    modules : Part 3 en ajoute trois, ce qui ferait échouer un décompte sans
    qu'aucune duplication n'ait été introduite.
    """

    def test_only_procs_defines_is_running(self) -> None:
        holders = sorted(
            path.name
            for path in _PACKAGE_DIR.glob("*.py")
            if "def is_running" in path.read_text(encoding="utf-8")
        )
        self.assertEqual(["procs.py"], holders)

    def test_mod_apps_delegates_to_procs(self) -> None:
        with mock.patch.object(procs, "is_running", return_value={"chrome.exe"}) as spy:
            warning = mod_apps._owner_reason(mod_apps.BROWSER_OWNERS)
        spy.assert_called_once_with(mod_apps.BROWSER_OWNERS)
        self.assertIsNotNone(warning)
        self.assertIn("chrome.exe", warning or "")

    def test_unknown_process_state_is_announced(self) -> None:
        """Un `None` doit **parler** : `warn-only` n'omet jamais rien d'autre."""
        with mock.patch.object(procs, "is_running", return_value=None):
            warning = mod_apps._owner_reason(mod_apps.BROWSER_OWNERS)
        self.assertIsNotNone(warning)
        self.assertIn("inconnu", (warning or "").lower())

    def test_nothing_running_stays_silent(self) -> None:
        with mock.patch.object(procs, "is_running", return_value=set()):
            self.assertIsNone(mod_apps._owner_reason(mod_apps.BROWSER_OWNERS))


class BrowserCacheTest(unittest.TestCase):
    def setUp(self) -> None:
        patcher = mock.patch.object(procs, "is_running", return_value=set())
        patcher.start()
        self.addCleanup(patcher.stop)

    def _chromium_env(self, local: Path) -> dict[str, str]:
        return {"LOCALAPPDATA": str(local), "APPDATA": str(local / "roaming")}

    def test_chromium_profile_yields_cache_only(self) -> None:
        local = tempdir(self)
        profile = local / "Google" / "Chrome" / "User Data" / "Default"
        mkdirs(profile, ["Cache", "Local Storage", "IndexedDB"])
        write(profile / "Login Data")
        write(profile / "Bookmarks")

        found = mod_apps.discover_browser_cache(env=self._chromium_env(local))

        self.assertEqual(1, len(found))
        self.assertEqual(str(profile / "Cache"), found[0].path)
        self.assertEqual(Level.MODERATE, found[0].level)
        self.assertIn("Chrome", found[0].label)

    def test_profile_root_is_not_protected(self) -> None:
        """Assertion dans ce sens : un sous-arbre protégé annulerait le module.

        `is_protected()` correspond à un chemin exact **ou** à un sous-arbre
        résolu ; protéger la racine de profil disqualifierait silencieusement le
        `Cache` que le test précédent exige.
        """
        local = tempdir(self)
        profile = local / "Google" / "Chrome" / "User Data" / "Default"
        mkdirs(profile, ["Cache"])

        self.assertFalse(
            guards.is_protected(guards.normalise(profile), guards.DEFAULT_PROTECTED)
        )
        self.assertFalse(
            guards.is_protected(
                guards.normalise(profile / "Cache"), guards.DEFAULT_PROTECTED
            )
        )

    def test_profile_without_allowlisted_name_yields_nothing(self) -> None:
        """Jamais de repli sur la racine du profil : un profil vide rend zéro."""
        local = tempdir(self)
        profile = local / "Google" / "Chrome" / "User Data" / "Stale Profile"
        mkdirs(profile, ["Local Storage", "Service Worker"])

        found = mod_apps.discover_browser_cache(env=self._chromium_env(local))

        self.assertEqual([], found)

    def test_firefox_layout_is_enumerated_on_its_own_shape(self) -> None:
        """Firefox n'a pas de `User Data` : ses profils sont sous `Profiles`."""
        local = tempdir(self)
        profile = local / "Mozilla" / "Firefox" / "Profiles" / "a1b2c3.default"
        mkdirs(profile, ["cache2", "settings", "safebrowsing"])

        found = mod_apps.discover_browser_cache(env=self._chromium_env(local))

        self.assertEqual({"cache2"}, names_of(found))
        self.assertIn("Firefox", found[0].label)

    def test_multiple_families_coexist(self) -> None:
        local = tempdir(self)
        mkdirs(local / "Google" / "Chrome" / "User Data" / "Default", ["Cache"])
        mkdirs(local / "Microsoft" / "Edge" / "User Data" / "Default", ["GPUCache"])
        mkdirs(local / "Mozilla" / "Firefox" / "Profiles" / "x.default", ["startupCache"])

        found = mod_apps.discover_browser_cache(env=self._chromium_env(local))

        self.assertEqual({"Cache", "GPUCache", "startupCache"}, names_of(found))

    def test_missing_variable_yields_nothing(self) -> None:
        self.assertEqual([], mod_apps.discover_browser_cache(env={}))

    def test_running_browser_warning_reaches_the_reason(self) -> None:
        local = tempdir(self)
        mkdirs(local / "Google" / "Chrome" / "User Data" / "Default", ["Cache"])
        with mock.patch.object(procs, "is_running", return_value={"chrome.exe"}):
            found = mod_apps.discover_browser_cache(env=self._chromium_env(local))
        self.assertIn("chrome.exe", found[0].reason)


class VscodeCacheTest(unittest.TestCase):
    #: Les onze répertoires relevés sur une installation réelle.
    REAL_LAYOUT = [
        "Cache",
        "Code Cache",
        "GPUCache",
        "CachedData",
        "DawnGraphiteCache",
        "DawnWebGPUCache",
        "CachedConfigurations",
        "CachedProfilesData",
        "CachedExtensionVSIXs",
        "logs",
        "User",
    ]

    def setUp(self) -> None:
        patcher = mock.patch.object(procs, "is_running", return_value=set())
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_exactly_the_six_allowlisted_names(self) -> None:
        """Ensemble **exact** : un motif `*Cache*` en prendrait neuf."""
        roaming = tempdir(self)
        mkdirs(roaming / "Code", self.REAL_LAYOUT)

        found = mod_apps.discover_vscode_cache(env={"APPDATA": str(roaming)})

        self.assertEqual(set(mod_apps.VSCODE_CACHE_NAMES), names_of(found))
        self.assertEqual(6, len(found))
        for excluded in (
            "CachedConfigurations",
            "CachedProfilesData",
            "CachedExtensionVSIXs",
            "logs",
            "User",
        ):
            self.assertNotIn(excluded, names_of(found))

    def test_cursor_is_covered_too(self) -> None:
        roaming = tempdir(self)
        mkdirs(roaming / "Cursor", ["Cache", "User"])

        found = mod_apps.discover_vscode_cache(env={"APPDATA": str(roaming)})

        self.assertEqual({"Cache"}, names_of(found))
        self.assertIn("Cursor", found[0].label)

    def test_no_installation_yields_nothing(self) -> None:
        roaming = tempdir(self)
        self.assertEqual([], mod_apps.discover_vscode_cache(env={"APPDATA": str(roaming)}))


class UserTempTest(unittest.TestCase):
    def test_first_level_entries_files_and_directories(self) -> None:
        temp = tempdir(self)
        write(temp / "installer.log", 16)
        mkdirs(temp, ["scratch"])
        write(temp / "scratch" / "deep.bin", 32)

        found = mod_apps.discover_user_temp(env={"TEMP": str(temp)})

        self.assertEqual({"installer.log", "scratch"}, names_of(found))

    def test_candidates_carry_a_mtime_so_the_secondary_guard_runs(self) -> None:
        temp = tempdir(self)
        write(temp / "a.tmp", 4)

        found = mod_apps.discover_user_temp(env={"TEMP": str(temp)})

        self.assertEqual(1, len(found))
        self.assertIsNotNone(found[0].stat_mtime)

    def test_missing_variable_yields_nothing(self) -> None:
        self.assertEqual([], mod_apps.discover_user_temp(env={}))

    def test_nested_build_candidate_is_absorbed_and_counted_once(self) -> None:
        """Décision 13, à travers deux familles de modules.

        Un `__pycache__` sous une entrée de `%TEMP%` est absorbé par celle-ci :
        l'ancêtre garde le candidat et les octets ne comptent qu'une fois.
        """
        temp = tempdir(self)
        nested = temp / "build-xyz" / "pkg" / "__pycache__"
        write(nested / "mod.cpython-312.pyc", 128)

        temp_candidates = mod_apps.discover_user_temp(env={"TEMP": str(temp)})
        pycache = mod_dev.discover_pycache(roots=[str(temp)], max_depth=6)
        self.assertTrue(pycache, "le fixture doit produire un candidat __pycache__")

        kept, absorbed = guards.absorb_nested(temp_candidates + pycache)

        self.assertEqual(1, len(kept))
        self.assertEqual("user-temp", kept[0].module)
        self.assertEqual(str(temp / "build-xyz"), kept[0].path)
        self.assertEqual(["pycache"], [entry.module for entry in absorbed])


class CrashdumpsTest(unittest.TestCase):
    def test_contents_are_listed(self) -> None:
        local = tempdir(self)
        write(local / "CrashDumps" / "app.exe.1234.dmp", 64)

        found = mod_apps.discover_crashdumps(env={"LOCALAPPDATA": str(local)})

        self.assertEqual({"app.exe.1234.dmp"}, names_of(found))
        self.assertEqual(Level.MODERATE, found[0].level)

    def test_missing_directory_yields_nothing(self) -> None:
        local = tempdir(self)
        self.assertEqual([], mod_apps.discover_crashdumps(env={"LOCALAPPDATA": str(local)}))


def _completed(returncode: int, stdout: str = "", stderr: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(
        args=list(mod_apps.DOCKER_PRUNE_COMMAND),
        returncode=returncode,
        stdout=stdout,
        stderr=stderr,
    )


class DockerLightTest(unittest.TestCase):
    def test_prune_vector_has_no_volumes_and_no_all(self) -> None:
        """Les volumes portent des données irremplaçables (décision figée 7)."""
        self.assertNotIn("--volumes", mod_apps.DOCKER_PRUNE_COMMAND)
        self.assertNotIn("-a", mod_apps.DOCKER_PRUNE_COMMAND)
        self.assertNotIn("--all", mod_apps.DOCKER_PRUNE_COMMAND)
        self.assertEqual(("docker", "system", "prune", "-f"), mod_apps.DOCKER_PRUNE_COMMAND)

    def test_probe_has_exactly_two_stages(self) -> None:
        self.assertEqual(
            ("docker", "system", "df", "--format", "json"), mod_apps.DOCKER_DF_COMMAND
        )
        source = (_PACKAGE_DIR / "mod_apps.py").read_text(encoding="utf-8")
        self.assertNotIn('"info"', source)

    def test_missing_binary_yields_nothing_and_says_nothing(self) -> None:
        stderr = io.StringIO()
        with mock.patch.object(mod_apps.shutil, "which", return_value=None):
            with redirect_stderr(stderr):
                found = mod_apps.discover_docker_light()
        self.assertEqual([], found)
        self.assertEqual("", stderr.getvalue())

    def test_stopped_daemon_yields_nothing(self) -> None:
        stderr = io.StringIO()
        with mock.patch.object(mod_apps.shutil, "which", return_value="docker.exe"):
            with mock.patch.object(mod_apps, "_run", return_value=_completed(1)):
                with redirect_stderr(stderr):
                    found = mod_apps.discover_docker_light()
        self.assertEqual([], found)
        self.assertEqual("", stderr.getvalue())

    def test_available_daemon_yields_one_unpriced_pathless_candidate(self) -> None:
        with mock.patch.object(mod_apps.shutil, "which", return_value="docker.exe"):
            with mock.patch.object(mod_apps, "_run", return_value=_completed(0, "{}\n{}")):
                found = mod_apps.discover_docker_light()
        self.assertEqual(1, len(found))
        candidate = found[0]
        self.assertIsNone(candidate.path)
        self.assertIsNone(candidate.estimated_bytes)
        for kind in ("image", "conteneur", "cache"):
            self.assertIn(kind, candidate.label.lower())

    def test_clean_reports_none_and_never_parses_the_reclaimed_line(self) -> None:
        bait = "Total reclaimed space: 4.21GB\n"
        with mock.patch.object(mod_apps, "_run", return_value=_completed(0, bait)) as spy:
            result = mod_apps.clean_docker_light()
        spy.assert_called_once_with(mod_apps.DOCKER_PRUNE_COMMAND)
        self.assertIsNone(result.freed)
        self.assertIsNone(result.recycled)
        self.assertIsNone(result.failed)
        self.assertEqual("docker-light", result.module)

    def test_non_zero_prune_raises(self) -> None:
        with mock.patch.object(
            mod_apps, "_run", return_value=_completed(1, "", "daemon refused")
        ):
            with self.assertRaises(OSError):
                mod_apps.clean_docker_light()

    def test_unlaunchable_prune_raises(self) -> None:
        with mock.patch.object(mod_apps, "_run", return_value=None):
            with self.assertRaises(OSError):
                mod_apps.clean_docker_light()


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
