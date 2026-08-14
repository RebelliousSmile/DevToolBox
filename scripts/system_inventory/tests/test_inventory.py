"""Unit tests for scripts/system_inventory/inventory.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` and ``test_registry.py`` — see ``test_common.py``'s
docstring for why this is the only import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_inventory``.

Deliberately does not call the real ``scan_registry()``: every test here
patches ``inventory.SCANNERS`` with a fake, deterministic in-memory source,
so ordering/total/``--json``-shape assertions are independent of whatever
happens to be installed on the machine running the suite.
"""

from __future__ import annotations

import io
import json
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

from scripts.system_inventory.common import InventoryItem
from scripts.system_inventory.inventory import SCANNERS, main


def _fake_registry_items() -> list[InventoryItem]:
    return [
        InventoryItem("registry", "Big App", "C:\\Big", 3000, {"hive": "HKLM", "view": "64"}),
        InventoryItem("registry", "Small App", "C:\\Small", 100, {"hive": "HKLM", "view": "64"}),
        InventoryItem("registry", "Unknown App", None, None, {"hive": "HKCU", "view": "64"}),
        InventoryItem("registry", "Medium App", None, 500, {"hive": "HKLM", "view": "32"}),
    ]


def _fake_docker_wsl_items() -> list[InventoryItem]:
    return [
        InventoryItem(
            "docker-wsl",
            "ext4.vhdx",
            "C:\\LocalAppData\\Docker\\wsl\\data\\ext4.vhdx",
            5000,
            {"kind": "vhdx"},
        ),
        InventoryItem(
            "docker-wsl",
            "Ubuntu",
            "C:\\LocalAppData\\Packages\\Ubuntu\\LocalState\\ext4.vhdx",
            7000,
            {"distro": "Ubuntu", "kind": "wsl-distro"},
        ),
    ]


def _run(argv: list[str]) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        code = main(argv)
    return code, out.getvalue(), err.getvalue()


class InventoryCliWithFakeSourceTests(unittest.TestCase):
    """Exercises the orchestrator with SCANNERS replaced by a fake source."""

    def setUp(self):
        patcher = patch.dict(SCANNERS, {"registry": _fake_registry_items}, clear=True)
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_text_report_sorted_descending_with_total(self):
        code, out, err = _run([])
        self.assertEqual(code, 0)
        self.assertEqual(err, "")

        item_lines = [line for line in out.splitlines() if line.startswith("[registry]")]
        names = [line.split("]", 1)[1].split(" — ", 1)[0].strip() for line in item_lines]
        self.assertEqual(names, ["Big App", "Medium App", "Small App", "Unknown App"])
        self.assertIn("Total: 3.5 KB (3600 bytes)", out)

    def test_json_shape_items_and_total(self):
        code, out, err = _run(["--json"])
        self.assertEqual(code, 0)
        self.assertEqual(err, "")

        payload = json.loads(out)
        self.assertEqual(set(payload), {"items", "total_bytes", "total_human"})
        self.assertEqual(payload["total_bytes"], 3600)
        self.assertEqual(payload["total_human"], "3.5 KB")

        names = [item["name"] for item in payload["items"]]
        self.assertEqual(names, ["Big App", "Medium App", "Small App", "Unknown App"])

        known_sizes = [item["size_bytes"] for item in payload["items"] if item["size_bytes"] is not None]
        self.assertEqual(known_sizes, sorted(known_sizes, reverse=True))

        self.assertEqual(
            payload["items"][0],
            {
                "source": "registry",
                "name": "Big App",
                "path": "C:\\Big",
                "size_bytes": 3000,
                "detail": {"hive": "HKLM", "view": "64"},
            },
        )

    def test_top_caps_displayed_lines_but_not_the_grand_total_text(self):
        code, out, _err = _run(["--top", "1"])
        self.assertEqual(code, 0)

        item_lines = [line for line in out.splitlines() if line.startswith("[registry]")]
        self.assertEqual(len(item_lines), 1)
        self.assertIn("Big App", item_lines[0])
        # The grand total must still reflect all four scanned items, not just
        # the single displayed one.
        self.assertIn("Total: 3.5 KB (3600 bytes)", out)

    def test_top_caps_json_items_but_not_the_grand_total(self):
        code, out, _err = _run(["--json", "--top", "2"])
        self.assertEqual(code, 0)
        payload = json.loads(out)

        self.assertEqual(len(payload["items"]), 2)
        self.assertEqual([item["name"] for item in payload["items"]], ["Big App", "Medium App"])
        # Same guarantee as the text report: total is over all scanned items.
        self.assertEqual(payload["total_bytes"], 3600)
        self.assertEqual(payload["total_human"], "3.5 KB")

    def test_top_zero_shows_no_items_but_keeps_total(self):
        code, out, _err = _run(["--json", "--top", "0"])
        self.assertEqual(code, 0)
        payload = json.loads(out)
        self.assertEqual(payload["items"], [])
        self.assertEqual(payload["total_bytes"], 3600)

    def test_explicit_source_flag_selects_registered_fake_source(self):
        code, out, _err = _run(["--source", "registry", "--json"])
        self.assertEqual(code, 0)
        payload = json.loads(out)
        self.assertEqual(len(payload["items"]), 4)

    def test_unknown_source_name_rejected_by_argparse_with_exit_2(self):
        with self.assertRaises(SystemExit) as ctx:
            _run(["--source", "bogus"])
        self.assertEqual(ctx.exception.code, 2)

    def test_negative_top_is_rejected_with_exit_2(self):
        with self.assertRaises(SystemExit) as ctx:
            _run(["--top", "-1"])
        self.assertEqual(ctx.exception.code, 2)


