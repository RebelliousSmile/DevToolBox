"""Unit tests for scripts/system_inventory/docker_native.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` — see that module's docstring for why this is the only
import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_docker_native``.

Mocking pattern: same as ``test_packages_linux.py``/``test_systemd.py`` —
``mock.patch.object(module.shutil, "which", ...)`` for binary detection,
``mock.patch.object(module, "_run", ...)`` for subprocess results,
``mock.patch.object(module.os.path, "isdir", ...)`` for the ``/var/lib/docker``
fallback's presence check.
"""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.system_inventory import docker_native


def _completed(returncode: int, stdout: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(args=[], returncode=returncode, stdout=stdout, stderr="")


class ParseDockerSizeTests(unittest.TestCase):
    def test_gigabytes_parsed(self):
        self.assertEqual(docker_native._parse_docker_size("14.9GB"), int(14.9 * 1000**3))

    def test_megabytes_parsed(self):
        self.assertEqual(docker_native._parse_docker_size("44.22MB"), int(44.22 * 1000**2))

    def test_bare_bytes_parsed(self):
        self.assertEqual(docker_native._parse_docker_size("0B"), 0)

    def test_trailing_percentage_suffix_is_stripped(self):
        self.assertEqual(docker_native._parse_docker_size("6.964GB (46%)"), int(6.964 * 1000**3))

    def test_blank_value_yields_none(self):
        self.assertIsNone(docker_native._parse_docker_size(""))

    def test_unrecognized_unit_yields_none(self):
        self.assertIsNone(docker_native._parse_docker_size("5 XB"))

    def test_malformed_value_yields_none(self):
        self.assertIsNone(docker_native._parse_docker_size("not-a-size"))


class ParseDockerSystemDfTests(unittest.TestCase):
    def test_ndjson_lines_parsed_into_one_item_per_category(self):
        output = (
            '{"Active":"13","Reclaimable":"6.964GB (46%)","Size":"14.9GB","TotalCount":"51","Type":"Images"}\n'
            '{"Active":"14","Reclaimable":"0B (0%)","Size":"44.22MB","TotalCount":"14","Type":"Containers"}\n'
        )
        items = docker_native._parse_docker_system_df(output)

        by_name = {item.name: item for item in items}
        self.assertEqual(set(by_name), {"Images", "Containers"})
        self.assertEqual(by_name["Images"].source, "docker-native")
        self.assertEqual(by_name["Images"].path, None)
        self.assertEqual(by_name["Images"].size_bytes, int(14.9 * 1000**3))
        self.assertEqual(
            by_name["Images"].detail,
            {"active": "13", "total_count": "51", "reclaimable": "6.964GB (46%)"},
        )

    def test_blank_lines_are_skipped(self):
        output = '\n{"Type":"Build Cache","Size":"0B"}\n\n'
        items = docker_native._parse_docker_system_df(output)
        self.assertEqual([item.name for item in items], ["Build Cache"])

    def test_malformed_json_line_is_skipped_not_fatal(self):
        output = 'not json\n{"Type":"Images","Size":"1GB"}\n'
        items = docker_native._parse_docker_system_df(output)
        self.assertEqual([item.name for item in items], ["Images"])

    def test_entry_missing_type_is_skipped(self):
        output = '{"Size":"1GB"}\n{"Type":"Images","Size":"1GB"}\n'
        items = docker_native._parse_docker_system_df(output)
        self.assertEqual([item.name for item in items], ["Images"])


class ScanDockerNativeTests(unittest.TestCase):
    def test_docker_present_parses_ndjson(self):
        output = '{"Type":"Images","Size":"1GB","Active":"1","TotalCount":"1","Reclaimable":"0B"}\n'
        with mock.patch.object(docker_native.shutil, "which", return_value="/usr/bin/docker"):
            with mock.patch.object(docker_native, "_run", return_value=_completed(0, output)):
                items = docker_native.scan_docker_native()

        self.assertEqual([item.name for item in items], ["Images"])
        for item in items:
            self.assertEqual(item.source, "docker-native")

    def test_docker_command_nonzero_exit_falls_back_to_directory(self):
        with mock.patch.object(docker_native.shutil, "which", return_value="/usr/bin/docker"):
            with mock.patch.object(docker_native, "_run", return_value=_completed(1, "")):
                with mock.patch.object(docker_native.os.path, "isdir", return_value=False):
                    items = docker_native.scan_docker_native()
        self.assertEqual(items, [])

    def test_docker_output_with_nothing_parseable_falls_back_to_directory(self):
        with mock.patch.object(docker_native.shutil, "which", return_value="/usr/bin/docker"):
            with mock.patch.object(docker_native, "_run", return_value=_completed(0, "not json")):
                with mock.patch.object(docker_native.os.path, "isdir", return_value=False):
                    items = docker_native.scan_docker_native()
        self.assertEqual(items, [])

    def test_docker_absent_falls_back_to_directory_sizing(self):
        with tempfile.TemporaryDirectory() as tmp:
            fake_var_lib_docker = Path(tmp) / "docker"
            fake_var_lib_docker.mkdir()
            (fake_var_lib_docker / "layer.bin").write_bytes(b"x" * 123)

            with mock.patch.object(docker_native.shutil, "which", return_value=None):
                with mock.patch.object(
                    docker_native, "_VAR_LIB_DOCKER_PATH", str(fake_var_lib_docker)
                ):
                    items = docker_native.scan_docker_native()

        self.assertEqual(len(items), 1)
        self.assertEqual(items[0].source, "docker-native")
        self.assertEqual(items[0].path, str(fake_var_lib_docker))
        self.assertEqual(items[0].size_bytes, 123)
        self.assertEqual(items[0].detail, {"kind": "fallback-dir"})

    def test_docker_absent_and_var_lib_docker_missing_returns_empty_list(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing = Path(tmp) / "does_not_exist"
            with mock.patch.object(docker_native.shutil, "which", return_value=None):
                with mock.patch.object(docker_native, "_VAR_LIB_DOCKER_PATH", str(missing)):
                    items = docker_native.scan_docker_native()
        self.assertEqual(items, [])

    def test_docker_absent_and_var_lib_docker_unreadable_yields_partial_zero_size(self):
        # Confirmed on this dev machine: a non-root user gets Permission
        # denied listing /var/lib/docker. dir_size_on_disk's own
        # OSError/PermissionError tolerance (see common.py) means this must
        # still yield an item, with size_bytes=0 (nothing readable) rather
        # than raising. Exercised with a real unreadable subdirectory
        # (chmod 0), not a mock, so this actually proves the OS-level
        # tolerance rather than assuming it.
        with tempfile.TemporaryDirectory() as tmp:
            fake_var_lib_docker = Path(tmp) / "docker"
            locked = fake_var_lib_docker / "locked"
            locked.mkdir(parents=True)
            locked.chmod(0o000)
            try:
                with mock.patch.object(docker_native.shutil, "which", return_value=None):
                    with mock.patch.object(
                        docker_native, "_VAR_LIB_DOCKER_PATH", str(fake_var_lib_docker)
                    ):
                        items = docker_native.scan_docker_native()
            finally:
                locked.chmod(0o755)

        self.assertEqual(len(items), 1)
        self.assertEqual(items[0].size_bytes, 0)


if __name__ == "__main__":
    unittest.main()
