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
import sys
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import clean, guards, registry_mod  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    DISCOVERY_FIXED,
    DISCOVERY_WALKING,
    DROP_PROTECTED,
    DROP_SANITY,
    EXCLUDE_NEEDS_NETWORK,
    PROC_GUARD_WARN_AND_SKIP,
    SKIP_CHANGED,
    SKIP_GONE,
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
    out, err = io.StringIO(), io.StringIO()
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


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
