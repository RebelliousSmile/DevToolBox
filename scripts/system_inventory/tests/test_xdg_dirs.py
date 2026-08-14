"""Unit tests for scripts/system_inventory/xdg_dirs.py.

Import note: same fully-qualified ``scripts.system_inventory.*`` spelling as
``test_common.py`` — see that module's docstring for why this is the only
import form that resolves under both
``python -m unittest discover -s scripts/system_inventory/tests -v`` and
``python -m unittest scripts.system_inventory.tests.test_xdg_dirs``.
"""

from __future__ import annotations

import unittest
from pathlib import Path

from scripts.system_inventory.xdg_dirs import (
    cache_home,
    config_home,
    data_home,
    state_home,
)


class XdgHomeResolutionTests(unittest.TestCase):
    def test_override_used_verbatim_when_present(self):
        env = {"XDG_CACHE_HOME": "/custom/cache", "HOME": "/home/someone"}
        self.assertEqual(cache_home(env), Path("/custom/cache"))

    def test_falls_back_to_home_relative_default_when_override_absent(self):
        env = {"HOME": "/home/someone"}
        self.assertEqual(config_home(env), Path("/home/someone/.config"))
        self.assertEqual(data_home(env), Path("/home/someone/.local/share"))
        self.assertEqual(cache_home(env), Path("/home/someone/.cache"))
        self.assertEqual(state_home(env), Path("/home/someone/.local/state"))

    def test_blank_override_is_treated_as_absent(self):
        env = {"XDG_DATA_HOME": "", "HOME": "/home/someone"}
        self.assertEqual(data_home(env), Path("/home/someone/.local/share"))

    def test_both_override_and_home_absent_returns_none(self):
        self.assertIsNone(cache_home({}))

    def test_home_absent_but_override_present_uses_override(self):
        env = {"XDG_CONFIG_HOME": "/custom/config"}
        self.assertEqual(config_home(env), Path("/custom/config"))


if __name__ == "__main__":
    unittest.main()
