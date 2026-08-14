"""Filet de sûreté porté sur les futurs modules Linux (Part 5 Phase 1).

`test_registry_mod.py` encode plusieurs invariants structurels sous forme de
tests qui *scannent la source* des `mod_*.py`, pas seulement leur
comportement - c'est cette discipline que le risk register de la partie 5
exige de reproduire pour `mod_linux_*.py` **avant** d'écrire leur logique,
justement pour que le filet existe avant ce qu'il est censé attraper.

Chaque classe ci-dessous porte le nom du test Windows qu'elle porte à son
tour, avec une note indiquant sa phase de mise en service :

- `TestNoRemovalInLinuxModules` est **actif dès aujourd'hui** : le scan de
  source ne suppose l'existence d'aucun module précis, il se contente de
  s'appliquer à tout `mod_linux_*.py` trouvé sur disque - vide aujourd'hui
  (`assertTrue(sources)` le fait `skip`, pas passer à vide), réel dès que la
  Phase 2 crée le premier fichier.
- Les trois autres classes ont besoin de modules *enregistrés* dans
  `registry_mod.MODULES` (pas seulement de fichiers présents) pour avoir
  quoi que ce soit à affirmer ; elles restent `skip` jusqu'à ce que les
  Phases 2/3 les enregistrent, avec la table `EXPECTED_*` correspondante
  écrite à ce moment-là - une table vide serait un test qui passe sans avoir
  rien vérifié, exactement le défaut que ce fichier existe pour éviter.
"""

from __future__ import annotations

import os
import re
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import mod_linux_pkg, platform_paths, registry_mod  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    DISCOVERY_FIXED,
    DISCOVERY_PATHLESS,
    PROC_GUARD_WARN_ONLY,
)

_LINUX_MODULE_GLOB = "mod_linux_*.py"

#: Les six modules Linux enregistrés en Phase 2/3. Comparée par égalité
#: stricte à `set(registry_mod.MODULES)` restreint à ces noms - une table
#: vide ou partielle validerait vacuously n'importe quel enregistrement.
EXPECTED_DISCOVERY_LINUX: dict[str, str] = {
    "pip-cache-linux": DISCOVERY_FIXED,
    "pnpm-store-linux": DISCOVERY_FIXED,
    "apt-cache": DISCOVERY_FIXED,
    "browser-cache-linux": DISCOVERY_FIXED,
    "user-cache-linux": DISCOVERY_FIXED,
    # Seul module Linux `pathless` : sa commande de suppression ne porte pas
    # sur un chemin, comme `docker-light` côté Windows.
    "journal-vacuum": DISCOVERY_PATHLESS,
}

EXPECTED_PROC_GUARD_LINUX: dict[str, str | None] = {
    "pip-cache-linux": None,
    "pnpm-store-linux": None,
    "apt-cache": None,
    # `warn-only`, pas `warn-and-skip` : un navigateur ouvert tient ses
    # fichiers de cache ouverts, il ne réécrit pas l'arbre supprimé.
    "browser-cache-linux": PROC_GUARD_WARN_ONLY,
    "user-cache-linux": None,
    "journal-vacuum": None,
}

EXPECTED_NEEDS_NETWORK_LINUX: dict[str, bool] = {
    "pip-cache-linux": True,
    "pnpm-store-linux": True,
    "apt-cache": True,
    "browser-cache-linux": False,
    "user-cache-linux": False,
    "journal-vacuum": False,
}


def _tempdir(case: unittest.TestCase) -> Path:
    root = Path(tempfile.mkdtemp(prefix="winclean-linux-contract-"))
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


def _linux_module_sources() -> list[Path]:
    package = Path(registry_mod.__file__).parent
    return sorted(package.glob(_LINUX_MODULE_GLOB))


