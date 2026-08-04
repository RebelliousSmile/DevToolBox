"""Tests de l'orchestrateur : plan, gardes, application, sorties.

Les modules utilisés ici sont **factices** et injectés dans le registre. C'est
délibéré : les propriétés testées appartiennent au CLI (ordre de la chaîne, codes
de sortie, sections du rapport), et un module réel les rendrait dépendantes de ce
qui se trouve installé sur la machine.
"""

from __future__ import annotations

import contextlib
import dataclasses
import io
import json
import os
import subprocess
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import clean, guards, mod_apps, registry_mod  # noqa: E402
from scripts.winclean import config as config_mod  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    DISCOVERY_FIXED,
    DISCOVERY_WALKING,
    DROP_PROTECTED,
    DROP_SANITY,
    EXCLUDE_NEEDS_NETWORK,
    PROC_GUARD_WARN_AND_SKIP,
    PROC_GUARD_WARN_ONLY,
    RECYCLE_FOOTER_LINES,
    SKIP_CHANGED,
    SKIP_GONE,
    SKIP_NO_UNDO,
    CleanCandidate,
    CleanModule,
    CleanResult,
    Level,
    human_size,
)
from scripts.winclean.tests.test_mod_dev import tempdir as _tempdir  # noqa: E402
from scripts.winclean.tests.test_mod_dev import write  # noqa: E402


# --------------------------------------------------------------------------- #
# Outillage
# --------------------------------------------------------------------------- #


@contextlib.contextmanager
def tempdir(case: unittest.TestCase):
    """Même dossier jetable que `test_mod_dev`, en forme de bloc `with`."""
    yield _tempdir(case)


def candidate(
    module: str,
    path: str | os.PathLike[str] | None,
    size: int | None,
    *,
    label: str | None = None,
    mtime: float | None = -1.0,
    needs_network: bool = False,
) -> CleanCandidate:
    """Candidat de test. `mtime=-1.0` veut dire « celui du disque »."""
    text = None if path is None else str(path)
    stamped: float | None
    if mtime == -1.0:
        try:
            stamped = os.stat(text).st_mtime if text else None
        except OSError:
            stamped = None
    else:
        stamped = mtime
    return CleanCandidate(
        module=module,
        path=text,
        label=label or f"{module} cible",
        estimated_bytes=size,
        level=Level.SAFE,
        reason="candidat de test",
        needs_network=needs_network,
        stat_mtime=stamped,
    )


def fake_module(
    name: str,
    *,
    candidates: tuple[CleanCandidate, ...] = (),
    discover=None,
    clean_fn=None,
    level: Level = Level.SAFE,
    discovery: str = DISCOVERY_WALKING,
    proc_guard: str | None = None,
    needs_network: bool = False,
    requires: tuple[str, ...] = (),
) -> CleanModule:
    def _discover(**_kwargs: object) -> list[CleanCandidate]:
        return [dataclasses.replace(c) for c in candidates]

    return CleanModule(
        name=name,
        level=level,
        requires=requires,
        discover=discover or _discover,
        clean=clean_fn,
        discovery=discovery,
        proc_guard=proc_guard,
        needs_network=needs_network,
    )


@contextlib.contextmanager
def registry_of(*modules: CleanModule):
    """Remplace le registre par les modules donnés, ordre de possession compris."""
    mapping = {m.name: m for m in modules}
    with mock.patch.object(registry_mod, "MODULES", mapping), mock.patch.object(
        registry_mod, "MODULE_ORDER", tuple(mapping)
    ), mock.patch.object(registry_mod, "PROC_OWNERS", {}):
        yield mapping


def run_cli(argv: list[str]) -> tuple[int, str, str]:
    """Lance le CLI, sans jamais lire le fichier de configuration de la machine.

    La recherche à l'emplacement par défaut est neutralisée ici, une fois pour
    toute la suite : sinon un `%APPDATA%\\winclean\\winclean.json` présent chez un
    développeur changerait le plafond, la liste protégée ou les modules
    sélectionnés de chaque test. Un test qui *veut* une configuration la nomme
    avec `--config`, ce que ce contournement laisse intact.
    """
    out, err = io.StringIO(), io.StringIO()
    with mock.patch.object(config_mod, "default_config_path", lambda env=None: None):
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = clean.main(argv)
    return code, out.getvalue(), err.getvalue()


@contextlib.contextmanager
def deletion_spy():
    """Espion sur `delete_tree`, qui délègue à la vraie suppression."""
    real = clean.remove.delete_tree
    calls: list[str] = []

    def _spy(path):
        calls.append(str(path))
        return real(path)

    with mock.patch.object(clean.remove, "delete_tree", _spy):
        yield calls


@contextlib.contextmanager
def recycle_stub(*, ok: bool = True, can: bool = True):
    """Corbeille simulée. Rend les deux journaux d'appels, `can` et `recycle`.

    Simulée et non réelle : `SHFileOperationW` déplacerait le fixture dans la
    corbeille de la machine qui exécute la suite, où il resterait.
    """
    calls: dict[str, list[str]] = {"can": [], "recycle": []}

    def _can(path):
        calls["can"].append(str(path))
        return can

    def _recycle(path):
        calls["recycle"].append(str(path))
        if ok:
            return clean.remove.RecycleOutcome(ok=True, code=0)
        return clean.remove.RecycleOutcome(
            ok=False,
            code=0x78,
            reason=clean.remove.RECYCLE_FAILED,
            detail="le shell a refusé le déplacement",
            errors=[
                clean.remove.RemovalError(
                    path=str(path),
                    winerror=None,
                    message="la corbeille a refusé ce chemin",
                )
            ],
        )

    with mock.patch.object(clean.remove, "can_recycle", _can), mock.patch.object(
        clean.remove, "recycle", _recycle
    ):
        yield calls


@contextlib.contextmanager
def no_tty():
    """`stdin` sans terminal : `isatty()` rend faux, `readline()` ne bloque pas."""
    with mock.patch.object(clean.sys, "stdin", io.StringIO()):
        yield


@contextlib.contextmanager
def docker_stub(*, prune_code: int = 0, prune_stdout: str = ""):
    """Les deux appels `docker` du module, journalisés et sans démon.

    La sonde (`system df`) réussit toujours — sans elle le module ne découvre
    rien et le test passerait pour la mauvaise raison — et le `prune` rend ce que
    le cas demande.
    """
    calls: list[tuple[str, ...]] = []

    def _run(command):
        vector = tuple(command)
        calls.append(vector)
        if vector == mod_apps.DOCKER_PRUNE_COMMAND:
            return subprocess.CompletedProcess(list(command), prune_code, prune_stdout, "")
        return subprocess.CompletedProcess(list(command), 0, "{}", "")

    with mock.patch.object(mod_apps, "_run", _run), mock.patch.object(
        mod_apps.shutil, "which", return_value="docker.exe"
    ):
        yield calls


def usage(total: int) -> SimpleNamespace:
    """Ce que `shutil.disk_usage` rend, réduit au champ que le code lit."""
    return SimpleNamespace(total=total, used=0, free=total)


# --------------------------------------------------------------------------- #
# Analyse des arguments
# --------------------------------------------------------------------------- #


