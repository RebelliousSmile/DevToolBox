"""Tests des modules `safe` de développement (Phase 3)."""

from __future__ import annotations

import dataclasses
import io
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import mod_dev, procs, registry_mod  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    DISCOVERY_FIXED,
    PROC_GUARD_WARN_AND_SKIP,
    PROC_GUARD_WARN_ONLY,
    SKIP_RUNNING,
    CleanCandidate,
    CleanModule,
    Level,
    Plan,
    format_plan,
)

IS_WINDOWS = sys.platform == "win32"


def tempdir(case: unittest.TestCase) -> Path:
    root = Path(tempfile.mkdtemp(prefix="winclean-mod-"))
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


def cargo_project(root: Path, name: str = "projet", marker: str = "debug") -> Path:
    """Projet cargo crédible : `Cargo.toml` + `target/` portant une marque."""
    project = root / name
    write(project / "Cargo.toml", 32)
    target = project / "target"
    if marker == "cachedir":
        write(target / "CACHEDIR.TAG", 43)
    elif marker:
        (target / marker).mkdir(parents=True, exist_ok=True)
    else:
        target.mkdir(parents=True, exist_ok=True)
    return target


class TestCargoTarget(unittest.TestCase):
    def test_target_next_to_cargo_toml_is_a_candidate(self) -> None:
        root = tempdir(self)
        target = cargo_project(root)
        write(target / "debug" / "projet.exe", 500)
        found = mod_dev.discover_cargo_target(roots=[root], max_depth=6)
        self.assertEqual([c.path for c in found], [str(target)])
        self.assertEqual(found[0].module, "cargo-target")
        self.assertEqual(found[0].level, Level.SAFE)

    def test_a_cachedir_tag_alone_qualifies(self) -> None:
        root = tempdir(self)
        target = cargo_project(root, marker="cachedir")
        found = mod_dev.discover_cargo_target(roots=[root], max_depth=6)
        self.assertEqual([c.path for c in found], [str(target)])

    def test_decoy_target_without_cargo_toml_is_not_a_candidate(self) -> None:
        root = tempdir(self)
        # Un dossier nommé `target` peut être n'importe quoi : sans `Cargo.toml`
        # voisin, il n'appartient pas à ce module.
        write(root / "donnees" / "target" / "precieux.csv", 10)
        self.assertEqual(mod_dev.discover_cargo_target(roots=[root], max_depth=6), [])

    def test_target_without_marker_is_not_a_candidate(self) -> None:
        root = tempdir(self)
        target = cargo_project(root, marker="")
        write(target / "notes.txt", 10)
        # `Cargo.toml` voisin mais aucune marque de cargo : on s'abstient.
        self.assertEqual(mod_dev.discover_cargo_target(roots=[root], max_depth=6), [])
        self.assertTrue(target.is_dir())

    def test_stat_mtime_is_stamped_at_discovery(self) -> None:
        root = tempdir(self)
        target = cargo_project(root)
        found = mod_dev.discover_cargo_target(roots=[root], max_depth=6)
        self.assertIsNotNone(found[0].stat_mtime)
        self.assertAlmostEqual(found[0].stat_mtime, target.stat().st_mtime, places=3)


class TestDepthLimit(unittest.TestCase):
    """`--max-depth` compte les composants sous la racine."""

    def build(self, root: Path) -> Path:
        deep = root.joinpath("a", "b", "c", "d", "e", "f")  # Cargo.toml en 7
        write(deep / "Cargo.toml", 16)
        (deep / "target" / "release").mkdir(parents=True)
        return deep / "target"

    def test_depth_seven_is_out_of_reach_at_six(self) -> None:
        root = tempdir(self)
        self.build(root)
        self.assertEqual(mod_dev.discover_cargo_target(roots=[root], max_depth=6), [])

    def test_depth_seven_is_found_at_eight(self) -> None:
        root = tempdir(self)
        target = self.build(root)
        found = mod_dev.discover_cargo_target(roots=[root], max_depth=8)
        self.assertEqual([c.path for c in found], [str(target)])


class TestPycache(unittest.TestCase):
    def test_a_pycache_tree_is_sized(self) -> None:
        root = tempdir(self)
        cache = root / "paquet" / "__pycache__"
        write(cache / "module.cpython-313.pyc", 1200)
        write(cache / "autre.cpython-313.pyc", 800)
        found = mod_dev.discover_pycache(roots=[root], max_depth=6)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].estimated_bytes, 2000)
        self.assertEqual(found[0].path, str(cache))

    def test_nested_pycache_inside_a_pycache_is_not_listed_twice(self) -> None:
        root = tempdir(self)
        cache = root / "paquet" / "__pycache__"
        write(cache / "__pycache__" / "bizarre.pyc", 10)
        found = mod_dev.discover_pycache(roots=[root], max_depth=6)
        self.assertEqual([c.path for c in found], [str(cache)])

    def test_a_module_declaring_no_guard_never_queries_processes(self) -> None:
        # Contrôle observable de `proc_guard = None` : le module ne doit pas
        # exercer un garde qu'il n'a pas déclaré.
        root = tempdir(self)
        write(root / "paquet" / "__pycache__" / "m.pyc", 10)
        with mock.patch.object(procs, "is_running") as spy:
            mod_dev.discover_pycache(roots=[root], max_depth=6)
        spy.assert_not_called()


