"""Tests du registre : tables déclarées, et comportement qui les honore (Phase 3).

Les trois tables (`discovery`, `proc_guard`, `needs_network`) sont comparées à
l'ensemble **actuellement enregistré**, jamais à un tout figé : les Parts 2 et 3
étendent la table au lieu de casser le test. Chaque table est doublée d'au moins
une assertion de comportement par valeur présente - sans quoi une table n'est
qu'un commentaire exécutable, et passerait sur un module dont la propriété
déclarée est un mensonge.
"""

from __future__ import annotations

import contextlib
import dataclasses
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Iterator
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import (  # noqa: E402
    guards,
    mod_apps,
    mod_dev,
    mod_system,
    procs,
    registry_mod,
)
from scripts.winclean.common import (  # noqa: E402
    DISCOVERY_FIXED,
    DISCOVERY_MODES,
    DISCOVERY_PATHLESS,
    DISCOVERY_WALKING,
    EXCLUDE_NEEDS_NETWORK,
    PROC_GUARDS,
    PROC_GUARD_WARN_AND_SKIP,
    PROC_GUARD_WARN_ONLY,
    CleanModule,
    Level,
)
from scripts.winclean.registry_mod import (  # noqa: E402
    MODULE_ORDER,
    MODULES,
    ValidationError,
    discover_module,
    modules_for_level,
    ownership_rank,
    select_modules,
    validate_level,
    validate_names,
)
from scripts.winclean.tests.test_mod_dev import cargo_project, tempdir, write  # noqa: E402

# --------------------------------------------------------------------------- #
# Les trois tables déclarées
# --------------------------------------------------------------------------- #

EXPECTED_DISCOVERY: dict[str, str] = {
    "cargo-target": DISCOVERY_WALKING,
    "pycache": DISCOVERY_WALKING,
    "dotnet-binobj": DISCOVERY_WALKING,
    "cargo-registry": DISCOVERY_FIXED,
    "npm-cache": DISCOVERY_FIXED,
    "pnpm-store": DISCOVERY_FIXED,
    "yarn-cache": DISCOVERY_FIXED,
    "bun-cache": DISCOVERY_FIXED,
    "pip-cache": DISCOVERY_FIXED,
    "uv-cache": DISCOVERY_FIXED,
    "nuget-packages": DISCOVERY_FIXED,
    "browser-cache": DISCOVERY_FIXED,
    "vscode-cache": DISCOVERY_FIXED,
    "user-temp": DISCOVERY_FIXED,
    "crashdumps": DISCOVERY_FIXED,
    "docker-light": DISCOVERY_PATHLESS,
    # `recycle-bin` énumère les volumes locaux au lieu de résoudre un chemin
    # documenté, et reste `fixed` : le critère est que `--root` ne le borne pas.
    "recycle-bin": DISCOVERY_FIXED,
    "package-cache": DISCOVERY_FIXED,
    "pip-cache-linux": DISCOVERY_FIXED,
    "pnpm-store-linux": DISCOVERY_FIXED,
    "apt-cache": DISCOVERY_FIXED,
    "browser-cache-linux": DISCOVERY_FIXED,
    "user-cache-linux": DISCOVERY_FIXED,
    "journal-vacuum": DISCOVERY_PATHLESS,
}

EXPECTED_PROC_GUARD: dict[str, str | None] = {
    "cargo-target": PROC_GUARD_WARN_AND_SKIP,
    "pycache": None,
    "dotnet-binobj": PROC_GUARD_WARN_AND_SKIP,
    "cargo-registry": None,
    "npm-cache": None,
    "pnpm-store": None,
    "yarn-cache": None,
    "bun-cache": None,
    "pip-cache": None,
    "uv-cache": None,
    "nuget-packages": None,
    "browser-cache": PROC_GUARD_WARN_ONLY,
    "vscode-cache": PROC_GUARD_WARN_ONLY,
    "user-temp": None,
    "crashdumps": None,
    "docker-light": None,
    # Ni l'un ni l'autre n'interroge `procs.is_running` : une réparation MSI est
    # cassée par la **suppression**, pas par un processus concurrent, et la
    # corbeille n'a pas de propriétaire nommé qu'on saurait interroger.
    "recycle-bin": None,
    "package-cache": None,
    "pip-cache-linux": None,
    "pnpm-store-linux": None,
    "apt-cache": None,
    "browser-cache-linux": PROC_GUARD_WARN_ONLY,
    "user-cache-linux": None,
    "journal-vacuum": None,
}