class TestParser(unittest.TestCase):
    def test_check_flag_does_not_exist(self):
        """`--check` n'est pas repris de deps_audit : un plan winclean n'est jamais vide."""
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            with self.assertRaises(SystemExit) as raised:
                clean.build_parser().parse_args(["--check"])
        self.assertEqual(raised.exception.code, 2)
        self.assertIn("--check", err.getvalue())

    def test_frozen_defaults(self):
        args = clean.build_parser().parse_args([])
        self.assertEqual(args.level, "safe")
        self.assertEqual(args.max_depth, 6)
        self.assertEqual(args.max_delete_bytes, 50 * 1024**3)
        self.assertFalse(args.apply)
        self.assertFalse(args.recycle)
        self.assertFalse(args.offline)
        self.assertIsNone(args.top)
        self.assertIsNone(args.out)

    def test_recycle_pair(self):
        parser = clean.build_parser()
        self.assertTrue(parser.parse_args(["--recycle"]).recycle)
        self.assertFalse(parser.parse_args(["--recycle", "--no-recycle"]).recycle)

    def test_parse_size(self):
        self.assertEqual(clean.parse_size("1024"), 1024)
        self.assertEqual(clean.parse_size("50GiB"), 50 * 1024**3)
        self.assertEqual(clean.parse_size("500m"), 500 * 1024**2)
        with self.assertRaises(Exception):
            clean.parse_size("beaucoup")

    def test_only_accepts_repetition_and_commas(self):
        args = clean.build_parser().parse_args(["--only", "a,b", "--only", "c"])
        self.assertEqual(clean._split_names(args.only), ["a", "b", "c"])


# --------------------------------------------------------------------------- #
# Racines
# --------------------------------------------------------------------------- #


class TestRoots(unittest.TestCase):
    def test_default_root_is_derived_from_file_not_cwd(self):
        """Racine par défaut = racine du dépôt, **cwd posé sur le paquet**.

        C'est ce déplacement du répertoire courant qui distingue une racine
        dérivée de `Path(__file__)` d'une racine dérivée de `Path.cwd()` : les
        deux coïncident quand l'outil est lancé depuis la racine du dépôt, donc
        un test qui n'en sort pas passe aussi sur la mauvaise implémentation.
        C'est aussi la valeur que le lanceur fournit réellement.

        Note de conformité au plan : la seconde moitié du critère (« aucune
        racine sous %USERPROFILE%\\Documents ») n'est pas assertable ici — ce
        dépôt *est* sous `%USERPROFILE%\\Documents`, donc les deux moitiés du
        critère se contredisent sur cette machine. Signalé comme `replan needed`
        plutôt que contourné.
        """
        package_dir = Path(clean.__file__).resolve().parent
        previous = os.getcwd()
        os.chdir(package_dir)
        try:
            roots = clean.resolve_roots([])
        finally:
            os.chdir(previous)
        self.assertEqual(roots, [_REPO_ROOT])
        self.assertNotEqual(roots, [package_dir])

    def test_explicit_roots_are_resolved_and_deduplicated(self):
        with tempdir(self) as tmp:
            roots = clean.resolve_roots([str(tmp), str(tmp) + os.sep, str(tmp / "." )])
            self.assertEqual(roots, [Path(tmp).resolve()])

    def test_sync_rule_matches_components_not_a_fixed_list(self):
        self.assertTrue(clean.is_sync_root(Path(r"C:\Users\x\OneDrive - Acme\p"), env={}))
        self.assertTrue(clean.is_sync_root(Path(r"C:\Users\x\Dropbox\p"), env={}))
        self.assertTrue(clean.is_sync_root(Path(r"C:\Users\x\Documents\Perso\p"), env={}))
        self.assertFalse(clean.is_sync_root(Path(r"C:\dev\projet"), env={}))

    def test_sync_rule_reads_the_environment_too(self):
        self.assertTrue(
            clean.is_sync_root(
                Path(r"C:\Sync\Entreprise\p"), env={"OneDriveCommercial": r"C:\Sync\Entreprise"}
            )
        )

    def test_sync_root_warns_and_the_run_proceeds(self):
        with tempdir(self) as tmp:
            fixture = tmp / "Documents" / "Perso" / "projet"
            fixture.mkdir(parents=True)
            module = fake_module("factice")
            with registry_of(module):
                code, out, _err = run_cli(["--root", str(fixture)])
        self.assertEqual(code, clean.EXIT_OK)
        self.assertIn("Racine synchronisée dans le cloud", out)
        self.assertIn("Simulation", out)


# --------------------------------------------------------------------------- #
# Chaîne de gardes : racines vs candidats (critère de la Phase 1)
# --------------------------------------------------------------------------- #


class TestChainScreensCandidatesNotRoots(unittest.TestCase):
    def test_a_short_root_is_legitimate(self):
        """`--root C:\\dev` mesure moins de 10 caractères et doit passer."""
        module = fake_module("factice")
        with registry_of(module):
            code, out, err = run_cli(["--root", r"C:\dev"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn(r"C:\dev", out)

    def test_a_profile_root_candidate_is_rejected(self):
        profile = os.environ["USERPROFILE"]
        module = fake_module("factice", candidates=(candidate("factice", profile, 0, mtime=None),))
        with registry_of(module):
            code, out, err = run_cli([])
        self.assertNotEqual(code, clean.EXIT_OK)
        self.assertEqual(code, clean.EXIT_SANITY)
        self.assertIn(DROP_SANITY, out)
        self.assertIn("factice", err)


# --------------------------------------------------------------------------- #
# Simulation
# --------------------------------------------------------------------------- #


class TestDryRun(unittest.TestCase):
    def test_no_apply_deletes_nothing(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 4096)
            module = fake_module("factice", candidates=(candidate("factice", target, 4096),))
            with registry_of(module), deletion_spy() as calls:
                code, out, err = run_cli(["--root", str(tmp)])
            self.assertEqual(code, clean.EXIT_OK, err)
            self.assertEqual(calls, [])
            self.assertTrue((target / "a.bin").exists())
            self.assertEqual((target / "a.bin").stat().st_size, 4096)
            self.assertIn("Simulation : rien n'a été supprimé", out)

    def test_json_payload_is_one_document_with_safe_candidates(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 100)
            module = fake_module("factice", candidates=(candidate("factice", target, 100),))
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--json"])
        self.assertEqual(code, clean.EXIT_OK, err)
        payload = json.loads(out)
        self.assertEqual(payload["level"], "safe")
        self.assertTrue(payload["candidates"])
        for entry in payload["candidates"]:
            self.assertEqual(entry["level"], "safe")

    def test_free_space_is_stated_before_and_after(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 2048)
            module = fake_module("factice", candidates=(candidate("factice", target, 2048),))
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Espace libre sur", out)
        self.assertIn("après opération", out)

    def test_columns_the_loop_owns_are_zero_not_unknown(self):
        """`safe` supprime en direct : « rien en corbeille » est mesuré, pas ignoré."""
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 8)
            module = fake_module("factice", candidates=(candidate("factice", target, 8),))
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes", "--json"])
        self.assertEqual(code, clean.EXIT_OK, err)
        result = json.loads(out)["run"]["results"][0]
        self.assertEqual(result["freed"], 8)
        self.assertEqual(result["recycled"], 0)
        self.assertEqual(result["failed"], 0)

    def test_a_pathless_module_keeps_what_it_does_not_know(self):
        """Ce que le `clean()` d'un module ne chiffre pas reste `null`."""

        def _clean(**_kwargs):
            return CleanResult(module="sans-chemin", estimated=None, freed=None)

        module = fake_module(
            "sans-chemin",
            candidates=(candidate("sans-chemin", None, None, label="volumes"),),
            clean_fn=_clean,
        )
        with registry_of(module):
            code, out, err = run_cli(["--apply", "--yes", "--json"])
        self.assertEqual(code, clean.EXIT_OK, err)
        result = json.loads(out)["run"]["results"][0]
        self.assertIsNone(result["freed"])
        self.assertIsNone(result["recycled"])
        self.assertIsNone(result["failed"])

    def test_an_empty_plan_announces_zero_not_unknown(self):
        module = fake_module("factice")
        with registry_of(module):
            code, out, err = run_cli([])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Aucun élément trouvé", out)
        self.assertIn(f"Total estimé : {human_size(0)} sur 0 élément(s)", out)
        self.assertNotIn("Total estimé : unknown", out)

    def test_a_volume_is_announced_once_whatever_the_case(self):
        """Les modules produisent la casse qu'ils veulent ; le volume reste un."""
        with tempdir(self) as tmp:
            lower = tmp / "bas"
            upper = tmp / "haut"
            lower.mkdir()
            upper.mkdir()
            write(lower / "a.bin", 16)
            write(upper / "a.bin", 16)
            module = fake_module(
                "factice",
                candidates=(
                    candidate("factice", str(lower).lower(), 16, label="bas"),
                    candidate("factice", str(upper).upper(), 16, label="haut"),
                ),
            )
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp)])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertEqual(out.count("Espace libre sur"), 1)