class TestDotnetBinObj(unittest.TestCase):
    def test_bin_and_obj_next_to_a_csproj(self) -> None:
        root = tempdir(self)
        project = root / "Appli"
        write(project / "Appli.csproj", 64)
        write(project / "bin" / "Debug" / "Appli.dll", 300)
        write(project / "obj" / "project.assets.json", 100)
        with mock.patch.object(procs, "is_running", return_value=set()):
            found = mod_dev.discover_dotnet_binobj(roots=[root], max_depth=6)
        self.assertEqual(
            sorted(c.path for c in found),
            sorted([str(project / "bin"), str(project / "obj")]),
        )

    def test_bin_without_a_csproj_is_left_alone(self) -> None:
        root = tempdir(self)
        write(root / "site" / "bin" / "cgi.exe", 10)
        with mock.patch.object(procs, "is_running", return_value=set()):
            self.assertEqual(mod_dev.discover_dotnet_binobj(roots=[root], max_depth=6), [])


class TestOwnerProcessWarning(unittest.TestCase):
    """`warn-and-skip` avertit puis omet ; `warn-only` avertit et agit."""

    def discover_with_cargo_running(self) -> CleanCandidate:
        root = tempdir(self)
        cargo_project(root)
        with mock.patch.object(procs, "is_running", return_value={"cargo.exe"}):
            found = mod_dev.discover_cargo_target(roots=[root], max_depth=6)
        self.assertEqual(len(found), 1)
        return found[0]

    def test_the_candidate_carries_the_warning_in_its_reason(self) -> None:
        candidate = self.discover_with_cargo_running()
        self.assertIn("cargo.exe", candidate.reason)

    def test_it_is_still_listed_in_a_dry_run_plan(self) -> None:
        candidate = self.discover_with_cargo_running()
        rendered = format_plan(Plan(candidates=[candidate], apply=False))
        self.assertIn(candidate.label, rendered)

    def test_under_apply_without_yes_it_is_skipped_as_running(self) -> None:
        candidate = self.discover_with_cargo_running()
        module = registry_mod.MODULES["cargo-target"]
        with mock.patch.object(procs, "is_running", return_value={"cargo.exe"}):
            skipped = registry_mod.proc_guard_skip(module, candidate, yes=False)
        self.assertIsNotNone(skipped)
        # Le jeton lui-même : une omission rangée sous `skipped-changed`
        # passerait une assertion « a-t-il été omis ? » en racontant autre chose.
        self.assertEqual(skipped.status, SKIP_RUNNING)
        self.assertIn("cargo.exe", skipped.reason)

    def test_yes_lifts_the_skip(self) -> None:
        candidate = self.discover_with_cargo_running()
        module = registry_mod.MODULES["cargo-target"]
        with mock.patch.object(procs, "is_running", return_value={"cargo.exe"}):
            self.assertIsNone(registry_mod.proc_guard_skip(module, candidate, yes=True))

    def test_unknown_process_state_skips_too(self) -> None:
        candidate = self.discover_with_cargo_running()
        module = registry_mod.MODULES["cargo-target"]
        with mock.patch.object(procs, "is_running", return_value=None):
            skipped = registry_mod.proc_guard_skip(module, candidate, yes=False)
        self.assertIsNotNone(skipped)
        self.assertEqual(skipped.status, SKIP_RUNNING)
        self.assertIn("inconnu", skipped.reason)

    def test_yes_acknowledges_the_unknown_state_exactly_as_a_running_owner(self) -> None:
        # Décision 4 : `--yes` a deux effets, pas trois. L'état inconnu est levé
        # par le même aveu que le propriétaire réellement actif.
        candidate = self.discover_with_cargo_running()
        module = registry_mod.MODULES["cargo-target"]
        with mock.patch.object(procs, "is_running", return_value=None):
            self.assertIsNone(registry_mod.proc_guard_skip(module, candidate, yes=True))

    def test_a_warn_only_module_is_warned_about_and_still_acted_upon(self) -> None:
        # Module factice : rien de `warn-only` n'est enregistré dans cette part,
        # et une force se prouve sans qu'un vrai chemin existe.
        candidate = CleanCandidate(
            module="factice-warn-only",
            path="C:\\dev\\factice\\cache",
            label="cache factice",
            estimated_bytes=10,
            level=Level.SAFE,
            reason="reconstruit - processus propriétaire actif : outil.exe",
        )
        module = CleanModule(
            name="factice-warn-only",
            level=Level.SAFE,
            requires=(),
            discover=lambda **_kw: [candidate],
            clean=None,
            discovery=DISCOVERY_FIXED,
            proc_guard=PROC_GUARD_WARN_ONLY,
            needs_network=False,
            opt_in=False,
        )
        with mock.patch.object(procs, "is_running", return_value={"outil.exe"}):
            self.assertIsNone(registry_mod.proc_guard_skip(module, candidate, yes=False))
        self.assertIn("outil.exe", candidate.reason)

    def test_the_two_strengths_do_not_collapse(self) -> None:
        # Même état de processus, même texte, verdicts opposés.
        state = ({"cargo.exe"}, ("cargo.exe",))
        candidate = CleanCandidate(
            module="x",
            path="C:\\dev\\x\\target",
            label="target x",
            estimated_bytes=1,
            level=Level.SAFE,
            reason="reconstruit",
        )
        strong = CleanModule(
            name="x",
            level=Level.SAFE,
            requires=(),
            discover=lambda **_kw: [],
            clean=None,
            discovery=DISCOVERY_FIXED,
            proc_guard=PROC_GUARD_WARN_AND_SKIP,
            needs_network=False,
            opt_in=False,
        )
        weak = dataclasses.replace(strong, proc_guard=PROC_GUARD_WARN_ONLY)
        self.assertIsNotNone(
            registry_mod.proc_guard_skip(strong, candidate, yes=False, state=state)
        )
        self.assertIsNone(registry_mod.proc_guard_skip(weak, candidate, yes=False, state=state))