EXPECTED_NEEDS_NETWORK: dict[str, bool] = {
    "cargo-target": False,
    "pycache": False,
    "dotnet-binobj": False,
    "cargo-registry": True,
    "npm-cache": True,
    "pnpm-store": True,
    "yarn-cache": True,
    "bun-cache": True,
    "pip-cache": True,
    "uv-cache": True,
    "nuget-packages": True,
    # Aucun module `moderate` ne se re-télécharge depuis un registre de paquets :
    # `--offline` protège une machine qui ne peut pas reconstruire, et rien ici
    # ne bloque une construction.
    "browser-cache": False,
    "vscode-cache": False,
    "user-temp": False,
    "crashdumps": False,
    "docker-light": False,
    # `package-cache` est le module le plus conséquent et pourtant `False` : rien
    # ne se recharge automatiquement, donc `--offline` n'a rien à protéger ici.
    "recycle-bin": False,
    "package-cache": False,
    "pip-cache-linux": True,
    "pnpm-store-linux": True,
    "apt-cache": True,
    "browser-cache-linux": False,
    "user-cache-linux": False,
    "journal-vacuum": False,
}


@contextlib.contextmanager
def neutral_user_paths(case: unittest.TestCase) -> Iterator[Path]:
    """Neutralise l'environnement des modules `fixed` des parties 2 et 3.

    Les quatre modules `moderate` porteurs de chemin lisent des variables
    d'environnement réelles, et `docker-light` interroge un vrai démon. Un test
    qui parcourt tout le registre mesurerait alors le `%TEMP%` de la machine —
    lent, et différent d'une machine à l'autre. La sonde Docker est coupée au
    premier étage pour la même raison.

    Les deux modules `aggressive` le sont aussi, et pour la même raison en pire :
    `package-cache` tarifierait le vrai cache MSI de la machine, et `recycle-bin`
    parcourrait la vraie corbeille du compte qui lance les tests. Le SID est
    rendu irrésolu, ce qui suffit — c'est la branche fermée de la tâche 1, et elle
    rend zéro candidat sans jamais toucher un volume.
    """
    sandbox = tempdir(case)
    with contextlib.ExitStack() as stack:
        stack.enter_context(
            mock.patch.dict(
                os.environ,
                {
                    "TEMP": str(sandbox),
                    "LOCALAPPDATA": str(sandbox),
                    "APPDATA": str(sandbox),
                    "PROGRAMDATA": str(sandbox),
                },
            )
        )
        stack.enter_context(mock.patch.object(mod_apps.shutil, "which", return_value=None))
        stack.enter_context(
            mock.patch.object(mod_system, "current_user_sid", return_value=None)
        )
        yield sandbox


def fixture_root(case: unittest.TestCase) -> Path:
    """Racine où chaque module `walking` enregistré trouve quelque chose."""
    root = tempdir(case)
    cargo_project(root)
    write(root / "paquet" / "__pycache__" / "m.cpython-313.pyc", 120)
    write(root / "Appli" / "Appli.csproj", 64)
    write(root / "Appli" / "bin" / "Appli.dll", 200)
    write(root / "Appli" / "obj" / "project.assets.json", 80)
    return root


