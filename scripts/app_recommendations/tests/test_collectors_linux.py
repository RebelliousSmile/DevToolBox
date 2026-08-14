from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.app_recommendations.collectors import linux

FIXTURES = Path(__file__).parent / "fixtures/linux"


class DesktopEntryTests(unittest.TestCase):
    def test_desktop_fixture_documents_visible_and_hidden_entries(self):
        rows = json.loads((FIXTURES / "desktop_entries.json").read_text())
        self.assertEqual(rows[0]["package"], "editor")
        self.assertTrue(rows[1]["hidden"])

    def test_exec_normalization_and_shared_wrappers(self):
        self.assertEqual(linux._command_from_exec("env FOO=bar /usr/bin/editor %F"), "/usr/bin/editor")
        self.assertIsNone(linux._command_from_exec("flatpak run org.example.Editor"))
        self.assertIsNone(linux._command_from_exec("/snap/bin/editor %U"))

    def test_hidden_and_non_application_entries_are_excluded(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "visible.desktop").write_text("[Desktop Entry]\nType=Application\nName=Editor\nExec=/usr/bin/editor %F\n")
            (root / "hidden.desktop").write_text("[Desktop Entry]\nType=Application\nName=Hidden\nExec=/usr/bin/hidden\nNoDisplay=true\n")
            self.assertEqual(linux._iter_desktop_apps([root]), [("Editor", "/usr/bin/editor")])


class AptCollectorTests(unittest.TestCase):
    def test_only_desktop_owned_packages_are_candidates(self):
        with mock.patch.object(linux.shutil, "which", return_value="/usr/bin/dpkg-query"):
            with mock.patch.object(linux, "_dpkg_owners", return_value={"/usr/bin/editor": "editor"}):
                items = linux.collect_apt_apps(
                    [("Editor", "/usr/bin/editor")],
                    {"editor": 10_000, "libunused": 50_000},
                    set(),
                )
        self.assertEqual([item.app_id for item in items], ["apt:editor"])
        self.assertEqual(items[0].size.scope, "paquet hors données utilisateur")

    def test_auto_package_is_protected_and_has_no_command(self):
        with mock.patch.object(linux.shutil, "which", return_value="/usr/bin/dpkg-query"):
            with mock.patch.object(linux, "_dpkg_owners", return_value={"/usr/bin/helper": "helper"}):
                item = linux.collect_apt_apps([("Helper", "/usr/bin/helper")], {"helper": 1}, {"helper"})[0]
        self.assertTrue(item.protection.protected)
        self.assertIsNone(item.command)

    def test_owner_lookup_is_batched_and_ambiguous_paths_are_ignored(self):
        output = "editor: /usr/bin/editor\nfirst, second: /usr/bin/shared\n"
        with mock.patch.object(linux, "_run_partial", return_value=output) as run:
            owners = linux._dpkg_owners(["/usr/bin/editor", "/usr/bin/shared"])
        self.assertEqual(owners, {"/usr/bin/editor": "editor"})
        run.assert_called_once()


class SnapCollectorTests(unittest.TestCase):
    def test_base_is_protected_and_archive_size_is_not_reclaimable(self):
        output = json.loads((FIXTURES / "snap_list.json").read_text())["output"]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = root / "archives"
            archive.mkdir()
            (archive / "editor_42.snap").write_bytes(b"x" * 7)
            items = linux.collect_snap_apps(output, root / "snap", archive)
        by_id = {item.app_id: item for item in items}
        self.assertEqual(by_id["snap:editor"].size.installed_bytes, 7)
        self.assertIsNone(by_id["snap:editor"].size.reclaimable_bytes)
        self.assertTrue(by_id["snap:core22"].protection.protected)
        self.assertEqual(by_id["snap:editor"].executable_hints, [])


class FlatpakCollectorTests(unittest.TestCase):
    def test_apps_only_parser_keeps_runtime_separate_and_no_generic_hint(self):
        output = json.loads((FIXTURES / "flatpak_list.json").read_text())["output"]
        with mock.patch.object(linux, "_specific_executable", return_value=None):
            item = linux.collect_flatpak_apps(output)[0]
        self.assertEqual(item.app_id, "flatpak:org.example.Editor")
        self.assertEqual(item.size.installed_bytes, 2_500_000_000)
        self.assertIn("hors runtime", item.size.scope)
        self.assertEqual(item.executable_hints, [])
        self.assertIn("--user", item.command.value)


if __name__ == "__main__":
    unittest.main()
