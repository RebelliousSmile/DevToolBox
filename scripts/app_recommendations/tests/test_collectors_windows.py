from __future__ import annotations

import json
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest import mock

from scripts.app_recommendations.collectors import windows
from scripts.app_recommendations.report import build_report
from scripts.system_inventory.common import InventoryItem

FIXTURES = Path(__file__).parent / "fixtures/windows"


def load(name: str):
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


class RegistryCandidatesTests(unittest.TestCase):
    def test_uninstall_string_is_opaque_and_system_component_is_protected(self):
        candidates = windows.registry_candidates(load("uninstall_registry.json"))
        by_name = {candidate.name: candidate for candidate in candidates}
        editor = by_name["Editor"]
        self.assertEqual(editor.command.origin, "publisher_unverified")
        self.assertEqual(editor.command.value, '"C:\\Apps\\Editor\\uninstall.exe" /remove')
        self.assertEqual(editor.executable_hints, ["C:\\Apps\\Editor\\editor.exe"])
        self.assertEqual(editor.metadata["version"], "4.2.0")
        update = by_name["KB500000"]
        self.assertTrue(update.protection.protected)
        self.assertIsNone(update.command)
        self.assertIsNone(update.size.installed_bytes)

    def test_generic_registry_executable_is_not_an_usage_hint(self):
        self.assertIsNone(windows._display_icon_executable("C:\\Windows\\System32\\msiexec.exe,0"))


class AppxCandidatesTests(unittest.TestCase):
    def test_main_package_and_framework_are_distinguished(self):
        with mock.patch.object(windows, "_find_windows_executable", return_value=None):
            candidates = windows.appx_candidates(load("appx_packages.json"))
        by_name = {candidate.name: candidate for candidate in candidates}
        self.assertFalse(by_name["Contoso.Editor"].protection.protected)
        self.assertEqual(by_name["Contoso.Editor"].command.origin, "manager_verified")
        self.assertTrue(by_name["Contoso.Runtime"].protection.protected)
        self.assertIsNone(by_name["Contoso.Runtime"].command)
        self.assertEqual(by_name["Contoso.Editor"].executable_hints, [])


class PackageManagerCandidatesTests(unittest.TestCase):
    def items(self):
        return [InventoryItem(**row) for row in load("package_managers.json")]

    def test_commands_are_manager_built_and_unknown_size_stays_unknown(self):
        with mock.patch.object(windows, "_find_windows_executable", return_value=None):
            candidates = windows.package_manager_candidates(self.items())
        by_source = {candidate.source: candidate for candidate in candidates}
        self.assertEqual(by_source["scoop"].command.value, "scoop uninstall Editor")
        self.assertIn('"Node JS"', by_source["chocolatey"].command.value)
        self.assertIsNone(by_source["chocolatey"].size.installed_bytes)

    def test_exact_location_and_name_deduplicate_without_adding_sizes(self):
        registry = windows.registry_candidates(load("uninstall_registry.json"))[0]
        with mock.patch.object(windows, "_find_windows_executable", return_value=None):
            scoop = windows.package_manager_candidates(self.items())[0]
        report = build_report(
            {"registry": lambda: [registry], "scoop": lambda: [scoop]},
            generated_at=datetime(2026, 1, 1, tzinfo=timezone.utc),
            platform_name="windows",
        )
        self.assertEqual(len(report.candidates), 1)
        self.assertEqual(report.candidates[0].size.installed_bytes, 600000000)
        self.assertEqual(report.candidates[0].command.origin, "manager_verified")

    def test_ambiguous_names_at_different_locations_remain_distinct(self):
        registry = windows.registry_candidates(load("uninstall_registry.json"))[0]
        other = self.items()[0]
        other.path = "C:\\Other\\Editor"
        with mock.patch.object(windows, "_find_windows_executable", return_value=None):
            scoop = windows.package_manager_candidates([other])[0]
        report = build_report({"registry": lambda: [registry], "scoop": lambda: [scoop]})
        self.assertEqual(len(report.candidates), 2)


class PlatformGuardsTests(unittest.TestCase):
    def test_live_collectors_refuse_non_windows_platform(self):
        with mock.patch.object(windows.sys, "platform", "linux"):
            with self.assertRaises(RuntimeError):
                windows.collect_registry_apps()
            with self.assertRaises(RuntimeError):
                windows.collect_appx_apps()
            with self.assertRaises(RuntimeError):
                windows.collect_package_manager_apps()


if __name__ == "__main__":
    unittest.main()