class TestDeclaredTables(unittest.TestCase):
    def test_discovery_mode_covers_the_registered_set_exactly(self) -> None:
        self.assertEqual(set(EXPECTED_DISCOVERY), set(MODULES))
        for name, expected in EXPECTED_DISCOVERY.items():
            with self.subTest(module=name):
                self.assertEqual(MODULES[name].discovery, expected)
                self.assertIn(expected, DISCOVERY_MODES)

    def test_proc_guard_covers_the_registered_set_exactly(self) -> None:
        self.assertEqual(set(EXPECTED_PROC_GUARD), set(MODULES))
        for name, expected in EXPECTED_PROC_GUARD.items():
            with self.subTest(module=name):
                self.assertEqual(MODULES[name].proc_guard, expected)
                self.assertIn(expected, PROC_GUARDS)

    def test_needs_network_covers_the_registered_set_exactly(self) -> None:
        self.assertEqual(set(EXPECTED_NEEDS_NETWORK), set(MODULES))
        for name, expected in EXPECTED_NEEDS_NETWORK.items():
            with self.subTest(module=name):
                self.assertIs(MODULES[name].needs_network, expected)

    def test_the_three_discovery_modes_are_all_represented(self) -> None:
        # La moitié comportementale de `pathless` arrive avec `docker-light` :
        # un critère sur un panier vide est infalsifiable.
        self.assertEqual(set(DISCOVERY_MODES), set(EXPECTED_DISCOVERY.values()))


class TestDiscoveryBehaviour(unittest.TestCase):
    """La moitié comportementale des modes `walking` et `fixed`."""

    def test_walking_modules_yield_nothing_on_an_empty_root(self) -> None:
        root = tempdir(self)
        for name, mode in EXPECTED_DISCOVERY.items():
            if mode != DISCOVERY_WALKING:
                continue
            with self.subTest(module=name):
                with mock.patch.object(procs, "is_running", return_value=set()):
                    self.assertEqual(
                        discover_module(MODULES[name], roots=[root], max_depth=6), []
                    )

    def test_a_fixed_module_still_yields_with_no_root_at_all(self) -> None:
        root = tempdir(self)
        write(root / "contenu" / "a", 300)
        fixed = [n for n, mode in EXPECTED_DISCOVERY.items() if mode == DISCOVERY_FIXED]
        self.assertTrue(fixed)
        name = fixed[0]
        with mock.patch.object(mod_dev, "resolve_cache_path", return_value=root):
            found = discover_module(MODULES[name], roots=[], max_depth=6)
        self.assertEqual([c.estimated_bytes for c in found], [300])

    def test_all_three_modes_at_once_on_a_single_empty_root(self) -> None:
        """Un seul run prouve les trois modes de découverte.

        Les deux étages de la sonde Docker sont corrigés — `shutil.which` et le
        **statut de sortie** de `docker system df --format json` — et `%TEMP%`
        pointe sur un fixture à une entrée. Aucun des deux correctifs n'est
        optionnel : sans le premier le critère échoue sur toute machine sans
        Docker, sans le second un module `fixed` qui ne rend rien est
        indiscernable d'un module `walking`, qui est précisément la distinction
        testée.
        """
        empty = tempdir(self)
        fake_temp = tempdir(self)
        write(fake_temp / "installeur.tmp", 700)

        found: dict[str, list[str | None]] = {}
        with contextlib.ExitStack() as stack:
            stack.enter_context(
                mock.patch.dict(
                    os.environ,
                    {
                        "TEMP": str(fake_temp),
                        "LOCALAPPDATA": str(empty),
                        "APPDATA": str(empty),
                        "PROGRAMDATA": str(empty),
                    },
                )
            )
            # Même raison que le correctif ci-dessous, pour les deux modules
            # `aggressive` : sans lui, ce test parcourt la vraie corbeille et
            # tarifie le vrai cache MSI de la machine.
            stack.enter_context(
                mock.patch.object(mod_system, "current_user_sid", return_value=None)
            )
            # Les autres modules `fixed` pointeraient sur les vrais caches de la
            # machine : mesurés, ils rendraient le test lent et dépendant du poste.
            stack.enter_context(
                mock.patch.object(mod_dev, "resolve_cache_path", return_value=None)
            )
            stack.enter_context(
                mock.patch.object(mod_apps.shutil, "which", return_value="docker.exe")
            )
            stack.enter_context(
                mock.patch.object(
                    mod_apps,
                    "_run",
                    return_value=subprocess.CompletedProcess(
                        args=list(mod_apps.DOCKER_DF_COMMAND), returncode=0, stdout="{}"
                    ),
                )
            )
            stack.enter_context(mock.patch.object(procs, "is_running", return_value=set()))
            for name, module in MODULES.items():
                found[name] = [
                    c.path for c in discover_module(module, roots=[empty], max_depth=6)
                ]

        # `pathless` : découvert sans aucune racine utile, et sans chemin.
        self.assertEqual([None], found["docker-light"])
        # `fixed` : découvert bien que la racine soit vide.
        self.assertEqual([str(fake_temp / "installeur.tmp")], found["user-temp"])
        # `walking` : rien à trouver sous une racine vide.
        for name, mode in EXPECTED_DISCOVERY.items():
            if mode == DISCOVERY_WALKING:
                with self.subTest(module=name):
                    self.assertEqual([], found[name])