# --------------------------------------------------------------------------- #
# Sections du plan
# --------------------------------------------------------------------------- #


class TestGuardSections(unittest.TestCase):
    def test_protected_candidate_is_reported_and_the_run_succeeds(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 512)
            module = fake_module("factice", candidates=(candidate("factice", target, 512),))
            protected = (guards.normalise(tmp),)
            with registry_of(module), mock.patch.object(
                guards, "DEFAULT_PROTECTED", protected
            ), deletion_spy() as calls:
                code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes"])
                _code, json_out, _err = run_cli(["--root", str(tmp), "--json"])
            self.assertEqual(code, clean.EXIT_OK, err)
            self.assertEqual(calls, [])
            self.assertTrue(target.exists())
            self.assertIn(DROP_PROTECTED, out)
            dropped = json.loads(json_out)["dropped"]
            self.assertEqual([d["reason_class"] for d in dropped], [DROP_PROTECTED])

    def test_sanity_candidate_aborts_before_any_deletion(self):
        module = fake_module("defectueux", candidates=(candidate("defectueux", "C:\\", 0, mtime=None),))
        with registry_of(module), deletion_spy() as calls:
            code, out, err = run_cli(["--apply", "--yes"])
            _code, json_out, _err = run_cli(["--json"])
        self.assertEqual(code, clean.EXIT_SANITY)
        self.assertEqual(calls, [])
        self.assertIn("defectueux", err)
        self.assertIn(DROP_SANITY, out)
        dropped = json.loads(json_out)["dropped"]
        self.assertEqual([d["reason_class"] for d in dropped], [DROP_SANITY])

    def test_absorbed_candidate_names_its_ancestor(self):
        with tempdir(self) as tmp:
            ancestor = tmp / "target"
            nested = ancestor / "sous" / "__pycache__"
            nested.mkdir(parents=True)
            write(nested / "a.pyc", 256)
            outer = fake_module("exterieur", candidates=(candidate("exterieur", ancestor, 256),))
            inner = fake_module("interieur", candidates=(candidate("interieur", nested, 256),))
            with registry_of(outer, inner):
                code, out, err = run_cli(["--root", str(tmp)])
                _code, json_out, _err = run_cli(["--root", str(tmp), "--json"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Absorbés par un ancêtre", out)
        self.assertIn("absorbé par", out)
        absorbed = json.loads(json_out)["absorbed"]
        self.assertEqual(len(absorbed), 1)
        self.assertEqual(absorbed[0]["module"], "interieur")
        self.assertEqual(absorbed[0]["ancestor_module"], "exterieur")
        self.assertEqual(json.loads(json_out)["total_estimated_bytes"], 256)


# --------------------------------------------------------------------------- #
# Plafond
# --------------------------------------------------------------------------- #


class TestCeiling(unittest.TestCase):
    def test_over_ceiling_aborts_naming_total_limit_and_knob(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 5000)
            module = fake_module("factice", candidates=(candidate("factice", target, 5000),))
            with registry_of(module), deletion_spy() as calls:
                code, _out, err = run_cli(
                    ["--root", str(tmp), "--apply", "--yes", "--max-delete-bytes", "1000"]
                )
            self.assertEqual(code, clean.EXIT_CEILING)
            self.assertEqual(calls, [])
            self.assertTrue(target.exists())
        self.assertIn("5000", err)
        self.assertIn("1000", err)
        self.assertIn("--max-delete-bytes", err)


# --------------------------------------------------------------------------- #
# Application : gardes TOCTOU
# --------------------------------------------------------------------------- #


class TestApplyToctou(unittest.TestCase):
    def test_changed_candidate_is_skipped_and_survives(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 128)
            stale = candidate("factice", target, 128, mtime=os.stat(target).st_mtime - 500)
            module = fake_module("factice", candidates=(stale,))
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes"])
            self.assertEqual(code, clean.EXIT_OK, err)
            self.assertIn(SKIP_CHANGED, out)
            self.assertTrue(target.exists())

    def test_vanished_candidate_is_skipped_without_raising(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 128)
            entry = candidate("factice", target, 128)
            clean.remove.delete_tree(target)
            module = fake_module("factice", candidates=(entry,))
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes"])
            self.assertEqual(code, clean.EXIT_OK, err)
            self.assertIn(SKIP_GONE, out)

    def test_unstamped_candidate_is_deleted(self):
        """`stat_mtime=None` : le garde ne s'applique pas, il n'omet pas tout."""
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 128)
            module = fake_module(
                "factice", candidates=(candidate("factice", target, 128, mtime=None),)
            )
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes"])
            self.assertEqual(code, clean.EXIT_OK, err)
            self.assertNotIn(SKIP_CHANGED, out)
            self.assertFalse(target.exists())


# --------------------------------------------------------------------------- #
# Application : fichier verrouillé
# --------------------------------------------------------------------------- #


class TestFreedCountsUnlinkedBytes(unittest.TestCase):
    def test_a_locked_file_is_failed_and_not_counted_as_freed(self):
        """`freed` = octets réellement déliés, pas un avant/après du répertoire.

        Les deux divergent d'exactement les octets verrouillés : c'est ce qui
        désigne laquelle des deux sources est utilisée.
        """
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "libre.bin", 1000)
            write(target / "verrouille.bin", 500)
            module = fake_module("factice", candidates=(candidate("factice", target, 1500),))
            handle = open(target / "verrouille.bin", "rb")
            try:
                with registry_of(module):
                    code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes", "--json"])
            finally:
                handle.close()
        self.assertEqual(code, clean.EXIT_OK, err)
        result = json.loads(out)["run"]["results"][0]
        self.assertEqual(result["estimated"], 1500)
        self.assertEqual(result["freed"], 1000)
        self.assertEqual(result["failed"], 500)


