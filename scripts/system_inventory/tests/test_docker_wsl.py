"""Unit tests for scripts/system_inventory/docker_wsl.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` — see that module's docstring for why this is the only
import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_docker_wsl``.
"""

from __future__ import annotations

import platform
import re
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.system_inventory.docker_wsl import (
    WINREG_AVAILABLE,
    _distro_items,
    _parse_lxss_distros,
    _vhdx_glob_items,
    scan_docker_wsl,
)

_DOCKER_WSL_SOURCE = (Path(__file__).resolve().parent.parent / "docker_wsl.py").read_text(encoding="utf-8")

# winreg write APIs: none of these must ever be *called* (i.e. referenced as
# ``winreg.<name>`` / ``winreg.<name>Ex``) by docker_wsl.py. Mirrors
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


class VhdxGlobItemsTests(unittest.TestCase):
    def test_finds_vhdx_under_data_disk_and_loose_at_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            wsl_root = Path(tmp) / "Docker" / "wsl"
            (wsl_root / "data").mkdir(parents=True)
            (wsl_root / "disk").mkdir(parents=True)

            (wsl_root / "data" / "docker_data.vhdx").write_bytes(b"x" * 100)
            (wsl_root / "disk" / "docker_disk.vhdx").write_bytes(b"x" * 50)
            (wsl_root / "ext4.vhdx").write_bytes(b"x" * 30)
            # A non-.vhdx file directly under the root must be ignored.
            (wsl_root / "readme.txt").write_bytes(b"ignored")

            items = _vhdx_glob_items(base={"docker_wsl_root": str(wsl_root)})

            by_name = {item.name: item for item in items}
            self.assertEqual(set(by_name), {"docker_data.vhdx", "docker_disk.vhdx", "ext4.vhdx"})
            self.assertEqual(by_name["docker_data.vhdx"].size_bytes, 100)
            self.assertEqual(by_name["docker_disk.vhdx"].size_bytes, 50)
            self.assertEqual(by_name["ext4.vhdx"].size_bytes, 30)
            for item in items:
                self.assertEqual(item.source, "docker-wsl")
                self.assertEqual(item.detail, {"kind": "vhdx"})
                self.assertIsNotNone(item.path)

    def test_missing_root_returns_empty_list(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing_root = Path(tmp) / "does_not_exist"
            items = _vhdx_glob_items(base={"docker_wsl_root": str(missing_root)})
            self.assertEqual(items, [])

    def test_root_present_but_no_vhdx_anywhere_returns_empty_list(self):
        with tempfile.TemporaryDirectory() as tmp:
            wsl_root = Path(tmp) / "Docker" / "wsl"
            wsl_root.mkdir(parents=True)
            items = _vhdx_glob_items(base={"docker_wsl_root": str(wsl_root)})
            self.assertEqual(items, [])

    def test_base_dict_missing_key_is_tolerated(self):
        self.assertEqual(_vhdx_glob_items(base={}), [])


class ParseLxssDistrosTests(unittest.TestCase):
    def test_valid_subkey_parsed(self):
        raw_subkeys = {
            "{guid-1}": {"DistributionName": "Ubuntu", "BasePath": "C:\\Packages\\Ubuntu\\LocalState"},
        }
        records = _parse_lxss_distros(raw_subkeys)
        self.assertEqual(records, [{"name": "Ubuntu", "base_path": "C:\\Packages\\Ubuntu\\LocalState"}])

    def test_subkey_missing_distribution_name_is_skipped(self):
        raw_subkeys = {"{guid-1}": {"BasePath": "C:\\somewhere"}}
        self.assertEqual(_parse_lxss_distros(raw_subkeys), [])

    def test_subkey_missing_base_path_is_skipped(self):
        raw_subkeys = {"{guid-1}": {"DistributionName": "Ubuntu"}}
        self.assertEqual(_parse_lxss_distros(raw_subkeys), [])

    def test_blank_values_are_skipped(self):
        raw_subkeys = {"{guid-1}": {"DistributionName": "   ", "BasePath": "C:\\somewhere"}}
        self.assertEqual(_parse_lxss_distros(raw_subkeys), [])

    def test_empty_dict_returns_empty_list(self):
        self.assertEqual(_parse_lxss_distros({}), [])

    def test_multiple_subkeys_all_parsed(self):
        raw_subkeys = {
            "{guid-1}": {"DistributionName": "Ubuntu", "BasePath": "C:\\Ubuntu"},
            "{guid-2}": {"DistributionName": "Debian", "BasePath": "C:\\Debian"},
        }
        records = _parse_lxss_distros(raw_subkeys)
        names = sorted(record["name"] for record in records)
        self.assertEqual(names, ["Debian", "Ubuntu"])


class DistroItemsTests(unittest.TestCase):
    def test_existing_vhdx_under_base_path_is_sized(self):
        with tempfile.TemporaryDirectory() as tmp:
            base_path = Path(tmp) / "Ubuntu" / "LocalState"
            base_path.mkdir(parents=True)
            (base_path / "ext4.vhdx").write_bytes(b"x" * 42)

            items = _distro_items([{"name": "Ubuntu", "base_path": str(base_path)}])

            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].name, "Ubuntu")
            self.assertEqual(items[0].size_bytes, 42)
            self.assertEqual(items[0].source, "docker-wsl")
            self.assertEqual(items[0].detail, {"distro": "Ubuntu", "kind": "wsl-distro"})
            self.assertEqual(items[0].path, str((base_path / "ext4.vhdx").resolve()))

    def test_missing_vhdx_under_base_path_is_skipped(self):
        with tempfile.TemporaryDirectory() as tmp:
            base_path = Path(tmp) / "NoVhdxHere"
            base_path.mkdir()

            items = _distro_items([{"name": "Ghost", "base_path": str(base_path)}])
            self.assertEqual(items, [])

    def test_nonexistent_base_path_is_skipped(self):
        items = _distro_items([{"name": "Ghost", "base_path": "Z:\\does\\not\\exist"}])
        self.assertEqual(items, [])