class TestNeedsNetworkStamping(unittest.TestCase):
    def test_every_candidate_carries_its_module_declaration(self) -> None:
        root = fixture_root(self)
        cache = tempdir(self)
        write(cache / "contenu" / "a", 90)
        seen = 0
        with neutral_user_paths(self):
          with mock.patch.object(mod_dev, "resolve_cache_path", return_value=cache):
            with mock.patch.object(procs, "is_running", return_value=set()):
                for name, module in MODULES.items():
                    found = discover_module(module, roots=[root], max_depth=6)
                    for candidate in found:
                        seen += 1
                        with self.subTest(module=name, path=candidate.path):
                            self.assertIs(candidate.needs_network, module.needs_network)
        # L'assertion est vacuous module par module (un `requires` manquant est
        # légitime), mais le mécanisme doit avoir été exercé au moins une fois.
        self.assertGreater(seen, 0)

    def test_offline_drops_the_refilled_and_keeps_the_rebuilt(self) -> None:
        root = fixture_root(self)
        cache = tempdir(self)
        write(cache / "contenu" / "a", 90)
        candidates = []
        with neutral_user_paths(self):
          with mock.patch.object(mod_dev, "resolve_cache_path", return_value=cache):
            with mock.patch.object(procs, "is_running", return_value=set()):
                for module in MODULES.values():
                    candidates.extend(discover_module(module, roots=[root], max_depth=6))
        kept, excluded = guards.filter_needs_network(candidates)
        self.assertTrue(kept)
        self.assertTrue(excluded)
        self.assertTrue(all(not c.needs_network for c in kept))
        self.assertTrue(all(MODULES[e.module].needs_network for e in excluded))
        self.assertTrue(all(e.reason == EXCLUDE_NEEDS_NETWORK for e in excluded))

    def test_a_discover_function_never_sets_the_field_itself(self) -> None:
        # Deux sources de vérité laisseraient la table au vert pendant que
        # `--offline` filtrerait sur la mauvaise valeur.
        root = fixture_root(self)
        cache = tempdir(self)
        write(cache / "contenu" / "a", 10)
        with neutral_user_paths(self):
          with mock.patch.object(mod_dev, "resolve_cache_path", return_value=cache):
            with mock.patch.object(procs, "is_running", return_value=set()):
                for name, module in MODULES.items():
                    if not module.needs_network:
                        continue
                    raw = module.discover(roots=[root], max_depth=6)
                    for candidate in raw:
                        with self.subTest(module=name):
                            self.assertFalse(candidate.needs_network)


