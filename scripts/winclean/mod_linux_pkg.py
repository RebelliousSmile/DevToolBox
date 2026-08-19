"""Modules `safe` de caches de gestionnaires de paquets, sous Linux.

Contrepartie de `mod_dev.py` (caches de gestionnaires de paquets Windows) :
même discipline - ce fichier n'importe jamais `remove`, ne rend que des
`CleanCandidate`, et ne décide jamais `needs_network` lui-même (estampillé une
seule fois par `registry_mod.discover_module()` depuis la déclaration du
module). Un test de source le vérifie
(`tests/test_registry_mod_linux_contract.py`).

Chaque cache est résolu dans l'ordre outil puis repli XDG, comme
`mod_dev.resolve_cache_path()` côté Windows - sauf que le repli lit
`platform_paths.py` (donc `$XDG_*`/`$HOME`) plutôt qu'une variable Windows
(`%LOCALAPPDATA%`). `apt-cache` n'a pas d'équivalent : c'est un chemin système
fixe, jamais dérivé de l'utilisateur courant, donc résolu à part.
"""

from __future__ import annotations

import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from shutil import which
from typing import Callable

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import platform_paths  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    CleanCandidate,
    Level,
    estimate_path,
)

__all__ = [
    "CACHE_SPECS",
    "CacheSpec",
    "resolve_cache_path",
    "discover_cache",
    "discover_apt_archives",
    "have",
]

_TOOL_TIMEOUT_SECONDS = 10


def have(binary: str) -> bool:
    """Vrai si le binaire est sur le `PATH`. Aucun bruit s'il manque."""
    return which(binary) is not None


def _candidate(module: str, path: Path, label: str, reason: str) -> CleanCandidate:
    try:
        mtime: float | None = path.stat().st_mtime
    except OSError:
        mtime = None
    return CleanCandidate(
        module=module,
        path=str(path),
        label=label,
        estimated_bytes=estimate_path(path),
        level=Level.SAFE,
        reason=reason,
        stat_mtime=mtime,
    )


@dataclass(frozen=True)
class CacheSpec:
    """Où trouver le cache d'un outil, dans l'ordre de préférence."""

    module: str
    label: str
    reason: str
    #: Commande de l'outil qui rend son propre chemin de cache, si elle existe.
    tool: tuple[str, ...] | None
    #: Repli documenté : base XDG (`platform_paths.cache_home`/`data_home`) et le
    #: sous-chemin à lui joindre.
    fallback_base: Callable[[dict[str, str] | None], Path | None]
    fallback_suffix: str


CACHE_SPECS: dict[str, CacheSpec] = {
    "npm-cache-linux": CacheSpec(
        module="npm-cache-linux",
        label="cache npm",
        reason="re-téléchargé par `npm install`",
        tool=("npm", "config", "get", "cache"),
        fallback_base=platform_paths.home,
        fallback_suffix=".npm",
    ),
    "pip-cache-linux": CacheSpec(
        module="pip-cache-linux",
        label="cache pip",
        reason="re-téléchargé par `pip install`",
        tool=("pip", "cache", "dir"),
        fallback_base=platform_paths.cache_home,
        fallback_suffix="pip",
    ),
    "pnpm-store-linux": CacheSpec(
        module="pnpm-store-linux",
        label="store pnpm",
        reason="re-téléchargé par `pnpm install`",
        tool=("pnpm", "store", "path"),
        fallback_base=platform_paths.data_home,
        fallback_suffix="pnpm/store",
    ),
}


def _ask_tool(command: tuple[str, ...]) -> Path | None:
    """Chemin rendu par l'outil lui-même, s'il est là et s'il répond."""
    if not have(command[0]):
        return None
    try:
        completed = subprocess.run(
            list(command),
            capture_output=True,
            text=True,
            timeout=_TOOL_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode != 0:
        return None
    answer = (completed.stdout or "").strip().splitlines()
    if not answer:
        return None
    candidate = Path(answer[-1].strip().strip('"'))
    return candidate if candidate.is_absolute() else None


def resolve_cache_path(spec: CacheSpec, env: dict[str, str] | None = None) -> Path | None:
    """Chemin du cache : l'outil d'abord, puis le repli XDG.

    L'outil est préféré parce que son chemin est le vrai : un store pnpm peut
    vivre ailleurs que sous `$XDG_DATA_HOME` (`$PNPM_HOME`, config utilisateur),
    et un repli figé désignerait un dossier qui n'a jamais rien contenu.
    """
    if spec.tool is not None:
        from_tool = _ask_tool(spec.tool)
        if from_tool is not None:
            return from_tool
    base = spec.fallback_base(env)
    if base is None:
        return None
    return base / spec.fallback_suffix


def discover_cache(
    module: str, env: dict[str, str] | None = None, **_kw: object
) -> list[CleanCandidate]:
    """Le cache d'un outil, en un candidat, ou rien s'il n'existe pas."""
    spec = CACHE_SPECS[module]
    path = resolve_cache_path(spec, env)
    if path is None or not path.is_dir():
        return []
    return [_candidate(module=module, path=path, label=spec.label, reason=spec.reason)]


def discover_apt_archives(**_kw: object) -> list[CleanCandidate]:
    """Paquets `.deb` téléchargés sous Debian/Ubuntu : chemin système fixe.

    Distinct de `discover_cache` : `apt` n'a pas de commande qui rend son
    propre répertoire de cache, et le chemin n'est jamais dérivé de
    l'utilisateur courant - lire ce dossier exige typiquement les droits root,
    donc un `estimate_path` refusé (`None`) est attendu sur une machine sans
    élévation, pas une anomalie.
    """
    path = platform_paths.APT_ARCHIVES_DIR
    if not path.is_dir():
        return []
    return [
        _candidate(
            module="apt-cache",
            path=path,
            label="cache apt (.deb téléchargés)",
            reason="re-téléchargé par `apt install`",
        )
    ]
