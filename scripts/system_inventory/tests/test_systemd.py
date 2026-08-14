"""Unit tests for scripts/system_inventory/systemd.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` — see that module's docstring for why this is the only
import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_systemd``.

Mocking pattern: same as ``test_packages_linux.py`` /
``scripts/winclean/tests/test_mod_apps.py``'s ``DockerLightTest`` —
``mock.patch.object(module.shutil, "which", ...)`` for binary detection,
``mock.patch.object(module, "_run", ...)`` for subprocess results.
"""

from __future__ import annotations

import json
import subprocess
import unittest
from unittest import mock

from scripts.system_inventory import systemd


def _completed(returncode: int, stdout: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(args=[], returncode=returncode, stdout=stdout, stderr="")


class ScanSystemdTests(unittest.TestCase):
    def test_systemctl_absent_returns_empty_list(self):
        with mock.patch.object(systemd.shutil, "which", return_value=None):
            self.assertEqual(systemd.scan_systemd(), [])

    def test_services_and_timers_both_parsed_and_tagged(self):
        services_json = json.dumps(
            [{"unit": "acpid.service", "load": "loaded", "active": "active", "sub": "running"}]
        )
        timers_json = json.dumps([{"unit": "fwupd.timer", "activates": "fwupd.service"}])

        def fake_run(command):
            if command == systemd._LIST_SERVICES_COMMAND:
                return _completed(0, services_json)
            if command == systemd._LIST_TIMERS_COMMAND:
                return _completed(0, timers_json)
            raise AssertionError(f"unexpected command {command!r}")

        with mock.patch.object(systemd.shutil, "which", return_value="/usr/bin/systemctl"):
            with mock.patch.object(systemd, "_run", side_effect=fake_run):
                items = systemd.scan_systemd()

        by_name = {item.name: item for item in items}
        self.assertEqual(set(by_name), {"acpid.service", "fwupd.timer"})
        self.assertIsNone(by_name["acpid.service"].size_bytes)
        self.assertEqual(by_name["acpid.service"].source, "systemd")
        self.assertEqual(
            by_name["acpid.service"].detail,
            {"kind": "service", "load": "loaded", "active": "active", "sub": "running"},
        )
        self.assertEqual(by_name["fwupd.timer"].detail, {"kind": "timer", "activates": "fwupd.service"})

    def test_services_query_failing_still_returns_timers(self):
        timers_json = json.dumps([{"unit": "fwupd.timer", "activates": "fwupd.service"}])

        def fake_run(command):
            if command == systemd._LIST_SERVICES_COMMAND:
                return _completed(1, "")
            return _completed(0, timers_json)

        with mock.patch.object(systemd.shutil, "which", return_value="/usr/bin/systemctl"):
            with mock.patch.object(systemd, "_run", side_effect=fake_run):
                items = systemd.scan_systemd()

        self.assertEqual([item.name for item in items], ["fwupd.timer"])

    def test_malformed_json_yields_empty_list_without_raising(self):
        with mock.patch.object(systemd.shutil, "which", return_value="/usr/bin/systemctl"):
            with mock.patch.object(systemd, "_run", return_value=_completed(0, "not json")):
                self.assertEqual(systemd.scan_systemd(), [])

    def test_entries_missing_unit_key_are_skipped(self):
        services_json = json.dumps([{"load": "loaded"}, {"unit": "ok.service", "load": "loaded"}])
        with mock.patch.object(systemd.shutil, "which", return_value="/usr/bin/systemctl"):
            with mock.patch.object(systemd, "_run", return_value=_completed(0, services_json)):
                items = systemd._parse_services(services_json)
        self.assertEqual([item.name for item in items], ["ok.service"])


if __name__ == "__main__":
    unittest.main()