class TestModuleOrder(unittest.TestCase):
    def test_it_lists_every_registered_module_exactly_once(self) -> None:
        self.assertEqual(sorted(MODULE_ORDER), sorted(MODULES))
        self.assertEqual(len(MODULE_ORDER), len(set(MODULE_ORDER)))

    def test_ownership_rank_is_unique_by_construction(self) -> None:
        ranks = [ownership_rank(name) for name in MODULES]
        self.assertEqual(len(set(ranks)), len(ranks))

    def test_an_unknown_module_ranks_last(self) -> None:
        self.assertEqual(ownership_rank("inconnu"), len(MODULE_ORDER))


class TestLevelSelection(unittest.TestCase):
    def test_safe_selects_the_safe_modules_and_only_them(self) -> None:
        selected = [m.name for m in modules_for_level(Level.SAFE)]
        expected = [n for n in MODULE_ORDER if MODULES[n].level is Level.SAFE]
        self.assertEqual(selected, expected)
        self.assertNotIn("browser-cache", selected)

    def test_moderate_adds_this_part_s_modules_to_the_safe_ones(self) -> None:
        selected = [m.name for m in modules_for_level(Level.MODERATE)]
        for name in ("browser-cache", "vscode-cache", "user-temp", "crashdumps", "docker-light"):
            self.assertIn(name, selected)
        self.assertIn("pycache", selected)

    def test_a_higher_level_still_includes_the_safe_ones(self) -> None:
        names = [m.name for m in modules_for_level(Level.AGGRESSIVE)]
        for name in MODULE_ORDER:
            self.assertIn(name, names)


class TestValidation(unittest.TestCase):
    def test_an_unknown_only_name_aborts_and_lists_the_valid_ones(self) -> None:
        with self.assertRaises(ValidationError) as raised:
            validate_names(["nmp"])
        message = str(raised.exception)
        self.assertIn("nmp", message)
        self.assertIn("npm-cache", message)  # les noms valides sont nommés

    def test_a_valid_name_passes(self) -> None:
        validate_names(["npm-cache", "pycache"])

    def test_only_then_skip_can_select_nothing_without_erroring(self) -> None:
        called: list[str] = []
        registry = dict(MODULES)
        registry["pycache"] = dataclasses.replace(
            registry["pycache"], discover=lambda **_kw: called.append("x") or []
        )
        selected = select_modules(
            Level.SAFE, only=["pycache"], skip=["pycache"], registry=registry
        )
        self.assertEqual(selected, [])
        self.assertEqual(called, [])  # rien n'a été parcouru

    def test_skip_applies_after_only(self) -> None:
        selected = select_modules(Level.SAFE, only=["pycache", "npm-cache"], skip=["npm-cache"])
        self.assertEqual([m.name for m in selected], ["pycache"])

    def test_skip_alone_removes_from_the_level_set(self) -> None:
        selected = select_modules(Level.SAFE, skip=["pycache"])
        self.assertNotIn("pycache", [m.name for m in selected])
        self.assertIn("npm-cache", [m.name for m in selected])

    def test_an_unknown_skip_name_aborts_too(self) -> None:
        with self.assertRaises(ValidationError):
            select_modules(Level.SAFE, skip=["pycahce"])


class TestValidateLevel(unittest.TestCase):
    """Testé sur une entrée synthétique : cette part n'enregistre que du `safe`."""

    def registry_with_a_moderate(self) -> dict[str, CleanModule]:
        registry = dict(MODULES)
        registry["factice-moderate"] = CleanModule(
            name="factice-moderate",
            level=Level.MODERATE,
            requires=(),
            discover=lambda **_kw: [],
            clean=None,
            discovery=DISCOVERY_FIXED,
            proc_guard=None,
            needs_network=False,
        )
        return registry

    def test_a_module_above_the_active_level_aborts_naming_the_level(self) -> None:
        registry = self.registry_with_a_moderate()
        with self.assertRaises(ValidationError) as raised:
            validate_level(["factice-moderate"], Level.SAFE, registry)
        message = str(raised.exception)
        self.assertIn("factice-moderate", message)
        self.assertIn("--level moderate", message)

    def test_it_passes_once_the_level_is_raised(self) -> None:
        registry = self.registry_with_a_moderate()
        validate_level(["factice-moderate"], Level.MODERATE, registry)

    def test_an_unknown_name_aborts_as_unknown_not_as_a_level_problem(self) -> None:
        # `validate_names()` passe d'abord : tester la décision 12 avec un nom
        # non enregistré ferait passer le test pour la mauvaise raison.
        registry = self.registry_with_a_moderate()
        with self.assertRaises(ValidationError) as raised:
            validate_level(["pas-un-module"], Level.SAFE, registry)
        self.assertIn("module inconnu", str(raised.exception))
        self.assertNotIn("--level", str(raised.exception))

    def test_select_modules_refuses_it_before_any_discovery(self) -> None:
        registry = self.registry_with_a_moderate()
        with self.assertRaises(ValidationError):
            select_modules(Level.SAFE, only=["factice-moderate"], registry=registry)