class TestNoRemovalInLinuxModules(unittest.TestCase):
    """Porte `TestNoRemovalInModules` (test_registry_mod.py) sur `mod_linux_*.py`.

    Aucune exemption ici, contrairement à `mod_system.py` côté Windows : rien
    ne motive encore un `mod_linux_*.py` à lire `remove.py` directement -
    `trash_linux.py` n'est pas nommé `mod_linux_*` et c'est *lui* que
    `remove.py` doit importer, pas l'inverse (voir le plan, "Files to
    modify" - `remove.py`). Si la Phase 2/3 introduit un besoin symétrique au
    `recycle-bin` de Windows, cette classe devra alors gagner sa propre liste
    fermée d'usages sanctionnés, comme `_ALLOWED_REMOVAL_USES` le fait côté
    Windows.
    """

    def test_no_mod_linux_module_imports_the_removal_layer(self) -> None:
        sources = _linux_module_sources()
        if not sources:
            self.skipTest(
                f"aucun {_LINUX_MODULE_GLOB} sur disque - Phase 2 les crée"
            )
        for source in sources:
            with self.subTest(module=source.name):
                text = source.read_text(encoding="utf-8")
                self.assertNotIn("import remove", text)
                self.assertNotIn("winclean.remove", text)


class TestLinuxDeclaredTablesCrossCheck(unittest.TestCase):
    """Porte `TestDeclaredTables` (test_registry_mod.py) sur les modules Linux.

    Côté Windows, `EXPECTED_DISCOVERY`/`EXPECTED_PROC_GUARD`/
    `EXPECTED_NEEDS_NETWORK` sont des tables écrites à la main, comparées à
    `set(MODULES)` : une table vide validerait vacuously n'importe quel
    enregistrement, donc écrire ces tables avant qu'il y ait des modules
    Linux enregistrés produirait un test qui ne peut rien réfuter. La table
    s'écrit avec le module qu'elle vérifie, en Phase 2/3.
    """

    def test_discovery_proc_guard_needs_network_are_declared_truthfully(self) -> None:
        registered = set(registry_mod.MODULES)
        for name in EXPECTED_DISCOVERY_LINUX:
            with self.subTest(module=name):
                self.assertIn(name, registered)
        for name, expected in EXPECTED_DISCOVERY_LINUX.items():
            with self.subTest(module=name, table="discovery"):
                self.assertEqual(registry_mod.MODULES[name].discovery, expected)
        for name, expected in EXPECTED_PROC_GUARD_LINUX.items():
            with self.subTest(module=name, table="proc_guard"):
                self.assertEqual(registry_mod.MODULES[name].proc_guard, expected)
        for name, expected in EXPECTED_NEEDS_NETWORK_LINUX.items():
            with self.subTest(module=name, table="needs_network"):
                self.assertIs(registry_mod.MODULES[name].needs_network, expected)
        self.assertEqual(set(EXPECTED_DISCOVERY_LINUX), set(EXPECTED_PROC_GUARD_LINUX))
        self.assertEqual(set(EXPECTED_DISCOVERY_LINUX), set(EXPECTED_NEEDS_NETWORK_LINUX))


class TestLinuxNeedsNetworkNeverSelfStamped(unittest.TestCase):
    """Porte `TestNeedsNetworkStamping.test_a_discover_function_never_sets_the_field_itself`.

    Décision structurelle du registre (docstring de `registry_mod.py`) : un
    `discover_*()` ne fixe jamais `needs_network` lui-même, seul
    `discover_module()` l'estampille depuis la déclaration du module. Deux
    sites de vérité laisseraient la table Linux annoncer `True` pendant que
    la fonction émettrait `False` sans qu'aucun test ne le remarque.
    """

    def test_a_linux_discover_function_never_sets_the_field_itself(self) -> None:
        home = _tempdir(self)
        (home / ".cache" / "pip").mkdir(parents=True)
        (home / ".local" / "share" / "pnpm" / "store").mkdir(parents=True)
        env = {"HOME": str(home)}

        with mock.patch.object(mod_linux_pkg, "which", return_value=None):
            pip_found = mod_linux_pkg.discover_cache("pip-cache-linux", env=env)
            pnpm_found = mod_linux_pkg.discover_cache("pnpm-store-linux", env=env)

        archives = _tempdir(self)
        (archives / "some.deb").write_bytes(b"x")
        with mock.patch.object(platform_paths, "APT_ARCHIVES_DIR", archives):
            apt_found = mod_linux_pkg.discover_apt_archives()

        found = pip_found + pnpm_found + apt_found
        self.assertTrue(found, "aucun candidat rendu - le test ne réfute rien")
        for candidate in found:
            with self.subTest(module=candidate.module):
                self.assertFalse(candidate.needs_network)


