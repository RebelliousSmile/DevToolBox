"""Tests des caches d'outils de développement Linux."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from scripts.winclean import mod_linux_dev
from scripts.winclean.common import Level


class PlaywrightBrowsersTest(unittest.TestCase):
    def test_only_allowlisted_existing_directories_are_candidates(self) -> None:
        with tempfile.TemporaryDirectory() as raw_home:
            home = Path(raw_home)
            cache = home / ".cache"
            for name in mod_linux_dev.PLAYWRIGHT_CACHE_NAMES:
                path = cache / name
                path.mkdir(parents=True)
                (path / "browser.bin").write_bytes(b"x" * 10)
            (cache / "my-playwright-data").mkdir()

            found = mod_linux_dev.discover_playwright_browsers(
                env={"HOME": str(home)}
            )

        self.assertEqual(
            {Path(candidate.path or "").name for candidate in found},
            set(mod_linux_dev.PLAYWRIGHT_CACHE_NAMES),
        )
        self.assertTrue(all(candidate.level is Level.SAFE for candidate in found))
        self.assertTrue(all(candidate.estimated_bytes == 10 for candidate in found))
        self.assertTrue(all(not candidate.needs_network for candidate in found))

    def test_absent_cache_home_yields_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as raw_home:
            found = mod_linux_dev.discover_playwright_browsers(
                env={"HOME": raw_home}
            )
        self.assertEqual(found, [])


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
