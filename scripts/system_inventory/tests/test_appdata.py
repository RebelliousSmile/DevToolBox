"""Unit tests for scripts/system_inventory/appdata.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` — see that module's docstring for why this is the only
import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_appdata``.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from scripts.system_inventory.appdata import scan_appdata, scan_dotfolders, scan_programdata


class ScanAppdataTests(unittest.TestCase):
    def test_first_level_enumeration_and_sizes_across_both_roots(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            local = root / "local"
            roaming = root / "roaming"
            (local / "AppOne" / "nested").mkdir(parents=True)
            (local / "AppOne" / "file.txt").write_bytes(b"x" * 10)
            (local / "AppOne" / "nested" / "deep.txt").write_bytes(b"x" * 20)
            (local / "AppTwo").mkdir()
            (local / "AppTwo" / "file.txt").write_bytes(b"x" * 5)
            (roaming / "Vendor").mkdir(parents=True)
            (roaming / "Vendor" / "file.txt").write_bytes(b"x" * 7)

            base = {"LOCALAPPDATA": str(local), "APPDATA": str(roaming)}
            items = scan_appdata(base=base)

            by_name = {item.name: item for item in items}
            self.assertEqual(set(by_name), {"AppOne", "AppTwo", "Vendor"})
            self.assertEqual(by_name["AppOne"].size_bytes, 30)
            self.assertEqual(by_name["AppTwo"].size_bytes, 5)
            self.assertEqual(by_name["Vendor"].size_bytes, 7)
            self.assertEqual(by_name["AppOne"].source, "appdata")
            self.assertEqual(by_name["AppOne"].detail, {"root": "LOCALAPPDATA"})
            self.assertEqual(by_name["Vendor"].detail, {"root": "APPDATA"})
            self.assertEqual(by_name["AppOne"].path, str(local / "AppOne"))

    def test_files_directly_under_root_are_ignored(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            local = root / "local"
            local.mkdir()
            (local / "loose_file.txt").write_bytes(b"x" * 100)
            (local / "RealApp").mkdir()
            (local / "RealApp" / "data.bin").write_bytes(b"x" * 3)

            items = scan_appdata(base={"LOCALAPPDATA": str(local)})

            self.assertEqual([item.name for item in items], ["RealApp"])

    def test_nested_dirs_are_not_listed_as_separate_first_level_items(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            local = root / "local"
            (local / "App" / "sub1" / "sub2").mkdir(parents=True)
            (local / "App" / "sub1" / "sub2" / "f.txt").write_bytes(b"x" * 8)

            items = scan_appdata(base={"LOCALAPPDATA": str(local)})

            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].name, "App")
            self.assertEqual(items[0].size_bytes, 8)

    def test_missing_localappdata_root_tolerated_appdata_still_scanned(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            roaming = root / "roaming"
            (roaming / "Vendor").mkdir(parents=True)
            (roaming / "Vendor" / "f.txt").write_bytes(b"x" * 4)

            # LOCALAPPDATA key absent from base entirely.
            items = scan_appdata(base={"APPDATA": str(roaming)})

            self.assertEqual([item.name for item in items], ["Vendor"])

    def test_root_present_in_base_but_directory_missing_on_disk_is_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            does_not_exist = root / "nope"

            items = scan_appdata(base={"LOCALAPPDATA": str(does_not_exist)})

            self.assertEqual(items, [])

    def test_both_roots_missing_returns_empty_list_without_raising(self):
        items = scan_appdata(base={})
        self.assertEqual(items, [])

    def test_exclude_paths_forwarded_and_excludes_nested_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            local = root / "local"
            (local / "App" / "sub").mkdir(parents=True)
            (local / "App" / "keep.txt").write_bytes(b"x" * 11)
            excluded_file = local / "App" / "sub" / "excluded.txt"
            excluded_file.write_bytes(b"x" * 5000)

            items = scan_appdata(
                base={"LOCALAPPDATA": str(local)},
                exclude_paths={excluded_file.resolve()},
            )

            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].size_bytes, 11)


class ScanDotfoldersTests(unittest.TestCase):
    def test_dotfolders_are_picked_up_with_correct_size(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ".cargo" / "registry").mkdir(parents=True)
            (root / ".cargo" / "config.toml").write_bytes(b"x" * 3)
            (root / ".cargo" / "registry" / "cache.bin").write_bytes(b"x" * 9)

            items = scan_dotfolders(base=str(root))

            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].name, ".cargo")
            self.assertEqual(items[0].source, "dotfolder")
            self.assertEqual(items[0].size_bytes, 12)
            self.assertEqual(items[0].detail, {})
            self.assertEqual(items[0].path, str(root / ".cargo"))

    def test_non_dot_entries_are_ignored(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Documents").mkdir()
            (root / "Documents" / "f.txt").write_bytes(b"x" * 5)
            (root / ".npm").mkdir()

            items = scan_dotfolders(base=str(root))

            self.assertEqual([item.name for item in items], [".npm"])

    def test_dotfiles_that_are_plain_files_are_ignored(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ".gitconfig").write_bytes(b"x" * 5)
            (root / ".cache").mkdir()

            items = scan_dotfolders(base=str(root))

            self.assertEqual([item.name for item in items], [".cache"])

    def test_missing_userprofile_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "does_not_exist"
            items = scan_dotfolders(base=str(root))
            self.assertEqual(items, [])


class ScanProgramdataTests(unittest.TestCase):
    def test_first_level_enumeration_and_sizes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Microsoft" / "Windows").mkdir(parents=True)
            (root / "Microsoft" / "top.txt").write_bytes(b"x" * 6)
            (root / "Microsoft" / "Windows" / "deep.txt").write_bytes(b"x" * 4)
            (root / "SomeVendor").mkdir()
            (root / "SomeVendor" / "f.bin").write_bytes(b"x" * 2)

            items = scan_programdata(base=str(root))

            by_name = {item.name: item for item in items}
            self.assertEqual(set(by_name), {"Microsoft", "SomeVendor"})
            self.assertEqual(by_name["Microsoft"].size_bytes, 10)
            self.assertEqual(by_name["SomeVendor"].size_bytes, 2)
            self.assertEqual(by_name["Microsoft"].source, "programdata")
            self.assertEqual(by_name["Microsoft"].detail, {})

    def test_chocolatey_excluded_case_insensitively(self):
        for choco_name in ("chocolatey", "Chocolatey", "CHOCOLATEY"):
            with self.subTest(choco_name=choco_name):
                with tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    (root / choco_name / "lib").mkdir(parents=True)
                    (root / choco_name / "lib" / "pkg.txt").write_bytes(b"x" * 123)
                    (root / "OtherVendor").mkdir()
                    (root / "OtherVendor" / "f.txt").write_bytes(b"x" * 1)

                    items = scan_programdata(base=str(root))

                    self.assertEqual([item.name for item in items], ["OtherVendor"])

    def test_missing_programdata_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "does_not_exist"
            items = scan_programdata(base=str(root))
            self.assertEqual(items, [])

    def test_exclude_paths_forwarded_and_excludes_nested_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Vendor" / "sub").mkdir(parents=True)
            (root / "Vendor" / "keep.txt").write_bytes(b"x" * 13)
            excluded_file = root / "Vendor" / "sub" / "excluded.txt"
            excluded_file.write_bytes(b"x" * 5000)

            items = scan_programdata(base=str(root), exclude_paths={excluded_file.resolve()})

            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].size_bytes, 13)


if __name__ == "__main__":
    unittest.main()