class ScanDockerWslTests(unittest.TestCase):
    """Exercises the merged scan_docker_wsl(), with Lxss enumeration patched
    to a deterministic fake (never the live registry) so results do not
    depend on whatever is actually installed on the machine running the
    suite.
    """

    def test_graceful_absence_returns_empty_list_without_raising(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing_root = Path(tmp) / "does_not_exist"
            with patch(
                "scripts.system_inventory.docker_wsl._iter_lxss_subkeys",
                return_value={},
            ):
                items = scan_docker_wsl(base={"docker_wsl_root": str(missing_root)})
            self.assertEqual(items, [])

    def test_every_returned_item_has_non_none_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            wsl_root = Path(tmp) / "Docker" / "wsl"
            (wsl_root / "data").mkdir(parents=True)
            (wsl_root / "data" / "docker_data.vhdx").write_bytes(b"x" * 10)

            distro_base = Path(tmp) / "Packages" / "Ubuntu" / "LocalState"
            distro_base.mkdir(parents=True)
            (distro_base / "ext4.vhdx").write_bytes(b"x" * 20)

            with patch(
                "scripts.system_inventory.docker_wsl._iter_lxss_subkeys",
                return_value={"{guid}": {"DistributionName": "Ubuntu", "BasePath": str(distro_base)}},
            ):
                items = scan_docker_wsl(base={"docker_wsl_root": str(wsl_root)})

            self.assertEqual(len(items), 2)
            for item in items:
                self.assertIsNotNone(item.path)
                self.assertEqual(item.source, "docker-wsl")

    def test_distro_vhdx_also_reachable_by_glob_is_deduped_keeping_distro_item(self):
        with tempfile.TemporaryDirectory() as tmp:
            # A distro whose BasePath sits directly under the Docker/wsl
            # root: the glob and the Lxss enumeration both find the exact
            # same ext4.vhdx file.
            wsl_root = Path(tmp) / "Docker" / "wsl"
            wsl_root.mkdir(parents=True)
            vhdx_path = wsl_root / "ext4.vhdx"
            vhdx_path.write_bytes(b"x" * 77)

            with patch(
                "scripts.system_inventory.docker_wsl._iter_lxss_subkeys",
                return_value={"{guid}": {"DistributionName": "docker-desktop", "BasePath": str(wsl_root)}},
            ):
                items = scan_docker_wsl(base={"docker_wsl_root": str(wsl_root)})

            # Deduped: exactly one item for this vhdx, and it is the
            # distro-tagged one (carries the distro name).
            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].detail, {"distro": "docker-desktop", "kind": "wsl-distro"})
            self.assertEqual(items[0].size_bytes, 77)

    def test_lxss_key_absent_but_vhdx_present_returns_glob_items_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            wsl_root = Path(tmp) / "Docker" / "wsl"
            wsl_root.mkdir(parents=True)
            (wsl_root / "solo.vhdx").write_bytes(b"x" * 15)

            with patch(
                "scripts.system_inventory.docker_wsl._iter_lxss_subkeys",
                return_value={},
            ):
                items = scan_docker_wsl(base={"docker_wsl_root": str(wsl_root)})

            self.assertEqual(len(items), 1)
            self.assertEqual(items[0].detail, {"kind": "vhdx"})


class DockerWslSourceReadOnlyContractTests(unittest.TestCase):
    """Verifies docker_wsl.py never references a winreg write API, mirroring
    test_registry.py's RegistrySourceReadOnlyContractTests.
    """

    def test_no_winreg_write_api_referenced(self):
        for write_api in _WINREG_WRITE_APIS:
            with self.subTest(write_api=write_api):
                pattern = re.compile(rf"winreg\.{re.escape(write_api)}\b")
                match = pattern.search(_DOCKER_WSL_SOURCE)
                self.assertIsNone(
                    match,
                    f"docker_wsl.py must never call the winreg write API winreg.{write_api}(...)",
                )


@unittest.skipUnless(WINREG_AVAILABLE and platform.system() == "Windows", "winreg only available on Windows")
class ScanDockerWslLiveSmokeTest(unittest.TestCase):
    """Runs the real scanner end-to-end; must not raise whether or not
    Docker/WSL is actually installed on this machine.
    """

    def test_does_not_raise_and_returns_a_list(self):
        items = scan_docker_wsl()

        self.assertIsInstance(items, list)
        for item in items:
            self.assertEqual(item.source, "docker-wsl")
            self.assertIsNotNone(item.path)


if __name__ == "__main__":
    unittest.main()