@unittest.skipUnless(IS_WINDOWS, "résolution des caches Windows")
class TestCacheModules(unittest.TestCase):
    def test_the_tool_reported_path_wins_over_the_fallback(self) -> None:
        spec = mod_dev.CACHE_SPECS["npm-cache"]
        with mock.patch.object(mod_dev, "_ask_tool", return_value=Path("D:\\npm-cache")):
            resolved = mod_dev.resolve_cache_path(spec, env={"LOCALAPPDATA": "C:\\Local"})
        self.assertEqual(resolved, Path("D:\\npm-cache"))

    def test_the_environment_wins_over_the_fallback(self) -> None:
        spec = mod_dev.CACHE_SPECS["cargo-registry"]
        resolved = mod_dev.resolve_cache_path(
            spec, env={"CARGO_HOME": "D:\\cargo", "USERPROFILE": "C:\\Users\\x"}
        )
        self.assertEqual(resolved, Path("D:\\cargo\\registry"))

    def test_the_documented_fallback_is_used_last(self) -> None:
        spec = mod_dev.CACHE_SPECS["cargo-registry"]
        resolved = mod_dev.resolve_cache_path(spec, env={"USERPROFILE": "C:\\Users\\x"})
        self.assertEqual(resolved, Path("C:\\Users\\x\\.cargo\\registry"))

    def test_no_base_in_the_environment_yields_no_path(self) -> None:
        spec = mod_dev.CACHE_SPECS["cargo-registry"]
        self.assertIsNone(mod_dev.resolve_cache_path(spec, env={}))

    def test_an_absent_cache_directory_yields_no_candidate(self) -> None:
        root = tempdir(self)
        with mock.patch.object(mod_dev, "resolve_cache_path", return_value=root / "jamais"):
            self.assertEqual(mod_dev.discover_cache("npm-cache"), [])

    def test_an_existing_cache_directory_yields_one_sized_candidate(self) -> None:
        root = tempdir(self)
        write(root / "_cacache" / "index" / "aa", 700)
        with mock.patch.object(mod_dev, "resolve_cache_path", return_value=root):
            found = mod_dev.discover_cache("npm-cache")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].estimated_bytes, 700)
        self.assertEqual(found[0].module, "npm-cache")
        # `needs_network` n'est pas décidé ici : l'appelant de découverte
        # l'estampille depuis la déclaration du registre.
        self.assertFalse(found[0].needs_network)


class TestMissingRequirement(unittest.TestCase):
    def test_an_absent_required_binary_yields_nothing_and_says_nothing(self) -> None:
        called: list[int] = []
        module = CleanModule(
            name="factice-requires",
            level=Level.SAFE,
            requires=("winclean-binaire-inexistant",),
            discover=lambda **_kw: called.append(1) or [],
            clean=None,
            discovery=DISCOVERY_FIXED,
            proc_guard=None,
            needs_network=False,
            opt_in=False,
        )
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            found = registry_mod.discover_module(module)
        self.assertEqual(found, [])
        self.assertEqual(called, [])  # la découverte n'est même pas lancée
        self.assertEqual(out.getvalue(), "")
        self.assertEqual(err.getvalue(), "")  # outil absent n'est pas une anomalie


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