# --------------------------------------------------------------------------- #
# Application : candidat sans chemin
# --------------------------------------------------------------------------- #


class TestPathlessApply(unittest.TestCase):
    def test_pathless_candidate_goes_to_the_module_clean(self):
        calls: list[dict] = []
        stats: list[object] = []

        def _clean(**kwargs):
            calls.append(kwargs)
            return CleanResult(module="sans-chemin", estimated=1000, freed=1000)

        module = fake_module(
            "sans-chemin",
            candidates=(candidate("sans-chemin", None, 1000, label="volumes docker"),),
            clean_fn=_clean,
        )
        real_stat = os.stat

        def _spy(path, *args, **kwargs):
            stats.append(path)
            return real_stat(path, *args, **kwargs)

        with registry_of(module), mock.patch.object(clean.os, "stat", _spy), deletion_spy() as deletions:
            code, out, err = run_cli(["--apply", "--yes"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertEqual(len(calls), 1)
        self.assertEqual(stats, [])
        self.assertEqual(deletions, [])
        self.assertNotIn(SKIP_GONE, out)
        self.assertNotIn(SKIP_CHANGED, out)
        self.assertIn(human_size(1000), out)


# --------------------------------------------------------------------------- #
# Application : interruption
# --------------------------------------------------------------------------- #


class TestInterrupted(unittest.TestCase):
    def test_report_comes_from_the_finally_block(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 4096)

            def _boom(**_kwargs):
                raise KeyboardInterrupt

            first = fake_module("premier", candidates=(candidate("premier", target, 4096),))
            second = fake_module(
                "second",
                candidates=(candidate("second", None, None, label="interruption"),),
                clean_fn=_boom,
            )
            with registry_of(first, second):
                code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes"])
            self.assertEqual(code, clean.EXIT_INTERRUPTED, err)
            self.assertIn("Résultat par module", out)
            self.assertIn("Run interrompu", out)
            self.assertIn(human_size(4096), out)
            self.assertFalse(target.exists())


# --------------------------------------------------------------------------- #
# `--offline`
# --------------------------------------------------------------------------- #


class TestOffline(unittest.TestCase):
    def _fixture(self, tmp: Path) -> tuple[CleanModule, CleanModule]:
        local = tmp / "local"
        remote = tmp / "cache"
        local.mkdir()
        remote.mkdir()
        write(local / "a.bin", 300)
        write(remote / "b.bin", 700)
        rebuilt = fake_module("local", candidates=(candidate("local", local, 300),))
        refilled = fake_module(
            "cache",
            candidates=(candidate("cache", remote, 700),),
            needs_network=True,
            discovery=DISCOVERY_FIXED,
        )
        return rebuilt, refilled

    def test_without_offline_they_are_listed_and_flagged(self):
        with tempdir(self) as tmp:
            with registry_of(*self._fixture(tmp)):
                code, out, err = run_cli(["--root", str(tmp)])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("réseau requis pour reconstituer", out)
        self.assertIn("Par utilisateur, hors de vos racines", out)
        self.assertIn("Sous vos racines", out)

    def test_offline_excludes_them_into_their_own_section(self):
        with tempdir(self) as tmp:
            with registry_of(*self._fixture(tmp)):
                code, out, err = run_cli(["--root", str(tmp), "--offline"])
                _code, json_out, _err = run_cli(["--root", str(tmp), "--offline", "--json"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Exclus par --offline", out)
        payload = json.loads(json_out)
        self.assertEqual([c["module"] for c in payload["candidates"]], ["local"])
        self.assertEqual(len(payload["excluded"]), 1)
        self.assertEqual(payload["excluded"][0]["module"], "cache")
        self.assertEqual(payload["excluded"][0]["estimated_bytes"], 700)
        self.assertEqual(payload["excluded"][0]["reason"], EXCLUDE_NEEDS_NETWORK)
        self.assertEqual(payload["total_estimated_bytes"], 300)


# --------------------------------------------------------------------------- #
# `--out`
# --------------------------------------------------------------------------- #


class TestOut(unittest.TestCase):
    def test_out_writes_text_creates_parents_and_overwrites(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 64)
            destination = tmp / "rapports" / "sous" / "plan.txt"
            module = fake_module("factice", candidates=(candidate("factice", target, 64),))
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--out", str(destination)])
                self.assertEqual(code, clean.EXIT_OK, err)
                self.assertTrue(destination.exists())
                first = destination.read_text(encoding="utf-8")
                self.assertIn("Niveau : safe", first)
                self.assertIn("Niveau : safe", out)
                run_cli(["--root", str(tmp), "--out", str(destination)])
            second = destination.read_text(encoding="utf-8")
            self.assertEqual(second.count("Niveau : safe"), 1)

    def test_json_out_is_valid_json(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 64)
            destination = tmp / "plan.json"
            module = fake_module("factice", candidates=(candidate("factice", target, 64),))
            with registry_of(module):
                code, out, err = run_cli(
                    ["--root", str(tmp), "--json", "--out", str(destination)]
                )
            self.assertEqual(code, clean.EXIT_OK, err)
            payload = json.loads(destination.read_text(encoding="utf-8"))
            self.assertEqual(payload["level"], "safe")
            self.assertEqual(json.loads(out)["level"], "safe")


# --------------------------------------------------------------------------- #
# `--top`
# --------------------------------------------------------------------------- #


class TestTop(unittest.TestCase):
    def _five(self, tmp: Path) -> tuple[CleanModule, list[Path]]:
        paths: list[Path] = []
        entries: list[CleanCandidate] = []
        for index, size in enumerate((5000, 4000, 3000, 2000, 1000), start=1):
            directory = tmp / f"cible{index}"
            directory.mkdir()
            write(directory / "a.bin", size)
            paths.append(directory)
            entries.append(candidate("factice", directory, size, label=f"cible{index}"))
        return fake_module("factice", candidates=tuple(entries)), paths

    def test_top_truncates_the_display_only(self):
        with tempdir(self) as tmp:
            module, paths = self._five(tmp)
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--top", "2"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("cible1", out)
        self.assertIn("cible2", out)
        for hidden in ("cible3", "cible4", "cible5"):
            self.assertNotIn(hidden, out)
        self.assertIn("3 ligne(s) masquée(s)", out)
        self.assertIn(human_size(15000), out)
        self.assertIn("sur 5 élément(s)", out)

    def test_top_never_narrows_the_deletion_set(self):
        with tempdir(self) as tmp:
            module, paths = self._five(tmp)
            with registry_of(module):
                code, _out, err = run_cli(["--root", str(tmp), "--apply", "--yes", "--top", "2"])
            self.assertEqual(code, clean.EXIT_OK, err)
            for path in paths:
                self.assertFalse(path.exists(), f"{path} devait être supprimé")


# --------------------------------------------------------------------------- #
# `--recycle` au niveau safe
# --------------------------------------------------------------------------- #


class TestRecycleAtSafe(unittest.TestCase):
    def test_recycle_is_accepted_and_declared_inert(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 32)
            module = fake_module("factice", candidates=(candidate("factice", target, 32),))
            with registry_of(module), deletion_spy() as calls:
                code, out, err = run_cli(["--root", str(tmp), "--recycle"])
            self.assertEqual(code, clean.EXIT_OK, err)
            self.assertEqual(calls, [])
            self.assertTrue(target.exists())
            self.assertIn("--recycle est accepté mais sans effet", out)

    def test_recycle_is_not_used_at_safe_under_apply(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 32)
            module = fake_module("factice", candidates=(candidate("factice", target, 32),))
            recycled: list[str] = []
            with registry_of(module), mock.patch.object(
                clean.remove, "recycle", lambda path: recycled.append(str(path))
            ):
                code, _out, err = run_cli(["--root", str(tmp), "--apply", "--yes", "--recycle"])
            self.assertEqual(code, clean.EXIT_OK, err)
            self.assertEqual(recycled, [])
            self.assertFalse(target.exists())


# --------------------------------------------------------------------------- #
# Validation
# --------------------------------------------------------------------------- #


class TestValidationExits(unittest.TestCase):
    def test_unknown_only_aborts_before_discovery(self):
        seen: list[str] = []

        def _discover(**_kwargs):
            seen.append("appelé")
            return []

        module = fake_module("pycache", discover=_discover)
        with registry_of(module):
            code, out, err = run_cli(["--only", "nmp"])
        self.assertEqual(code, clean.EXIT_VALIDATION)
        self.assertIn("module inconnu", err)
        self.assertIn("pycache", err)
        self.assertEqual(seen, [])
        self.assertEqual(out, "")

    def test_only_then_skip_selects_nothing_and_exits_zero(self):
        seen: list[str] = []

        def _discover(**_kwargs):
            seen.append("appelé")
            return []

        module = fake_module("pycache", discover=_discover)
        with registry_of(module):
            code, out, err = run_cli(["--only", "pycache", "--skip", "pycache"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertEqual(seen, [])
        self.assertIn("Aucun élément trouvé", out)

    def test_a_later_parts_module_name_reaches_nothing(self):
        """`--only recycle-bin` : inconnu ici, donc refusé avant toute découverte."""
        with deletion_spy() as calls:
            code, _out, err = run_cli(["--apply", "--only", "recycle-bin"])
        self.assertEqual(code, clean.EXIT_VALIDATION)
        self.assertEqual(calls, [])
        self.assertIn("recycle-bin", err)
        self.assertIn("noms valides", err)


# --------------------------------------------------------------------------- #
# Portée de `--root` et garde de processus
# --------------------------------------------------------------------------- #


class TestRootScope(unittest.TestCase):
    def test_fixed_modules_ignore_root_and_are_listed_apart(self):
        with tempdir(self) as tmp:
            elsewhere = tmp / "ailleurs"
            elsewhere.mkdir()
            write(elsewhere / "a.bin", 900)
            walking = fake_module("marcheur")  # ne trouve rien hors de <tmp>
            fixed = fake_module(
                "par-utilisateur",
                candidates=(candidate("par-utilisateur", elsewhere, 900),),
                discovery=DISCOVERY_FIXED,
            )
            with tempdir(self) as other:
                with registry_of(walking, fixed):
                    code, out, err = run_cli(["--root", str(other)])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Par utilisateur, hors de vos racines", out)
        self.assertIn("par-utilisateur", out)


class TestProcessGuardUnderApply(unittest.TestCase):
    def test_a_running_owner_skips_the_candidate_once_per_module(self):
        with tempdir(self) as tmp:
            target = tmp / "cible"
            target.mkdir()
            write(target / "a.bin", 200)
            module = fake_module(
                "occupe",
                candidates=(candidate("occupe", target, 200),),
                proc_guard=PROC_GUARD_WARN_AND_SKIP,
            )
            queries: list[tuple[str, ...]] = []

            def _is_running(owners):
                queries.append(tuple(owners))
                return {"cargo.exe"}

            mapping = {m.name: m for m in (module,)}
            with mock.patch.object(registry_mod, "MODULES", mapping), mock.patch.object(
                registry_mod, "MODULE_ORDER", tuple(mapping)
            ), mock.patch.object(
                registry_mod, "PROC_OWNERS", {"occupe": ("cargo.exe",)}
            ), mock.patch.object(
                registry_mod.procs, "is_running", _is_running
            ):
                code, out, err = run_cli(["--root", str(tmp), "--apply"])
            self.assertEqual(code, clean.EXIT_OK, err)
            self.assertIn("skipped-running", out)
            self.assertTrue(target.exists())
            self.assertEqual(len(queries), 1)


# --------------------------------------------------------------------------- #
# Garde de plateforme
# --------------------------------------------------------------------------- #


class TestPlatformGuard(unittest.TestCase):
    def test_non_windows_is_refused(self):
        with self.assertRaises(clean.PlatformError):
            clean.ensure_windows("linux")
        with mock.patch.object(clean.sys, "platform", "linux"):
            code, _out, err = run_cli([])
        self.assertEqual(code, clean.EXIT_PLATFORM)
        self.assertIn("Windows", err)


# --------------------------------------------------------------------------- #
# Classification de découverte, lue et non écrite en dur
# --------------------------------------------------------------------------- #


class TestDiscoveryClassificationIsRead(unittest.TestCase):
    def test_the_cli_holds_no_module_name(self):
        source = Path(clean.__file__).read_text(encoding="utf-8")
        for name in registry_mod.MODULE_ORDER:
            self.assertNotIn(f'"{name}"', source, f"clean.py nomme le module {name}")
            self.assertNotIn(f"'{name}'", source, f"clean.py nomme le module {name}")


# --------------------------------------------------------------------------- #
# Portail de confirmation (Part 2 Phase 3, tâche 1)
# --------------------------------------------------------------------------- #


def moderate_module(name: str, target, size: int, **kwargs) -> CleanModule:
    """Module `moderate` porteur d'un chemin, pour les tests de cette phase."""
    return fake_module(
        name,
        candidates=(candidate(name, target, size),),
        level=Level.MODERATE,
        discovery=DISCOVERY_FIXED,
        **kwargs,
    )


class TestConfirmationGate(unittest.TestCase):
    def test_without_a_tty_and_without_yes_it_aborts_having_deleted_nothing(self):
        """Sans terminal et sans `--yes`, la réponse est non (critère 172)."""
        with tempdir(self) as tmp:
            target = tmp / "cache"
            target.mkdir()
            write(target / "a.bin", 400)
            module = moderate_module("modere", target, 400)
            with registry_of(module), no_tty(), deletion_spy() as deletions:
                code, out, err = run_cli(
                    ["--root", str(tmp), "--level", "moderate", "--apply"]
                )
        self.assertEqual(code, clean.EXIT_VALIDATION)
        self.assertIn("Confirmation absente", err)
        self.assertEqual(deletions, [])
        self.assertTrue(target.exists())

    def test_safe_never_asks_and_moderate_reads_the_answer(self):
        self.assertTrue(clean.confirm_level(Level.SAFE, yes=False, stream=io.StringIO()))
        for answer, expected in (("oui\n", True), ("o\n", True), ("non\n", False), ("", False)):
            stream = io.StringIO(answer)
            stream.isatty = lambda: True  # type: ignore[method-assign]
            with self.subTest(answer=answer):
                buffer = io.StringIO()
                with contextlib.redirect_stdout(buffer):
                    got = clean.confirm_level(Level.MODERATE, yes=False, stream=stream)
                self.assertIs(got, expected)
                self.assertIn("confirmer", buffer.getvalue())

    def test_yes_answers_in_advance_without_reading_the_stream(self):
        stream = io.StringIO("non\n")
        stream.isatty = lambda: True  # type: ignore[method-assign]
        self.assertTrue(clean.confirm_level(Level.MODERATE, yes=True, stream=stream))
        self.assertEqual(stream.tell(), 0)  # rien n'a été lu


class TestLevelGateFromTheCommandLine(unittest.TestCase):
    def test_only_a_moderate_module_without_the_level_aborts_before_prompting(self):
        """Critère 173 : `--only` ne remplace pas `--level` (décision 12)."""
        with tempdir(self) as tmp:
            target = tmp / "cache"
            target.mkdir()
            write(target / "a.bin", 100)
            module = moderate_module("modere", target, 100)
            asked: list[str] = []

            def _confirm(level, **_kw):
                asked.append(str(level))
                return True

            with registry_of(module), mock.patch.object(
                clean, "confirm_level", _confirm
            ), deletion_spy() as deletions:
                code, _out, err = run_cli(
                    ["--root", str(tmp), "--apply", "--only", "modere"]
                )
        self.assertEqual(code, clean.EXIT_VALIDATION)
        self.assertIn("--level moderate", err)
        self.assertEqual(asked, [])  # la question n'a jamais été posée
        self.assertEqual(deletions, [])
        self.assertTrue(target.exists())


# --------------------------------------------------------------------------- #
# Les deux classes de défaillance derrière la colonne `failed`
# --------------------------------------------------------------------------- #


class TestFailureClasses(unittest.TestCase):
    def _run_with_error(self, error: object, argv: list[str] | None = None):
        with tempdir(self) as tmp:
            target = tmp / "cache"
            target.mkdir()
            write(target / "a.bin", 500)
            module = fake_module(
                "factice",
                candidates=(candidate("factice", target, 500),),
                discovery=DISCOVERY_FIXED,
            )

            def _delete(path):
                return (0, 500, [dataclasses.replace(error, path=str(path))])

            with registry_of(module), mock.patch.object(
                clean.remove, "delete_tree", _delete
            ):
                return (*run_cli(["--root", str(tmp), "--apply", *(argv or [])]), str(target))

    def test_a_share_lock_keeps_the_exit_code_at_zero_and_names_the_path(self):
        """Critère 174, première moitié."""
        code, out, err, path = self._run_with_error(
            clean.remove.RemovalError(path="", winerror=32, message="fichier en cours d'utilisation")
        )
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Verrouillés", out)
        self.assertIn(path, out)

    def test_any_other_oserror_makes_the_run_non_zero(self):
        """Critère 174, seconde moitié : `winerror 5` n'est pas un verrou."""
        code, out, err, path = self._run_with_error(
            clean.remove.RemovalError(path="", winerror=5, message="accès refusé")
        )
        self.assertEqual(code, clean.EXIT_REMOVAL)
        self.assertIn("Échec de suppression", err)
        self.assertNotIn("Verrouillés", out)
        self.assertIn(path, err)

    def test_a_share_lock_is_carried_by_locked_paths_in_the_payload(self):
        code, out, err, path = self._run_with_error(
            clean.remove.RemovalError(path="", winerror=32, message="fichier en cours d'utilisation"),
            ["--json"],
        )
        self.assertEqual(code, clean.EXIT_OK, err)
        result = json.loads(out)["run"]["results"][0]
        self.assertEqual(result["locked_paths"], [path])
        self.assertEqual(result["recycle_failed_paths"], [])
        self.assertEqual(result["failed"], 500)

    def _recycle_failure(self, argv: list[str]):
        tmp = _tempdir(self)
        target = tmp / "cache"
        target.mkdir()
        write(target / "a.bin", 700)
        module = moderate_module("modere", target, 700)
        with registry_of(module), recycle_stub(ok=False), deletion_spy() as deletions:
            code, out, err = run_cli(
                ["--root", str(tmp), "--level", "moderate", "--apply", "--yes", *argv]
            )
        return code, out, err, str(target), target, deletions

    def test_a_refused_recycle_is_terminal_and_reported_apart(self):
        """Critère 175, rendu texte."""
        code, out, err, path, target, deletions = self._recycle_failure([])
        self.assertEqual(code, clean.EXIT_REMOVAL)
        self.assertIn("Mise en corbeille refusée", out)
        self.assertIn("laissé en place", out)
        self.assertIn(path, out)
        self.assertNotIn("Verrouillés", out)
        self.assertEqual(deletions, [])  # aucun repli sur `delete_tree` (décision 14)
        self.assertTrue(target.exists())

    def test_a_refused_recycle_surfaces_in_both_keys_of_the_payload(self):
        """Critère 175, rendu machine — le seul qu'un lanceur peut lire."""
        code, out, _err, path, _target, _deletions = self._recycle_failure(["--json"])
        self.assertEqual(code, clean.EXIT_REMOVAL)
        run = json.loads(out)["run"]
        result = run["results"][0]
        self.assertEqual(result["recycle_failed_paths"], [path])
        self.assertEqual(result["locked_paths"], [])
        self.assertEqual(result["failed"], 700)
        self.assertEqual(run["failed_total_bytes"], 700)
        self.assertFalse(run["recycle_happened"])


class TestModerateAttemptsWarnOnlyCandidates(unittest.TestCase):
    """Critère 171, sans navigateur réel : un fichier verrouillé et un `warn-only`.

    Le fixture ouvre lui-même une poignée sur une entrée du candidat, ce qu'un
    navigateur ferait, et `procs.is_running` rend un **ensemble vide** — une
    interrogation réussie qui n'a rien trouvé, jamais `None`, sans quoi les
    modules `warn-and-skip` seraient omis et la convergence des deux invocations
    serait prouvée pour la mauvaise raison.
    """

    def _fixture(self, tmp: Path) -> tuple[CleanModule, Path]:
        target = tmp / "Cache"
        target.mkdir()
        write(target / "libre.bin", 300)
        write(target / "verrouille.bin", 200)
        module = fake_module(
            "cache-appli",
            candidates=(candidate("cache-appli", target, 500),),
            discovery=DISCOVERY_FIXED,
            level=Level.MODERATE,
            proc_guard=PROC_GUARD_WARN_ONLY,
        )
        return module, target

    def _run(self, argv: list[str], stdin: str | None):
        tmp = _tempdir(self)
        module, target = self._fixture(tmp)
        handle = open(target / "verrouille.bin", "rb")
        stack = contextlib.ExitStack()
        try:
            stack.enter_context(registry_of(module))
            stack.enter_context(
                mock.patch.object(registry_mod.procs, "is_running", return_value=set())
            )
            if stdin is not None:
                stream = io.StringIO(stdin)
                stream.isatty = lambda: True  # type: ignore[method-assign]
                stack.enter_context(mock.patch.object(clean.sys, "stdin", stream))
            with stack:
                return run_cli(
                    [
                        "--root",
                        str(tmp),
                        "--level",
                        "moderate",
                        "--no-recycle",
                        "--apply",
                        *argv,
                    ]
                )
        finally:
            handle.close()

    def test_yes_attempts_the_candidate_reports_the_lock_and_exits_zero(self):
        code, out, err = self._run(["--yes"], None)
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Verrouillés", out)
        self.assertIn("verrouille.bin", out)
        self.assertNotIn("skipped-running", out)

    def test_answering_the_prompt_gives_the_same_outcome(self):
        code, out, err = self._run([], "oui\n")
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Verrouillés", out)
        self.assertIn("verrouille.bin", out)


# --------------------------------------------------------------------------- #
# Le candidat sans chemin sous le régime de la corbeille
# --------------------------------------------------------------------------- #


class TestPathlessUnderRecycling(unittest.TestCase):
    """`docker-light`, le seul module sans chemin de la v1, sur le vrai registre."""

    def _run(self, argv: list[str], *, prune_code: int = 0, prune_stdout: str = ""):
        tmp = _tempdir(self)
        with docker_stub(prune_code=prune_code, prune_stdout=prune_stdout) as commands:
            with recycle_stub() as recycled, deletion_spy() as deletions:
                result = run_cli(
                    [
                        "--root",
                        str(tmp),
                        "--level",
                        "moderate",
                        "--only",
                        "docker-light",
                        "--apply",
                        "--yes",
                        *argv,
                    ]
                )
        return result, commands, recycled, deletions

    def test_it_reaches_its_own_clean_and_never_touches_can_recycle(self):
        """Critère 176 : la répartition précède tout test porteur de chemin."""
        (code, out, err), commands, recycled, deletions = self._run([])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn(mod_apps.DOCKER_PRUNE_COMMAND, commands)
        self.assertEqual(recycled["can"], [])
        self.assertEqual(recycled["recycle"], [])
        self.assertEqual(deletions, [])
        self.assertNotIn(SKIP_NO_UNDO, out)

    def test_it_prices_nothing_even_with_the_bait_line(self):
        """Critère 177 : `Total reclaimed space:` est un appât, pas une mesure."""
        (code, out, err), _commands, _recycled, _deletions = self._run(
            ["--json"], prune_stdout="Total reclaimed space: 1.523GB\n"
        )
        self.assertEqual(code, clean.EXIT_OK, err)
        result = json.loads(out)["run"]["results"][0]
        self.assertIsNone(result["freed"])
        self.assertIsNone(result["recycled"])
        self.assertIsNone(result["failed"])
        self.assertNotIn("1.523", out)

    def test_a_failing_prune_is_non_zero_and_prices_nothing_either(self):
        """Critère 177, seconde moitié.

        `clean_docker_light` lève, et `main()` relaie faute d'octets libérés à
        rapporter : le processus sort non nul par l'exception (`sys.exit(main())`
        n'est jamais atteint). Le rapport, lui, a été écrit depuis le `finally`.
        """
        tmp = _tempdir(self)
        out = io.StringIO()
        with docker_stub(prune_code=1):
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(OSError):
                    clean.main(
                        [
                            "--root",
                            str(tmp),
                            "--level",
                            "moderate",
                            "--only",
                            "docker-light",
                            "--apply",
                            "--yes",
                        ]
                    )
        text = out.getvalue()
        self.assertIn("docker-light", text)
        self.assertIn("unknown", text)
        self.assertNotIn("0 B", text)


# --------------------------------------------------------------------------- #
# Provenance des octets : mesure à l'application, jamais l'estimation
# --------------------------------------------------------------------------- #


class TestBytesComeFromTheApply(unittest.TestCase):
    def test_a_tree_that_grew_reports_its_size_at_apply_time(self):
        """Critère 178 : la divergence *est* l'assertion."""
        tmp = _tempdir(self)
        target = tmp / "cache"
        target.mkdir()
        write(target / "gros.bin", 100)
        built = candidate("modere", target, 100)

        def _discover(**_kw):
            # L'arbre grossit **après** l'estimation, en réécrivant une entrée
            # existante : le mtime du répertoire ne bouge pas, donc le garde
            # secondaire n'omet pas le candidat et la mesure seule est en cause.
            write(target / "gros.bin", 1000)
            return [dataclasses.replace(built)]

        module = fake_module(
            "modere", discover=_discover, level=Level.MODERATE, discovery=DISCOVERY_FIXED
        )
        with registry_of(module), recycle_stub() as recycled:
            code, out, err = run_cli(
                ["--root", str(tmp), "--level", "moderate", "--apply", "--yes", "--json"]
            )
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertEqual(recycled["recycle"], [str(target)])
        payload = json.loads(out)
        result = payload["run"]["results"][0]
        self.assertEqual(result["estimated"], 100)
        self.assertEqual(result["recycled"], 1000)
        self.assertEqual(result["freed"], 0)


# --------------------------------------------------------------------------- #
# Pied de page de récupération différée (décision 18)
# --------------------------------------------------------------------------- #


class TestDeferredReclamationFooter(unittest.TestCase):
    def _recycled_run(self, argv: list[str], *, measured: bool = True):
        tmp = _tempdir(self)
        target = tmp / "cache"
        target.mkdir()
        write(target / "a.bin", 640)
        module = moderate_module("modere", target, 640)
        stack = contextlib.ExitStack()
        with stack:
            stack.enter_context(registry_of(module))
            stack.enter_context(recycle_stub())
            if not measured:
                # La mesure d'avant opération échoue alors que la corbeille
                # reçoit les octets : total inconnu, pied de page dû quand même.
                stack.enter_context(
                    mock.patch.object(clean, "estimate_path", return_value=None)
                )
            return run_cli(
                ["--root", str(tmp), "--level", "moderate", "--apply", "--yes", *argv]
            )

    def test_a_recycled_run_prints_the_total_and_the_three_ways_out(self):
        code, out, err = self._recycled_run([])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("Mis en corbeille : ", out)
        self.assertIn("--trash-days 0", out)  # sinon la commande est un no-op
        for line in RECYCLE_FOOTER_LINES:
            self.assertIn(line, out)

    def test_freed_stays_zero_on_a_run_whose_only_outcome_is_recycling(self):
        code, out, err = self._recycled_run(["--json"])
        self.assertEqual(code, clean.EXIT_OK, err)
        run = json.loads(out)["run"]
        self.assertEqual(run["freed_total_bytes"], 0)
        self.assertEqual(run["recycled_total_bytes"], 640)
        self.assertTrue(run["recycle_happened"])

    def test_an_unmeasured_total_still_prints_all_three_ways_out(self):
        code, out, err = self._recycled_run([], measured=False)
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("quantité inconnue", out)
        for line in RECYCLE_FOOTER_LINES:
            self.assertIn(line, out)

    def test_a_run_that_recycled_nothing_prints_no_footer(self):
        with tempdir(self) as tmp:
            target = tmp / "cache"
            target.mkdir()
            write(target / "a.bin", 320)
            module = fake_module(
                "factice",
                candidates=(candidate("factice", target, 320),),
                discovery=DISCOVERY_FIXED,
            )
            with registry_of(module):
                code, out, err = run_cli(["--root", str(tmp), "--apply", "--yes"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertNotIn("--trash-days 0", out)
        self.assertNotIn("toujours sur le disque", out)


# --------------------------------------------------------------------------- #
# Quota de la corbeille (décision 4)
# --------------------------------------------------------------------------- #


class TestBinAllowanceWarning(unittest.TestCase):
    def _plan(self, size: int | None, *, total: int = 1000, argv: list[str] | None = None):
        tmp = _tempdir(self)
        target = tmp / "cache"
        target.mkdir()
        write(target / "a.bin", size or 1)
        module = moderate_module("modere", target, size)
        with registry_of(module), mock.patch.object(
            clean.shutil, "disk_usage", return_value=usage(total)
        ):
            return (
                *run_cli(["--root", str(tmp), "--level", "moderate", "--json", *(argv or [])]),
                str(target),
            )

    def _codes(self, out: str) -> list[str]:
        return [w["code"] for w in json.loads(out)["warnings"]]

    def test_over_ten_percent_warns_without_recycle_on_the_command_line(self):
        code, out, err, _path = self._plan(500)
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("recycle-bin-allowance", self._codes(out))

    def test_under_ten_percent_stays_silent(self):
        code, out, err, _path = self._plan(50)
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertNotIn("recycle-bin-allowance", self._codes(out))
        self.assertNotIn("recycle-bin-allowance-unknown", self._codes(out))

    def test_an_unmeasurable_candidate_gets_its_own_code_in_the_payload(self):
        code, out, err, path = self._plan(None)
        self.assertEqual(code, clean.EXIT_OK, err)
        codes = self._codes(out)
        self.assertIn("recycle-bin-allowance-unknown", codes)
        self.assertNotIn("recycle-bin-allowance", codes)
        warning = next(
            w for w in json.loads(out)["warnings"] if w["code"] == "recycle-bin-allowance-unknown"
        )
        self.assertEqual(warning["path"], path)
        self.assertEqual(warning["volume"], clean.volume_of(path))
        self.assertIn("modere", warning["label"])

    def test_the_printed_sentence_hedges(self):
        tmp = _tempdir(self)
        target = tmp / "cache"
        target.mkdir()
        write(target / "a.bin", 500)
        module = moderate_module("modere", target, 500)
        with registry_of(module), mock.patch.object(
            clean.shutil, "disk_usage", return_value=usage(1000)
        ):
            code, out, err = run_cli(["--root", str(tmp), "--level", "moderate"])
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("peut dépasser", out)

    def test_a_pathless_candidate_is_never_sized_against_a_volume(self):
        """`splitdrive(None)` planterait, et cette étape court avant la répartition."""
        tmp = _tempdir(self)
        with docker_stub(), mock.patch.object(
            clean, "volume_of", side_effect=clean.volume_of
        ) as volumes:
            code, out, err = run_cli(
                [
                    "--root",
                    str(tmp),
                    "--level",
                    "moderate",
                    "--only",
                    "docker-light",
                    "--json",
                ]
            )
        self.assertEqual(code, clean.EXIT_OK, err)
        volumes.assert_not_called()
        codes = self._codes(out)
        # `ceiling-total-partial` reste dû : un candidat sans prix rend le total
        # partiel. Seul le quota de la corbeille est hors sujet ici.
        self.assertNotIn("recycle-bin-allowance", codes)
        self.assertNotIn("recycle-bin-allowance-unknown", codes)

    def test_the_warning_changes_neither_the_mode_nor_the_exit_code(self):
        tmp = _tempdir(self)
        target = tmp / "cache"
        target.mkdir()
        write(target / "a.bin", 500)
        module = moderate_module("modere", target, 500)
        with registry_of(module), recycle_stub() as recycled, deletion_spy() as deletions:
            with mock.patch.object(clean.shutil, "disk_usage", return_value=usage(1000)):
                code, out, err = run_cli(
                    ["--root", str(tmp), "--level", "moderate", "--apply", "--yes"]
                )
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn("peut dépasser", out)
        self.assertEqual(recycled["recycle"], [str(target)])
        self.assertEqual(deletions, [])


# --------------------------------------------------------------------------- #
# `%TEMP%` : le garde secondaire, et jusqu'où il voit
# --------------------------------------------------------------------------- #


class TestUserTempSecondaryGuard(unittest.TestCase):
    @contextlib.contextmanager
    def _registry_with_hook(self, hook):
        """Registre réel où la découverte de `user-temp` porte un crochet.

        Le crochet est la seule fenêtre disponible entre le plan et
        l'application : il tourne pendant la découverte, donc après l'estampille
        du candidat et avant toute suppression.
        """
        registry = dict(registry_mod.MODULES)
        registry["user-temp"] = dataclasses.replace(registry["user-temp"], discover=hook)
        with mock.patch.object(registry_mod, "MODULES", registry):
            yield

    def _run(self, temp: Path, hook):
        with self._registry_with_hook(hook), mock.patch.dict(
            os.environ, {"TEMP": str(temp)}
        ), deletion_spy() as deletions:
            code, out, err = run_cli(
                [
                    "--root",
                    str(temp),
                    "--level",
                    "moderate",
                    "--only",
                    "user-temp",
                    "--apply",
                    "--yes",
                    "--no-recycle",
                ]
            )
        return code, out, err, deletions

    def test_candidates_carry_a_mtime_so_the_guard_runs_at_all(self):
        temp = _tempdir(self)
        write(temp / "installeur.tmp", 300)
        found = mod_apps.discover_user_temp(env={"TEMP": str(temp)})
        self.assertTrue(found)
        for entry in found:
            self.assertIsNotNone(entry.stat_mtime)

    def test_a_direct_entry_whose_mtime_moved_is_skipped(self):
        """Critère 182, première moitié."""
        temp = _tempdir(self)
        entry = write(temp / "installeur.tmp", 300)
        real = registry_mod.MODULES["user-temp"].discover

        def _hook(**kwargs):
            found = real(**kwargs)
            stamp = os.stat(entry).st_mtime + 120
            os.utime(entry, (stamp, stamp))
            return found

        code, out, err, deletions = self._run(temp, _hook)
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertIn(SKIP_CHANGED, out)
        self.assertEqual(deletions, [])
        self.assertTrue(entry.exists())

    def test_a_write_two_levels_down_is_the_documented_residual_risk(self):
        """Critère 182, seconde moitié : le garde ne voit que les entrées directes."""
        temp = _tempdir(self)
        branch = temp / "installeur-xyz"
        child = branch / "payload"
        write(child / "existant.bin", 100)
        real = registry_mod.MODULES["user-temp"].discover

        def _hook(**kwargs):
            found = real(**kwargs)
            write(child / "nouveau.bin", 400)  # deux niveaux sous le candidat
            return found

        code, out, err, deletions = self._run(temp, _hook)
        self.assertEqual(code, clean.EXIT_OK, err)
        self.assertNotIn(SKIP_CHANGED, out)
        self.assertEqual(deletions, [str(branch)])
        self.assertFalse(branch.exists())


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