class TestLinuxDiscoveryIsCheapEnough(unittest.TestCase):
    """Porte `TestDiscoveryIsCheapEnough.test_a_fixed_module_does_not_walk_the_roots`.

    Un module Linux déclaré `fixed` (les caches d'outils de
    `mod_linux_pkg.py` : chemin documenté, pas de racine à parcourir) qui
    parcourrait quand même `--root` rendrait ce mode-là mensonger, exactement
    comme côté Windows.
    """

    def test_a_fixed_linux_module_does_not_walk_the_roots(self) -> None:
        # `mod_linux_pkg.py` n'a pas d'équivalent de `mod_dev._walk` à patcher :
        # ses modules `fixed` résolvent un chemin unique, sans jamais parcourir
        # d'arbre. La garantie se vérifie donc par le résultat : un `--root`
        # énorme et non pertinent ne change rien, et `os.walk` n'est jamais
        # sollicité pendant l'appel.
        home = _tempdir(self)
        (home / ".cache" / "pip").mkdir(parents=True)
        huge_root = _tempdir(self)
        (huge_root / "a" / "b" / "c" / "d").mkdir(parents=True)
        env = {"HOME": str(home)}

        with mock.patch.object(mod_linux_pkg, "which", return_value=None):
            with mock.patch("os.walk") as walker:
                without_roots = registry_mod.discover_module(
                    registry_mod.MODULES["pip-cache-linux"], env=env
                )
                with_huge_root = registry_mod.discover_module(
                    registry_mod.MODULES["pip-cache-linux"],
                    roots=[huge_root],
                    max_depth=6,
                    env=env,
                )
        walker.assert_not_called()
        self.assertEqual(
            [c.path for c in without_roots], [c.path for c in with_huge_root]
        )


# Repère utilisé par un futur re.findall d'inventaire de contrat, au même
# titre que _ALLOWED_REMOVAL_USES côté Windows : garde la liste des noms de
# fonctions de test ci-dessus visible sans relire tout le fichier.
_PORTED_PATTERNS = frozenset(
    {
        "test_no_mod_linux_module_imports_the_removal_layer",
        "test_discovery_proc_guard_needs_network_are_declared_truthfully",
        "test_a_linux_discover_function_never_sets_the_field_itself",
        "test_a_fixed_linux_module_does_not_walk_the_roots",
    }
)


class TestThisFileNamesEveryPortedPattern(unittest.TestCase):
    """Garde-fou anti-dérive : un test ajouté ci-dessus sans être listé dans
    `_PORTED_PATTERNS` (ou l'inverse) signale que ce fichier et son en-tête
    ont divergé - vérifié sur les noms de méthode réels, pas recopié à la main.
    """

    def test_every_test_method_in_this_module_is_accounted_for(self) -> None:
        here = sys.modules[__name__]
        found = {
            name
            for cls_name in dir(here)
            if (cls := getattr(here, cls_name, None))
            and isinstance(cls, type)
            and issubclass(cls, unittest.TestCase)
            and cls is not TestThisFileNamesEveryPortedPattern
            for name in dir(cls)
            if name.startswith("test_")
        }
        self.assertEqual(found, set(_PORTED_PATTERNS))

    def test_pattern_names_use_the_test_prefix(self) -> None:
        self.assertTrue(all(re.match(r"^test_", n) for n in _PORTED_PATTERNS))


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
