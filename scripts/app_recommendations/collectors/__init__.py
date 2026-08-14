"""Platform collector registry."""

from __future__ import annotations

import sys

from scripts.app_recommendations.report import Collector


def available_collectors() -> dict[str, Collector]:
    if sys.platform.startswith("linux"):
        from .linux import linux_collectors

        return linux_collectors()
    if sys.platform == "win32":
        try:
            from .windows import windows_collectors
        except ImportError:
            return {}
        return windows_collectors()
    return {}
