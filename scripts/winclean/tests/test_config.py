"""Tests du fichier de configuration : liste blanche, résolution, câblage CLI.

La propriété centrale n'est pas « le fichier est lu correctement » mais **le
fichier ne peut que restreindre**. Elle est vérifiée de deux façons
complémentaires : sur la forme des clés admises (aucune ne nomme un réglage qui
élargit), et de bout en bout sur un run (une clé inconnue arrête le run avant la
première lecture de disque).
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import clean, guards, registry_mod  # noqa: E402
from scripts.winclean import config as config_mod  # noqa: E402
from scripts.winclean.common import Level  # noqa: E402
from scripts.winclean.tests.test_clean import (  # noqa: E402
    candidate,
    fake_module,
    registry_of,
    run_cli,
)
from scripts.winclean.tests.test_mod_dev import tempdir, write  # noqa: E402

IS_WINDOWS = sys.platform == "win32"


def config_file(root: Path, payload: object, name: str = "winclean.json") -> Path:
    """Écrit un fichier de configuration et renvoie son chemin."""
    target = root / name
    target.write_text(json.dumps(payload), encoding="utf-8")
    return target


def raw_config_file(root: Path, text: str, name: str = "winclean.json") -> Path:
    """Écrit un contenu brut — pour les cas de JSON invalide."""
    target = root / name
    target.write_text(text, encoding="utf-8")
    return target


def parsed(argv: list[str]):
    return clean.build_parser().parse_args(argv)


# --------------------------------------------------------------------------- #
# La liste blanche ne peut que restreindre
# --------------------------------------------------------------------------- #


class TestAllowlistCannotWiden(unittest.TestCase):
    def test_no_allowed_key_names_a_widening_setting(self) -> None:
        """Aucune clé ne nomme `--apply`, le niveau, `--yes`, des racines ou une portée."""
        for key in config_mod.ALLOWED_KEYS:
            upper = key.upper()
            for fragment in config_mod.FORBIDDEN_KEY_FRAGMENTS:
                self.assertNotIn(
                    fragment,
                    upper,
                    f"la clé {key} porte {fragment!r} : elle élargirait le run",
                )

    def test_import_time_guard_rejects_a_widening_key(self) -> None:
        """La propriété est exécutable, pas seulement documentée.

        Ce test est le seul endroit où la garde d'import est déclenchée : sans
        lui, `_assert_restrictive()` serait du code jamais vu échouer.
        """
        widened = dict(config_mod.ALLOWED_KEYS)
        widened["EXTRA_ROOTS"] = ("protected_paths", lambda *_: ())
        with mock.patch.object(config_mod, "ALLOWED_KEYS", widened):
            with self.assertRaises(RuntimeError) as caught:
                config_mod._assert_restrictive()
        self.assertIn("EXTRA_ROOTS", str(caught.exception))

    def test_roots_key_is_refused_as_unknown(self) -> None:
        """`ROOTS` a existé dans le plan : un fichier qui la porte échoue bruyamment."""
        root = tempdir(self)
        path = config_file(root, {"ROOTS": ["D:\\"]})
        with self.assertRaises(config_mod.ConfigError) as caught:
            config_mod.load_config(path)
        message = str(caught.exception)
        self.assertIn("clé inconnue", message)
        self.assertIn("ROOTS", message)


# --------------------------------------------------------------------------- #
# Lecture et validation
# --------------------------------------------------------------------------- #


class TestLoadConfig(unittest.TestCase):
    def test_unknown_key_aborts(self) -> None:
        root = tempdir(self)
        path = config_file(root, {"TRASH_DAYS": 3, "TRASH_DAYZ": 3})
        with self.assertRaises(config_mod.ConfigError) as caught:
            config_mod.load_config(path)
        message = str(caught.exception)
        self.assertIn("clé inconnue", message)
        self.assertIn("TRASH_DAYZ", message)
        # Le message dit aussi ce qui *est* admis : une faute de frappe se corrige
        # sans ouvrir le code source.
        self.assertIn("TRASH_DAYS", message)

    def test_negative_trash_days_aborts(self) -> None:
        root = tempdir(self)
        path = config_file(root, {"TRASH_DAYS": -1})
        with self.assertRaises(config_mod.ConfigError) as caught:
            config_mod.load_config(path)
        self.assertIn("TRASH_DAYS", str(caught.exception))

    def test_boolean_is_not_accepted_as_an_integer(self) -> None:
        """`true` vaut 1 en Python : le refuser demande un test explicite."""
        root = tempdir(self)
        path = config_file(root, {"TRASH_DAYS": True})
        with self.assertRaises(config_mod.ConfigError):
            config_mod.load_config(path)

    def test_zero_ceiling_aborts(self) -> None:
        """Un plafond nul interdirait tout : c'est `--dry-run`, pas un plafond."""
        root = tempdir(self)
        path = config_file(root, {"MAX_DELETE_BYTES": 0})
        with self.assertRaises(config_mod.ConfigError):
            config_mod.load_config(path)

    def test_unknown_module_name_aborts(self) -> None:
        root = tempdir(self)
        path = config_file(root, {"DISABLED_MODULES": ["nmp"]})
        with self.assertRaises(config_mod.ConfigError) as caught:
            config_mod.load_config(path)
        message = str(caught.exception)
        self.assertIn("nmp", message)
        # Les noms valides sont listés : « npm » se trouve sans deviner.
        self.assertIn("npm", message)

    def test_relative_protected_path_is_refused_not_resolved(self) -> None:
        root = tempdir(self)
        path = config_file(root, {"PROTECTED_PATHS": ["archives"]})
        with self.assertRaises(config_mod.ConfigError) as caught:
            config_mod.load_config(path)
        self.assertIn("relatif", str(caught.exception))

    def test_protected_paths_are_normalised_for_comparison(self) -> None:
        """Sinon `guards.is_protected` lèverait `ValueError` au premier candidat."""
        root = tempdir(self)
        protected = root / "Archives"
        protected.mkdir()
        path = config_file(root, {"PROTECTED_PATHS": [str(protected)]})
        settings = config_mod.load_config(path)
        for entry in settings.protected_paths:
            self.assertEqual(entry, guards.normalise(entry))
        self.assertTrue(
            guards.is_protected(
                guards.normalise(protected / "un-fichier"),
                config_mod.protected_union(settings),
            )
        )

    def test_broken_json_names_line_and_column(self) -> None:
        root = tempdir(self)
        path = raw_config_file(root, '{"TRASH_DAYS": 3,}')
        with self.assertRaises(config_mod.ConfigError) as caught:
            config_mod.load_config(path)
        message = str(caught.exception)
        self.assertIn("ligne", message)
        self.assertIn("colonne", message)

    def test_non_object_root_aborts(self) -> None:
        root = tempdir(self)
        path = raw_config_file(root, "[1, 2, 3]")
        with self.assertRaises(config_mod.ConfigError):
            config_mod.load_config(path)

    def test_missing_default_file_is_not_an_error(self) -> None:
        root = tempdir(self)
        self.assertEqual(
            config_mod.load_config(env={"APPDATA": str(root)}),
            config_mod.Config(),
        )

    def test_absent_appdata_is_not_an_error(self) -> None:
        self.assertIsNone(config_mod.default_config_path(env={}))
        self.assertEqual(config_mod.load_config(env={}), config_mod.Config())

    def test_named_missing_file_is_an_error(self) -> None:
        """Écart assumé : un `--config` qui nomme un fichier absent échoue.

        Même doctrine qu'un `--only` mal orthographié — continuer sur les défauts
        laisserait croire que les restrictions demandées s'appliquent.
        """
        root = tempdir(self)
        with self.assertRaises(config_mod.ConfigError) as caught:
            config_mod.load_config(root / "absent.json")
        self.assertIn("introuvable", str(caught.exception))

    @unittest.skipUnless(IS_WINDOWS, "exemple de configuration Windows")
    def test_example_file_is_valid_strict_json_and_loads(self) -> None:
        """L'exemple livré est du JSON strict, sans commentaire, et validé."""
        example = Path(config_mod.__file__).with_name("winclean.json.example")
        settings = config_mod.load_config(example)
        self.assertEqual(settings.trash_days, 30)
        self.assertEqual(settings.source, str(example))
        self.assertTrue(settings.disabled_modules)

    def test_source_records_the_file_that_spoke(self) -> None:
        root = tempdir(self)
        path = config_file(root, {"TRASH_DAYS": 3})
        self.assertEqual(config_mod.load_config(path).source, str(path))
        self.assertIsNone(config_mod.DEFAULT_CONFIG.source)


