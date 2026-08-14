"""Unit tests for scripts/system_inventory/packages_linux.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` — see that module's docstring for why this is the only
import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_packages_linux``.

Mocking pattern: same as ``scripts/winclean/tests/test_mod_apps.py``'s
``DockerLightTest`` — ``mock.patch.object(module.shutil, "which", ...)`` to
control manager detection, ``mock.patch.object(module, "_run", ...)`` to
control the subprocess result, so no real package manager needs to be
installed (or absent) on the machine running the suite.
"""

from __future__ import annotations

import subprocess
import unittest
from unittest import mock

from scripts.system_inventory import packages_linux


def _completed(returncode: int, stdout: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(args=[], returncode=returncode, stdout=stdout, stderr="")


def _which_side_effect(available: str) -> object:
    def side_effect(binary: str) -> str | None:
        return f"/usr/bin/{binary}" if binary == available else None

    return side_effect


class ScanPackagesLinuxDetectionTests(unittest.TestCase):
    def test_no_manager_detected_returns_empty_list(self):
        with mock.patch.object(packages_linux.shutil, "which", return_value=None):
            self.assertEqual(packages_linux.scan_packages_linux(), [])

    def test_dpkg_detected_and_parsed(self):
        output = "vim\t1234\tinstall ok installed\nbash\t5678\tinstall ok installed\n"
        with mock.patch.object(packages_linux.shutil, "which", side_effect=_which_side_effect("dpkg-query")):
            with mock.patch.object(packages_linux, "_run", return_value=_completed(0, output)):
                items = packages_linux.scan_packages_linux()

        by_name = {item.name: item for item in items}
        self.assertEqual(set(by_name), {"vim", "bash"})
        self.assertEqual(by_name["vim"].size_bytes, 1234 * 1024)
        self.assertEqual(by_name["vim"].detail, {"manager": "apt"})
        self.assertEqual(by_name["vim"].source, "packages-linux")
        self.assertIsNone(by_name["vim"].path)

    def test_dpkg_detected_takes_priority_over_rpm_and_pacman(self):
        with mock.patch.object(packages_linux.shutil, "which", return_value="/usr/bin/anything"):
            output = "pkg\t1\tinstall ok installed\n"
            with mock.patch.object(packages_linux, "_run", return_value=_completed(0, output)) as run_mock:
                packages_linux.scan_packages_linux()

        run_mock.assert_called_once_with(packages_linux._DPKG_QUERY_COMMAND)

    def test_rpm_detected_when_dpkg_absent(self):
        output = "httpd\t2048\n"
        with mock.patch.object(packages_linux.shutil, "which", side_effect=_which_side_effect("rpm")):
            with mock.patch.object(packages_linux, "_run", return_value=_completed(0, output)):
                items = packages_linux.scan_packages_linux()

        self.assertEqual(len(items), 1)
        self.assertEqual(items[0].name, "httpd")
        self.assertEqual(items[0].size_bytes, 2048)
        self.assertEqual(items[0].detail, {"manager": "dnf"})

    def test_pacman_detected_when_others_absent(self):
        output = "Name            : coreutils\nInstalled Size  : 12.34 MiB\n"
        with mock.patch.object(packages_linux.shutil, "which", side_effect=_which_side_effect("pacman")):
            with mock.patch.object(packages_linux, "_run", return_value=_completed(0, output)):
                items = packages_linux.scan_packages_linux()

        self.assertEqual(len(items), 1)
        self.assertEqual(items[0].name, "coreutils")
        self.assertEqual(items[0].size_bytes, int(12.34 * 1024**2))
        self.assertEqual(items[0].detail, {"manager": "pacman"})

    def test_nonzero_exit_code_yields_empty_list(self):
        with mock.patch.object(packages_linux.shutil, "which", side_effect=_which_side_effect("dpkg-query")):
            with mock.patch.object(packages_linux, "_run", return_value=_completed(1, "")):
                self.assertEqual(packages_linux.scan_packages_linux(), [])

    def test_run_raising_internally_yields_empty_list(self):
        # _run() itself never raises (it catches OSError/SubprocessError) —
        # this exercises the None-result path that represents that.
        with mock.patch.object(packages_linux.shutil, "which", side_effect=_which_side_effect("dpkg-query")):
            with mock.patch.object(packages_linux, "_run", return_value=None):
                self.assertEqual(packages_linux.scan_packages_linux(), [])


class ParseDpkgQueryTests(unittest.TestCase):
    def test_blank_lines_and_malformed_rows_are_skipped(self):
        output = (
            "good\t100\tinstall ok installed\n"
            "\n"
            "malformed_no_tab\n"
            "another\t50\tinstall ok installed\n"
        )
        items = packages_linux._parse_dpkg_query(output)
        self.assertEqual([item.name for item in items], ["good", "another"])

    def test_missing_installed_size_yields_none_size_but_keeps_package(self):
        output = "third-party-pkg\t\tinstall ok installed\n"
        items = packages_linux._parse_dpkg_query(output)
        self.assertEqual([item.name for item in items], ["third-party-pkg"])
        self.assertIsNone(items[0].size_bytes)

    def test_deinstalled_config_files_row_is_skipped(self):
        output = "removed-pkg\t100\tdeinstall ok config-files\nkept-pkg\t50\tinstall ok installed\n"
        items = packages_linux._parse_dpkg_query(output)
        self.assertEqual([item.name for item in items], ["kept-pkg"])

    def test_public_size_query_reuses_read_only_dpkg_parser(self):
        output = "desktop-app\t2048\tinstall ok installed\n"
        with mock.patch.object(packages_linux.shutil, "which", return_value="/usr/bin/dpkg-query"):
            with mock.patch.object(packages_linux, "_run", return_value=_completed(0, output)):
                sizes = packages_linux.query_dpkg_installed_sizes()
        self.assertEqual(sizes, {"desktop-app": 2048 * 1024})


class ParsePacmanQueryTests(unittest.TestCase):
    def test_multiple_blocks_separated_by_blank_lines(self):
        output = (
            "Name            : pkg-one\n"
            "Installed Size  : 1.00 KiB\n"
            "\n"
            "Name            : pkg-two\n"
            "Installed Size  : 2.00 GiB\n"
        )
        items = packages_linux._parse_pacman_query(output)
        by_name = {item.name: item for item in items}
        self.assertEqual(set(by_name), {"pkg-one", "pkg-two"})
        self.assertEqual(by_name["pkg-one"].size_bytes, 1024)
        self.assertEqual(by_name["pkg-two"].size_bytes, 2 * 1024**3)

    def test_missing_installed_size_yields_none_size(self):
        output = "Name            : pkg-one\n"
        items = packages_linux._parse_pacman_query(output)
        self.assertEqual(items[0].size_bytes, None)

    def test_unrecognized_unit_yields_none_size(self):
        self.assertIsNone(packages_linux._parse_pacman_size("5 TiB"))


if __name__ == "__main__":
    unittest.main()
