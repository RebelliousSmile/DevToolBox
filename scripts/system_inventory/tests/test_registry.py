"""Unit tests for scripts/system_inventory/registry.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` — see that module's docstring for why this is the only
import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_registry``.
"""

from __future__ import annotations

import platform
import re
import unittest
from pathlib import Path

from scripts.system_inventory.registry import WINREG_AVAILABLE, _read_uninstall_entry, scan_registry

_REGISTRY_SOURCE = (Path(__file__).resolve().parent.parent / "registry.py").read_text(encoding="utf-8")

# winreg write APIs: none of these must ever be *called* (i.e. referenced as
# ``winreg.<name>`` / ``winreg.<name>Ex``) by registry.py. This enforces the
# read-only contract (risk register: "Accidental registry write")
# independently of code review. Matched with a ``winreg.`` prefix (rather
# than a bare substring search) so this test does not false-positive on the
# module's own prose docstring, which *names* these APIs in order to say
# they are deliberately not used.
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


class ReadUninstallEntryTests(unittest.TestCase):
    def test_estimated_size_kb_converted_to_bytes(self):
        entry = _read_uninstall_entry({"DisplayName": "Some App", "EstimatedSize": 2048})
        self.assertEqual(entry["size_bytes"], 2048 * 1024)

    def test_missing_estimated_size_is_none_not_fabricated(self):
        entry = _read_uninstall_entry({"DisplayName": "Some App"})
        self.assertIsNone(entry["size_bytes"])

    def test_non_int_estimated_size_is_none(self):
        entry = _read_uninstall_entry({"DisplayName": "Some App", "EstimatedSize": "not-an-int"})
        self.assertIsNone(entry["size_bytes"])

    def test_zero_estimated_size_is_not_treated_as_missing(self):
        entry = _read_uninstall_entry({"DisplayName": "Some App", "EstimatedSize": 0})
        self.assertEqual(entry["size_bytes"], 0)

    def test_install_date_is_captured(self):
        entry = _read_uninstall_entry({"DisplayName": "Some App", "InstallDate": "20260115"})
        self.assertEqual(entry["install_date"], "20260115")

    def test_missing_install_date_is_empty_string_not_none(self):
        entry = _read_uninstall_entry({"DisplayName": "Some App"})
        self.assertEqual(entry["install_date"], "")

    def test_install_location_is_captured(self):
        entry = _read_uninstall_entry(
            {"DisplayName": "Some App", "InstallLocation": "C:\\Program Files\\Some App"}
        )
        self.assertEqual(entry["install_location"], "C:\\Program Files\\Some App")

    def test_missing_install_location_is_empty_string(self):
        entry = _read_uninstall_entry({"DisplayName": "Some App"})
        self.assertEqual(entry["install_location"], "")

    def test_missing_display_name_skips_entry_entirely(self):
        entry = _read_uninstall_entry({"EstimatedSize": 100, "InstallDate": "20260101"})
        self.assertIsNone(entry)

    def test_blank_display_name_skips_entry_entirely(self):
        entry = _read_uninstall_entry({"DisplayName": "   "})
        self.assertIsNone(entry)

    def test_empty_values_dict_skips_entry(self):
        entry = _read_uninstall_entry({})
        self.assertIsNone(entry)

    def test_full_entry_all_fields_present(self):
        entry = _read_uninstall_entry(
            {
                "DisplayName": "Full App",
                "InstallDate": "20250630",
                "EstimatedSize": 51200,
                "InstallLocation": "C:\\Apps\\Full App",
            }
        )
        self.assertEqual(
            entry,
            {
                "name": "Full App",
                "size_bytes": 51200 * 1024,
                "install_date": "20250630",
                "install_location": "C:\\Apps\\Full App",
            },
        )


class RegistrySourceReadOnlyContractTests(unittest.TestCase):
    """Verifies registry.py never references a winreg write API, per the
    plan's risk register ("Accidental registry write / breaks read-only
    contract"). Pure source-text scan: no live registry access involved.
    """

    def test_no_winreg_write_api_referenced(self):
        for write_api in _WINREG_WRITE_APIS:
            with self.subTest(write_api=write_api):
                pattern = re.compile(rf"winreg\.{re.escape(write_api)}\b")
                match = pattern.search(_REGISTRY_SOURCE)
                self.assertIsNone(
                    match,
                    f"registry.py must never call the winreg write API winreg.{write_api}(...)",
                )


@unittest.skipUnless(WINREG_AVAILABLE and platform.system() == "Windows", "winreg only available on Windows")
class ScanRegistryLiveSmokeTest(unittest.TestCase):
    """Runs the real scanner against this machine's live registry.

    Guarded to Windows only. On this development machine (Windows, Python
    3.13) it is expected to actually execute, not merely be skipped: a
    Windows machine always has at least one entry under
    HKLM\\...\\Uninstall (Windows itself ships components registered there).
    """

    def test_returns_at_least_one_item_without_raising(self):
        items = scan_registry()

        self.assertGreater(len(items), 0)
        for item in items:
            self.assertEqual(item.source, "registry")
            self.assertTrue(item.name)
            self.assertIn(item.detail["hive"], ("HKLM", "HKCU"))
            self.assertIn(item.detail["view"], ("32", "64"))


@unittest.skipUnless(platform.system() == "Linux", "Linux dispatch only")
class ScanRegistryLinuxDispatchLiveSmokeTest(unittest.TestCase):
    """Runs the real scanner on Linux: it must dispatch to systemd.scan_systemd()
    (Part 4 Phase 2) rather than raise ``WinregUnavailableError``.

    Guarded to Linux only, mirroring ``ScanRegistryLiveSmokeTest`` above for
    Windows. Containers and WSL hosts may legitimately return no units.
    """

    def test_dispatches_to_systemd_without_raising(self):
        items = scan_registry()

        for item in items:
            self.assertEqual(item.source, "systemd")
            self.assertTrue(item.name)
            self.assertIn(item.detail["kind"], ("service", "timer"))


if __name__ == "__main__":
    unittest.main()
