"""Caches d'outils de développement propres à Linux.

Ces caches sont sûrs à supprimer parce qu'ils ne contiennent que des artefacts
téléchargés ou générés. Ils restent séparés du balayage générique de
``~/.cache`` afin de déclarer correctement leur besoin réseau et de les rendre
visibles dès le niveau ``safe``.
"""

from __future__ import annotations

import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import platform_paths  # noqa: E402
from scripts.winclean.common import CleanCandidate, Level, estimate_path  # noqa: E402

__all__ = ["PLAYWRIGHT_CACHE_NAMES", "discover_playwright_browsers"]


# Liste fermée : ne jamais absorber un autre répertoire simplement parce que
# son nom contient ``playwright``.
PLAYWRIGHT_CACHE_NAMES: tuple[str, ...] = (
    "ms-playwright",
    "ms-playwright-go",
    "ms-playwright-mcp",
)


def discover_playwright_browsers(
    env: dict[str, str] | None = None,
    **_kw: object,
) -> list[CleanCandidate]:
    """Binaires de navigateurs Playwright sous le cache XDG."""
    cache_root = platform_paths.cache_home(env)
    if cache_root is None:
        return []

    found: list[CleanCandidate] = []
    for name in PLAYWRIGHT_CACHE_NAMES:
        path = cache_root / name
        if not path.is_dir():
            continue
        try:
            mtime: float | None = path.stat().st_mtime
        except OSError:
            mtime = None
        found.append(
            CleanCandidate(
                module="playwright-browsers-linux",
                path=str(path),
                label=f"navigateurs Playwright ({name})",
                estimated_bytes=estimate_path(path),
                level=Level.SAFE,
                reason="retéléchargés par l'outil Playwright qui les utilise",
                stat_mtime=mtime,
            )
        )
    return found
