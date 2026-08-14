"""Unit tests for scripts/system_inventory/path_env.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` — see that module's docstring for why this is the only
import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_path_env``.
"""

from __future__ import annotations

import platform
import re
import tempfile
import unittest
from pathlib import Path

from scripts.system_inventory.path_env import WINREG_AVAILABLE, _build_path_items, scan_path, split_path_value

_PATH_ENV_SOURCE = (Path(__file__).resolve().parent.parent / "path_env.py").read_text(encoding="utf-8")

# winreg write APIs: none of these must ever be *called* (i.e. referenced as
# ``winreg.<name>`` / ``winreg.<name>Ex``) by path_env.py. Mirrors
# test_registry.py's _WINREG_WRITE_APIS list and pattern verbatim, applied
# to this module instead, enforcing the same read-only contract.
_WINREG_WRITE_APIS = (
    "SetValue",  # covers SetValue and SetValueEx
    "CreateKey",  # covers CreateKey and CreateKeyEx
    "DeleteKey",  # covers DeleteKey and DeleteKeyEx
    "DeleteValue",
    "SaveKey",
    "RestoreKey",
    "LoadKey",
    "DisableReflectionKey",
    "EnableReflectionKey",
)


class SplitPathValueTests(unittest.TestCase):
    def test_splits_on_semicolon(self):
        self.assertEqual(split_path_value("C:\\A;C:\\B;C:\\C"), ["C:\\A", "C:\\B", "C:\\C"])

    def test_strips_whitespace_around_entries(self):
        self.assertEqual(split_path_value("  C:\\A ; C:\\B  "), ["C:\\A", "C:\\B"])

    def test_drops_empty_segments_from_trailing_or_doubled_separators(self):
        self.assertEqual(split_path_value("C:\\A;;C:\\B;"), ["C:\\A", "C:\\B"])

    def test_empty_string_yields_empty_list(self):
        self.assertEqual(split_path_value(""), [])

    def test_none_like_falsy_value_yields_empty_list(self):
        self.assertEqual(split_path_value(None), [])  # type: ignore[arg-type]

    def test_single_entry_no_separator(self):
        self.assertEqual(split_path_value("C:\\Only"), ["C:\\Only"])