class NoActiveSourceGuardTests(unittest.TestCase):
    """Covers the "no valid source available" non-zero-exit guard."""

    def test_empty_scanner_registry_triggers_nonzero_exit_and_stderr_message(self):
        with patch.dict(SCANNERS, {}, clear=True):
            code, out, err = _run([])

        self.assertNotEqual(code, 0)
        self.assertEqual(out, "")
        self.assertIn("Aucune source disponible", err)

    def test_path_requested_but_winreg_unavailable_triggers_guard(self):
        # "path" has no Linux dispatch of its own (Windows PATH lives only
        # in the registry — see Part 4 plan Amendments), so it stays in
        # _WINREG_DEPENDENT_SOURCES and is pre-filtered when WINREG_AVAILABLE
        # is False.
        with (
            patch.dict(SCANNERS, {"path": lambda: []}, clear=True),
            patch("scripts.system_inventory.inventory.WINREG_AVAILABLE", False),
        ):
            code, out, err = _run([])

        self.assertNotEqual(code, 0)
        self.assertEqual(out, "")
        self.assertIn("winreg", err)

    def test_registry_requested_with_winreg_unavailable_still_runs(self):
        # Part 4 Phase 2: "registry" now dispatches internally to
        # systemd.py on Linux instead of hard-requiring winreg, so it must
        # no longer be pre-filtered by WINREG_AVAILABLE alone.
        with (
            patch.dict(SCANNERS, {"registry": _fake_registry_items}, clear=True),
            patch("scripts.system_inventory.inventory.WINREG_AVAILABLE", False),
        ):
            code, out, err = _run([])

        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        self.assertIn("[registry] Big App", out)

    def test_docker_wsl_requested_with_winreg_unavailable_still_runs(self):
        # Part 4 Phase 3: "docker-wsl" now dispatches internally to
        # docker_native.py on Linux instead of hard-requiring winreg, so it
        # must no longer be pre-filtered by WINREG_AVAILABLE alone.
        with (
            patch.dict(SCANNERS, {"docker-wsl": _fake_docker_wsl_items}, clear=True),
            patch("scripts.system_inventory.inventory.WINREG_AVAILABLE", False),
        ):
            code, out, err = _run([])

        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        self.assertIn("[docker-wsl] Ubuntu", out)


class DockerWslAppdataExcludePathsThreadingTests(unittest.TestCase):
    """Covers the Part 3 double-count-avoidance wiring: with fake docker-wsl
    and appdata scanners installed, the orchestrator must run docker-wsl
    first and thread its items' resolved paths into appdata's
    ``exclude_paths=`` keyword argument — and the grand total must reflect
    that exclusion actually happening (no double count).
    """

    def test_appdata_receives_resolved_docker_wsl_paths_as_exclude_paths(self):
        recorded_kwargs: dict[str, object] = {}

        def fake_appdata(exclude_paths=None):
            recorded_kwargs["exclude_paths"] = exclude_paths
            return [InventoryItem("appdata", "Docker", "C:\\LocalAppData\\Docker", 1000, {"root": "LOCALAPPDATA"})]

        with patch.dict(
            SCANNERS,
            {"docker-wsl": _fake_docker_wsl_items, "appdata": fake_appdata},
            clear=True,
        ):
            code, out, err = _run(["--json"])

        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        expected_paths = {Path(item.path).resolve() for item in _fake_docker_wsl_items()}
        self.assertEqual(recorded_kwargs["exclude_paths"], expected_paths)

    def test_grand_total_does_not_double_count_excluded_docker_wsl_bytes(self):
        # Simulates a real appdata scan: without exclude_paths it would sum
        # the same vhdx bytes a second time (13000); with exclude_paths
        # threaded in, it correctly reports only the non-overlapping bytes
        # (1000).
        def fake_appdata_respecting_exclusions(exclude_paths=None):
            size = 1000 if exclude_paths else 13000
            return [InventoryItem("appdata", "Docker", "C:\\LocalAppData\\Docker", size, {"root": "LOCALAPPDATA"})]

        with patch.dict(
            SCANNERS,
            {"docker-wsl": _fake_docker_wsl_items, "appdata": fake_appdata_respecting_exclusions},
            clear=True,
        ):
            code, out, _err = _run(["--json"])

        self.assertEqual(code, 0)
        payload = json.loads(out)
        docker_wsl_total = sum(item.size_bytes for item in _fake_docker_wsl_items())
        # 5000 + 7000 (docker-wsl) + 1000 (appdata, correctly excluded) =
        # 13000 — NOT 5000 + 7000 + 13000 = 25000, which would be the
        # double-counted total if exclude_paths had not been threaded in.
        self.assertEqual(payload["total_bytes"], docker_wsl_total + 1000)

    def test_appdata_alone_without_docker_wsl_runs_with_no_exclude_paths(self):
        recorded_kwargs: dict[str, object] = {"called": False}

        def fake_appdata(exclude_paths=None):
            recorded_kwargs["called"] = True
            recorded_kwargs["exclude_paths"] = exclude_paths
            return []

        with patch.dict(SCANNERS, {"appdata": fake_appdata}, clear=True):
            code, out, err = _run(["--json"])

        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        self.assertTrue(recorded_kwargs["called"])
        self.assertIsNone(recorded_kwargs["exclude_paths"])

    def test_docker_wsl_alone_without_appdata_runs_normally(self):
        with patch.dict(SCANNERS, {"docker-wsl": _fake_docker_wsl_items}, clear=True):
            code, out, err = _run(["--json"])

        self.assertEqual(code, 0)
        self.assertEqual(err, "")
        payload = json.loads(out)
        self.assertEqual(len(payload["items"]), 2)
        self.assertEqual(payload["total_bytes"], 12000)


if __name__ == "__main__":
    unittest.main()