#: Le seul `mod_*` autorisé à voir `remove.py`, et la liste **fermée** de ce
#: qu'il peut y lire. La partie 3 phase 3 tâche 1 l'exige : l'unité de
#: suppression de `recycle-bin` est la paire `$I`/`$R`, `remove.py` ne retire
#: qu'un chemin à la fois, donc le module porte son propre `clean()`. L'invariant
#: n'est pas abandonné pour autant, il est borné : la découverte peut consulter
#: le type de volume, la suppression peut effacer un arbre — et rien ne peut
#: atteindre la corbeille (`recycle`, `_sh_file_operation`), qui est exactement
#: ce que le niveau `aggressive` ne fait pas.
_REMOVAL_EXEMPT = "mod_system.py"
_ALLOWED_REMOVAL_USES = frozenset(
    {"delete_tree", "is_lock_error", "_get_drive_type", "DRIVE_FIXED"}
)


class TestNoRemovalInModules(unittest.TestCase):
    def test_no_mod_module_imports_the_removal_layer(self) -> None:
        package = Path(registry_mod.__file__).parent
        sources = sorted(package.glob("mod_*.py"))
        self.assertTrue(sources)
        exempt = [s for s in sources if s.name == _REMOVAL_EXEMPT]
        self.assertEqual(len(exempt), 1, "l'exception doit désigner un fichier réel")
        for source in sources:
            if source.name == _REMOVAL_EXEMPT:
                continue
            with self.subTest(module=source.name):
                text = source.read_text(encoding="utf-8")
                self.assertNotIn("import remove", text)
                self.assertNotIn("winclean.remove", text)

    def test_the_exempt_module_touches_only_the_sanctioned_removal_names(self) -> None:
        """L'exception est bornée : `mod_system` ne lit que quatre noms de `remove`.

        Sans cette borne, l'exception rendrait la couche de suppression
        entièrement accessible depuis un `mod_*`, et notamment `recycle()` — dont
        l'usage ici serait la mise à la corbeille de la corbeille.
        """
        source = Path(registry_mod.__file__).parent / _REMOVAL_EXEMPT
        text = source.read_text(encoding="utf-8")
        used = set(re.findall(r"\bremove\.([A-Za-z_][A-Za-z_0-9]*)", text))
        self.assertTrue(used, "le fichier exempté doit vraiment utiliser `remove`")
        self.assertEqual(used - _ALLOWED_REMOVAL_USES, set())


class TestDiscoveryIsCheapEnough(unittest.TestCase):
    def test_a_fixed_module_does_not_walk_the_roots(self) -> None:
        # Un module `fixed` qui parcourrait quand même les racines rendrait
        # `--root` payant pour rien : le mode déclaré serait un mensonge.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write(root / "a" / "b" / "c" / "d", 10)
            with mock.patch.object(mod_dev, "_walk") as walker:
                with mock.patch.object(mod_dev, "resolve_cache_path", return_value=None):
                    discover_module(MODULES["npm-cache"], roots=[root], max_depth=6)
            walker.assert_not_called()


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