class BuildPathItemsTests(unittest.TestCase):
    def test_alive_vs_dead_flagging_via_real_temp_dirs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            alive_dir = root / "alive"
            alive_dir.mkdir()
            dead_dir = root / "does_not_exist"

            raw_values = {"user": f"{alive_dir};{dead_dir}", "system": None}
            items = _build_path_items(raw_values)

            by_path = {item.path: item for item in items}
            self.assertEqual(by_path[str(alive_dir)].detail["status"], "alive")
            self.assertEqual(by_path[str(dead_dir)].detail["status"], "dead")

    def test_every_item_has_none_size_bytes_and_source_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "one").mkdir()
            items = _build_path_items({"user": str(root / "one"), "system": None})
            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].source, "path")
            self.assertIsNone(items[0].size_bytes)

    def test_both_origins_captured_with_correct_origin_tag(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            user_dir = root / "user_dir"
            system_dir = root / "system_dir"
            user_dir.mkdir()
            system_dir.mkdir()

            items = _build_path_items({"user": str(user_dir), "system": str(system_dir)})

            by_path = {item.path: item for item in items}
            self.assertEqual(by_path[str(user_dir)].detail["origin"], "user")
            self.assertEqual(by_path[str(system_dir)].detail["origin"], "system")

    def test_dedupe_exact_duplicate_within_same_origin(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            some_dir = root / "some_dir"
            some_dir.mkdir()

            raw = f"{some_dir};{some_dir}"
            items = _build_path_items({"user": raw, "system": None})

            self.assertEqual(len(items), 1)

    @unittest.skipUnless(platform.system() == "Windows", "séparateur Windows")
    def test_dedupe_normalizes_trailing_separator_variants(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            some_dir = root / "some_dir"
            some_dir.mkdir()

            raw = f"{some_dir};{some_dir}\\"
            items = _build_path_items({"user": raw, "system": None})

            self.assertEqual(len(items), 1)

    def test_same_path_in_both_origins_is_kept_as_two_separate_items(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            shared_dir = root / "shared"
            shared_dir.mkdir()

            items = _build_path_items({"user": str(shared_dir), "system": str(shared_dir)})

            self.assertEqual(len(items), 2)
            origins = sorted(item.detail["origin"] for item in items)
            self.assertEqual(origins, ["system", "user"])

    def test_only_user_present_system_absent_is_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "a").mkdir()
            items = _build_path_items({"user": str(root / "a"), "system": None})
            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].detail["origin"], "user")

    def test_only_system_present_user_absent_is_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "b").mkdir()
            items = _build_path_items({"user": None, "system": str(root / "b")})
            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].detail["origin"], "system")

    def test_both_origins_absent_returns_empty_list(self):
        items = _build_path_items({"user": None, "system": None})
        self.assertEqual(items, [])

    def test_missing_keys_entirely_are_tolerated_like_none(self):
        items = _build_path_items({})
        self.assertEqual(items, [])

    def test_original_unexpanded_text_is_preserved_in_name_and_path(self):
        # An entry containing an unresolved %VAR% placeholder: existence is
        # checked against the expanded form, but the reported name/path
        # keep the original, unexpanded text.
        items = _build_path_items(
            {"user": "%SOME_UNDEFINED_VAR%\\bin", "system": None},
            exists_fn=lambda entry: False,
        )
        self.assertEqual(len(items), 1)
        self.assertEqual(items[0].name, "%SOME_UNDEFINED_VAR%\\bin")
        self.assertEqual(items[0].path, "%SOME_UNDEFINED_VAR%\\bin")
        self.assertEqual(items[0].detail["status"], "dead")

    def test_injectable_exists_fn_used_instead_of_real_filesystem(self):
        calls: list[str] = []

        def fake_exists(entry: str) -> bool:
            calls.append(entry)
            return entry == "C:\\Alive"

        items = _build_path_items({"user": "C:\\Alive;C:\\Dead", "system": None}, exists_fn=fake_exists)

        by_path = {item.path: item for item in items}
        self.assertEqual(by_path["C:\\Alive"].detail["status"], "alive")
        self.assertEqual(by_path["C:\\Dead"].detail["status"], "dead")
        self.assertEqual(sorted(calls), ["C:\\Alive", "C:\\Dead"])


class PathEnvSourceReadOnlyContractTests(unittest.TestCase):
    """Verifies path_env.py never references a winreg write API, mirroring
    test_registry.py's RegistrySourceReadOnlyContractTests.
    """

    def test_no_winreg_write_api_referenced(self):
        for write_api in _WINREG_WRITE_APIS:
            with self.subTest(write_api=write_api):
                pattern = re.compile(rf"winreg\.{re.escape(write_api)}\b")
                match = pattern.search(_PATH_ENV_SOURCE)
                self.assertIsNone(
                    match,
                    f"path_env.py must never call the winreg write API winreg.{write_api}(...)",
                )


@unittest.skipUnless(WINREG_AVAILABLE and platform.system() == "Windows", "winreg only available on Windows")
class ScanPathLiveSmokeTest(unittest.TestCase):
    """Runs the real scanner against this machine's live PATH registry values."""

    def test_returns_a_list_of_path_items_without_raising(self):
        items = scan_path()

        self.assertIsInstance(items, list)
        for item in items:
            self.assertEqual(item.source, "path")
            self.assertIsNone(item.size_bytes)
            self.assertIn(item.detail["origin"], ("user", "system"))
            self.assertIn(item.detail["status"], ("alive", "dead"))


if __name__ == "__main__":
    unittest.main()