# --------------------------------------------------------------------------- #
# Résolution : CLI > fichier > défaut, sauf pour les ensembles
# --------------------------------------------------------------------------- #


class TestResolution(unittest.TestCase):
    def test_cli_trash_days_beats_the_file(self) -> None:
        root = tempdir(self)
        settings = config_mod.load_config(config_file(root, {"TRASH_DAYS": 30}))
        self.assertEqual(config_mod.resolve_trash_days(0, settings), 0)

    def test_file_trash_days_beats_the_default(self) -> None:
        root = tempdir(self)
        settings = config_mod.load_config(config_file(root, {"TRASH_DAYS": 30}))
        self.assertEqual(config_mod.resolve_trash_days(None, settings), 30)

    def test_zero_in_the_file_is_not_absence(self) -> None:
        root = tempdir(self)
        settings = config_mod.load_config(config_file(root, {"TRASH_DAYS": 0}))
        self.assertEqual(config_mod.resolve_trash_days(None, settings), 0)

    def test_default_applies_without_file_or_flag(self) -> None:
        self.assertEqual(
            config_mod.resolve_trash_days(None), config_mod.DEFAULT_TRASH_DAYS
        )
        self.assertEqual(
            config_mod.resolve_max_delete_bytes(None),
            config_mod.DEFAULT_MAX_DELETE_BYTES,
        )

    def test_ceiling_resolution_order(self) -> None:
        root = tempdir(self)
        settings = config_mod.load_config(config_file(root, {"MAX_DELETE_BYTES": 1024}))
        self.assertEqual(config_mod.resolve_max_delete_bytes(None, settings), 1024)
        self.assertEqual(config_mod.resolve_max_delete_bytes(2048, settings), 2048)

    def test_protected_paths_add_and_never_remove(self) -> None:
        root = tempdir(self)
        extra = root / "archives"
        extra.mkdir()
        settings = config_mod.load_config(config_file(root, {"PROTECTED_PATHS": [str(extra)]}))
        union = config_mod.protected_union(settings)
        for default in guards.DEFAULT_PROTECTED:
            self.assertIn(default, union)
        self.assertIn(guards.normalise(extra), union)

    def test_empty_protected_list_does_not_erase_the_defaults(self) -> None:
        root = tempdir(self)
        settings = config_mod.load_config(config_file(root, {"PROTECTED_PATHS": []}))
        self.assertEqual(
            config_mod.protected_union(settings), tuple(guards.DEFAULT_PROTECTED)
        )

    def test_disabled_union_adds_to_skip(self) -> None:
        root = tempdir(self)
        settings = config_mod.load_config(
            config_file(root, {"DISABLED_MODULES": ["npm-cache"]})
        )
        self.assertEqual(
            config_mod.disabled_union(["pycache"], settings), ["pycache", "npm-cache"]
        )
        self.assertEqual(config_mod.disabled_union(["npm-cache"], settings), ["npm-cache"])


