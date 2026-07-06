"""Unit tests for scripts/system_inventory/packages.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py``/``test_appdata.py`` — see ``test_common.py``'s docstring
for why this is the only import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_packages``.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from scripts.system_inventory.packages import scan_scoop_choco


class ScanScoopChocoTests(unittest.TestCase):
    def test_scoop_only_present_choco_absent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scoop = root / "scoop"
            (scoop / "apps" / "git" / "current").mkdir(parents=True)
            (scoop / "apps" / "git" / "current" / "bin.exe").write_bytes(b"x" * 30)
            (scoop / "apps" / "7zip").mkdir()
            (scoop / "apps" / "7zip" / "7z.exe").write_bytes(b"x" * 5)

            items = scan_scoop_choco(base={"scoop": str(scoop)})

            by_name = {item.name: item for item in items}
            self.assertEqual(set(by_name), {"git", "7zip"})
            self.assertEqual(by_name["git"].size_bytes, 30)
            self.assertEqual(by_name["7zip"].size_bytes, 5)
            self.assertEqual(by_name["git"].source, "scoop-choco")
            self.assertEqual(by_name["git"].detail, {"manager": "scoop"})
            self.assertEqual(by_name["7zip"].detail, {"manager": "scoop"})
            self.assertEqual(by_name["git"].path, str(scoop / "apps" / "git"))

    def test_chocolatey_only_present_scoop_absent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            choco = root / "chocolatey"
            (choco / "lib" / "nodejs").mkdir(parents=True)
            (choco / "lib" / "nodejs" / "tools.exe").write_bytes(b"x" * 42)

            items = scan_scoop_choco(base={"chocolatey": str(choco)})

            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].name, "nodejs")
            self.assertEqual(items[0].size_bytes, 42)
            self.assertEqual(items[0].source, "scoop-choco")
            self.assertEqual(items[0].detail, {"manager": "chocolatey"})
            self.assertEqual(items[0].path, str(choco / "lib" / "nodejs"))

    def test_both_absent_returns_empty_list(self):
        items = scan_scoop_choco(base={})
        self.assertEqual(items, [])

    def test_both_present_items_tagged_correctly(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scoop = root / "scoop"
            choco = root / "chocolatey"
            (scoop / "apps" / "git").mkdir(parents=True)
            (scoop / "apps" / "git" / "bin.exe").write_bytes(b"x" * 11)
            (choco / "lib" / "nodejs").mkdir(parents=True)
            (choco / "lib" / "nodejs" / "tools.exe").write_bytes(b"x" * 22)

            items = scan_scoop_choco(base={"scoop": str(scoop), "chocolatey": str(choco)})

            by_name = {item.name: item for item in items}
            self.assertEqual(set(by_name), {"git", "nodejs"})
            self.assertEqual(by_name["git"].detail, {"manager": "scoop"})
            self.assertEqual(by_name["git"].size_bytes, 11)
            self.assertEqual(by_name["nodejs"].detail, {"manager": "chocolatey"})
            self.assertEqual(by_name["nodejs"].size_bytes, 22)

    def test_manager_root_exists_but_apps_subfolder_missing_is_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scoop = root / "scoop"
            scoop.mkdir()  # scoop root exists, but no "apps" subfolder yet

            items = scan_scoop_choco(base={"scoop": str(scoop)})

            self.assertEqual(items, [])

    def test_manager_root_exists_but_lib_subfolder_missing_is_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            choco = root / "chocolatey"
            choco.mkdir()  # chocolatey root exists, but no "lib" subfolder yet

            items = scan_scoop_choco(base={"chocolatey": str(choco)})

            self.assertEqual(items, [])

    def test_manager_root_missing_on_disk_is_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            does_not_exist = root / "nope"

            items = scan_scoop_choco(base={"scoop": str(does_not_exist)})

            self.assertEqual(items, [])


if __name__ == "__main__":
    unittest.main()