# --------------------------------------------------------------------------- #
# Câblage dans le CLI
# --------------------------------------------------------------------------- #


class TestCliWiring(unittest.TestCase):
    def test_unknown_key_aborts_the_run_before_any_discovery(self) -> None:
        root = tempdir(self)
        path = config_file(root, {"TRASH_DAYZ": 3})
        calls: list[str] = []

        def _spy(module, **kwargs):
            calls.append(module.name)
            return []

        with registry_of(fake_module("fake-a")):
            with mock.patch.object(registry_mod, "discover_module", _spy):
                code, _out, err = run_cli(["--config", str(path), "--root", str(root)])
        self.assertEqual(code, clean.EXIT_VALIDATION)
        self.assertIn("TRASH_DAYZ", err)
        self.assertEqual(calls, [])

    def test_disabled_module_in_only_aborts_before_any_discovery(self) -> None:
        root = tempdir(self)
        path = config_file(root, {"DISABLED_MODULES": ["fake-a"]})
        calls: list[str] = []

        def _spy(module, **kwargs):
            calls.append(module.name)
            return []

        with registry_of(fake_module("fake-a"), fake_module("fake-b")):
            with mock.patch.object(registry_mod, "discover_module", _spy):
                code, _out, err = run_cli(
                    ["--config", str(path), "--root", str(root), "--only", "fake-a"]
                )
        self.assertEqual(code, clean.EXIT_VALIDATION)
        self.assertIn("fake-a", err)
        self.assertIn("DISABLED_MODULES", err)
        self.assertIn(str(path), err)
        self.assertEqual(calls, [])

    def test_skip_and_config_disable_both_modules(self) -> None:
        """`--skip` s'unit à `DISABLED_MODULES` au lieu de le remplacer."""
        root = tempdir(self)
        path = config_file(root, {"DISABLED_MODULES": ["fake-a"]})
        with registry_of(
            fake_module("fake-a"), fake_module("fake-b"), fake_module("fake-c")
        ):
            settings = config_mod.load_config(path)
            args = parsed(["--config", str(path), "--root", str(root), "--skip", "fake-b"])
            plan = clean.build_plan(args, config=settings)
        self.assertEqual(list(plan.discovery_by_module), ["fake-c"])

    def test_plain_run_omits_the_disabled_module_without_an_error(self) -> None:
        root = tempdir(self)
        path = config_file(root, {"DISABLED_MODULES": ["fake-a"]})
        with registry_of(fake_module("fake-a"), fake_module("fake-b")):
            settings = config_mod.load_config(path)
            args = parsed(["--config", str(path), "--root", str(root)])
            plan = clean.build_plan(args, config=settings)
            code, out, err = run_cli(["--config", str(path), "--root", str(root)])
        self.assertEqual(list(plan.discovery_by_module), ["fake-b"])
        self.assertEqual(code, clean.EXIT_OK)
        self.assertEqual(err, "")
        self.assertNotIn("fake-a", out)

    def test_naming_a_disabled_module_in_skip_is_not_an_error(self) -> None:
        """Redondance légitime : le dire deux fois ne rend pas le run fautif."""
        root = tempdir(self)
        path = config_file(root, {"DISABLED_MODULES": ["fake-a"]})
        with registry_of(fake_module("fake-a"), fake_module("fake-b")):
            code, _out, err = run_cli(
                ["--config", str(path), "--root", str(root), "--skip", "fake-a"]
            )
        self.assertEqual(code, clean.EXIT_OK)
        self.assertEqual(err, "")

    def test_config_protected_path_drops_a_candidate_from_the_plan(self) -> None:
        root = tempdir(self)
        target = write(root / "archives" / "cache" / "gros.bin", 512)
        path = config_file(root, {"PROTECTED_PATHS": [str(root / "archives")]})
        module = fake_module("fake-a", candidates=(candidate("fake-a", target, 512),))

        with registry_of(module):
            args = parsed(["--config", str(path), "--root", str(root)])
            without = clean.build_plan(args)
            with_config = clean.build_plan(args, config=config_mod.load_config(path))

        self.assertEqual([c.path for c in without.candidates], [str(target)])
        self.assertEqual(with_config.candidates, [])
        self.assertEqual([d.path for d in with_config.dropped], [str(target)])

    def test_file_ceiling_stops_the_run_and_the_flag_lifts_it(self) -> None:
        root = tempdir(self)
        target = write(root / "cache" / "gros.bin", 4096)
        path = config_file(root, {"MAX_DELETE_BYTES": 1024})
        module = fake_module("fake-a", candidates=(candidate("fake-a", target, 4096),))

        with registry_of(module):
            low, _out, err = run_cli(["--config", str(path), "--root", str(root)])
            lifted, _out2, err2 = run_cli(
                [
                    "--config",
                    str(path),
                    "--root",
                    str(root),
                    "--max-delete-bytes",
                    "1MiB",
                ]
            )
        self.assertEqual(low, clean.EXIT_CEILING)
        self.assertIn("limite", err.lower())
        self.assertIn("1024", err)
        self.assertEqual(lifted, clean.EXIT_OK)
        self.assertEqual(err2, "")

    def test_real_registry_honours_disabled_modules(self) -> None:
        """Même propriété, sur des noms de modules réels et sans registre factice.

        Le critère du plan nomme `recycle-bin`, qui n'existe qu'à la phase 3 : la
        propriété ne dépend pas du module choisi, seulement du registre.
        """
        root = tempdir(self)
        path = config_file(root, {"DISABLED_MODULES": ["browser-cache"]})
        settings = config_mod.load_config(path)

        with self.assertRaises(registry_mod.ValidationError) as caught:
            registry_mod.select_modules(
                Level.MODERATE,
                ["browser-cache"],
                (),
                disabled=settings.disabled_modules,
                config_source=settings.source,
            )
        message = str(caught.exception)
        self.assertIn("browser-cache", message)
        self.assertIn("DISABLED_MODULES", message)
        self.assertIn(str(path), message)

        selected = {
            module.name
            for module in registry_mod.select_modules(
                Level.MODERATE, (), ["pycache"], disabled=settings.disabled_modules
            )
        }
        self.assertNotIn("browser-cache", selected)
        self.assertNotIn("pycache", selected)
        self.assertIn("npm-cache", selected)

    def test_trash_days_flag_refuses_a_negative_value(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()) as err:
            with self.assertRaises(SystemExit):
                parsed(["--trash-days", "-1"])
        self.assertIn("--trash-days", err.getvalue())

    def test_trash_days_and_config_default_to_none_on_the_namespace(self) -> None:
        """`None` = drapeau non tapé, ce que la résolution attend."""
        args = parsed([])
        self.assertIsNone(args.trash_days)
        self.assertIsNone(args.config)
        self.assertNotIn("max_delete_bytes", clean.explicit_flags(args))

    def test_explicit_ceiling_flag_is_tracked_even_at_its_default_value(self) -> None:
        """Un `--max-delete-bytes 50GiB` tapé doit battre un fichier plus bas.

        Comparer la valeur au défaut ne le distinguerait pas d'un drapeau absent.
        """
        args = parsed(["--max-delete-bytes", "50GiB"])
        self.assertIn("max_delete_bytes", clean.explicit_flags(args))


if __name__ == "__main__":
    unittest.main()
