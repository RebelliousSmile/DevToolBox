"""Modules `safe` d'outillage de développement. Découverte seulement.

Ce fichier n'importe jamais `remove` : il rend des `CleanCandidate` et rien
d'autre ne lui est permis. Un test le vérifie sur la source.

Aucun `discover_*()` ne décide `needs_network` : la valeur est déclarée une seule
fois sur le module dans `registry_mod.py` et estampillée sur chaque candidat par
l'appelant de découverte. La décider ici laisserait le registre annoncer `True`
pendant que la fonction émet `False`, avec le test de correspondance au vert.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Iterator, Sequence

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from scripts.winclean import procs  # noqa: E402
from scripts.winclean.common import (  # noqa: E402
    CleanCandidate,
    Level,
    estimate_path,
)

__all__ = [
    "CARGO_OWNERS",
    "DOTNET_OWNERS",
    "CACHE_SPECS",
    "CacheSpec",
    "resolve_cache_path",
    "discover_cargo_target",
    "discover_pycache",
    "discover_dotnet_binobj",
    "discover_cache",
    "have",
]

#: Répertoires jamais traversés : rien de nettoyable dedans, et la traversée
#: coûte le plus clair du temps d'un run.
PRUNE_ALWAYS: frozenset[str] = frozenset(
    {".git", ".hg", ".svn", "$RECYCLE.BIN", "System Volume Information"}
)

FILE_ATTRIBUTE_REPARSE_POINT = 0x400

#: Propriétaires de processus par famille d'outils. Déclarés ici, à côté de la
#: découverte qui les surveille ; `registry_mod` les rattache aux modules et
#: Phase 4 lit la même table pour décider l'omission.
CARGO_OWNERS: tuple[str, ...] = ("cargo.exe", "rustc.exe")
DOTNET_OWNERS: tuple[str, ...] = ("msbuild.exe", "dotnet.exe")

_TOOL_TIMEOUT_SECONDS = 10
_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0) if sys.platform == "win32" else 0


# --------------------------------------------------------------------------- #
# Outils communs
# --------------------------------------------------------------------------- #


def have(binary: str) -> bool:
    """Vrai si le binaire est sur le `PATH`. Aucun bruit s'il manque."""
    return shutil.which(binary) is not None


def _is_reparse(entry: os.DirEntry[str]) -> bool:
    try:
        attributes = getattr(entry.stat(follow_symlinks=False), "st_file_attributes", 0)
        return bool(attributes & FILE_ATTRIBUTE_REPARSE_POINT)
    except OSError:
        return False


def _walk(
    root: Path,
    max_depth: int,
    prune: Iterable[str] = (),
) -> Iterator[tuple[Path, list[os.DirEntry[str]]]]:
    """Parcours borné en profondeur. Rend chaque répertoire et ses entrées.

    Un répertoire situé à la profondeur `max_depth` n'est pas ouvert : ses
    entrées seraient à `max_depth + 1`. La racine est à la profondeur 0, donc
    ses enfants sont à 1 - ce qui rend `--max-depth 6` équivalent à « six
    composants sous la racine », la lecture qu'un utilisateur en fait.
    """
    pruned = PRUNE_ALWAYS | {name.lower() for name in prune}
    stack: list[tuple[Path, int]] = [(root, 0)]
    while stack:
        current, depth = stack.pop()
        if depth >= max_depth:
            continue
        try:
            with os.scandir(current) as iterator:
                entries = list(iterator)
        except OSError:
            continue  # dossier illisible : ignoré sans bruit
        yield current, entries
        for entry in entries:
            try:
                if not entry.is_dir(follow_symlinks=False):
                    continue
            except OSError:
                continue
            if entry.name.lower() in pruned or _is_reparse(entry):
                continue
            stack.append((Path(entry.path), depth + 1))


def _owner_reason(owners: Sequence[str]) -> str | None:
    """Avertissement à joindre à la raison, ou `None` si rien ne tourne."""
    return procs.owner_reason(procs.is_running(owners), owners)


def _candidate(
    module: str,
    path: Path,
    label: str,
    reason: str,
    warning: str | None = None,
) -> CleanCandidate:
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
        reason=reason if warning is None else f"{reason} - {warning}",
        stat_mtime=mtime,
    )


def _roots(roots: Iterable[str | os.PathLike[str]] | None) -> list[Path]:
    return [Path(r) for r in (roots or ())]


# --------------------------------------------------------------------------- #
# Modules `walking`
# --------------------------------------------------------------------------- #


def _qualifies_as_cargo_target(target: Path) -> bool:
    """Un `target/` ne compte que s'il porte une marque de cargo.

    Sans cette marque, un dossier nommé `target` peut être n'importe quoi -
    un dossier de données à côté d'un `Cargo.toml` suffirait à le faire
    supprimer.
    """
    if (target / "CACHEDIR.TAG").exists():
        return True
    return (target / "debug").is_dir() or (target / "release").is_dir()


def discover_cargo_target(
    roots: Iterable[str | os.PathLike[str]] | None = None,
    max_depth: int = 6,
    **_kw: object,
) -> list[CleanCandidate]:
    """`target/` voisin d'un `Cargo.toml`, sous les racines et pas plus profond."""
    warning = _owner_reason(CARGO_OWNERS)
    out: list[CleanCandidate] = []
    seen: set[str] = set()
    for root in _roots(roots):
        for directory, entries in _walk(root, max_depth, prune=("target",)):
            if not any(e.name == "Cargo.toml" and not e.is_dir() for e in entries):
                continue
            target = directory / "target"
            if not target.is_dir() or not _qualifies_as_cargo_target(target):
                continue
            key = os.path.normcase(str(target))
            if key in seen:
                continue
            seen.add(key)
            out.append(
                _candidate(
                    module="cargo-target",
                    path=target,
                    label=f"target de {directory.name}",
                    reason="reconstruit par `cargo build`",
                    warning=warning,
                )
            )
    return out


def discover_pycache(
    roots: Iterable[str | os.PathLike[str]] | None = None,
    max_depth: int = 6,
    **_kw: object,
) -> list[CleanCandidate]:
    """Répertoires `__pycache__`. Aucun garde de processus : `proc_guard = None`.

    Cette fonction n'interroge jamais `procs.is_running` - un module qui ne
    déclare aucune force de garde ne doit pas en exercer une.
    """
    out: list[CleanCandidate] = []
    seen: set[str] = set()
    for root in _roots(roots):
        for directory, entries in _walk(root, max_depth, prune=("__pycache__",)):
            for entry in entries:
                if entry.name != "__pycache__":
                    continue
                try:
                    if not entry.is_dir(follow_symlinks=False):
                        continue
                except OSError:
                    continue
                path = Path(entry.path)
                key = os.path.normcase(str(path))
                if key in seen:
                    continue
                seen.add(key)
                out.append(
                    _candidate(
                        module="pycache",
                        path=path,
                        label=f"__pycache__ de {directory.name}",
                        reason="recréé à l'import par Python",
                    )
                )
    return out


def discover_dotnet_binobj(
    roots: Iterable[str | os.PathLike[str]] | None = None,
    max_depth: int = 6,
    **_kw: object,
) -> list[CleanCandidate]:
    """`bin/` et `obj/` à côté d'un `*.csproj`."""
    warning = _owner_reason(DOTNET_OWNERS)
    out: list[CleanCandidate] = []
    seen: set[str] = set()
    for root in _roots(roots):
        for directory, entries in _walk(root, max_depth, prune=("bin", "obj")):
            if not any(e.name.lower().endswith(".csproj") and not e.is_dir() for e in entries):
                continue
            for name in ("bin", "obj"):
                path = directory / name
                if not path.is_dir():
                    continue
                key = os.path.normcase(str(path))
                if key in seen:
                    continue
                seen.add(key)
                out.append(
                    _candidate(
                        module="dotnet-binobj",
                        path=path,
                        label=f"{name} de {directory.name}",
                        reason="reconstruit par `dotnet build`",
                        warning=warning,
                    )
                )
    return out


# --------------------------------------------------------------------------- #
# Modules `fixed` : caches de gestionnaires de paquets
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class CacheSpec:
    """Où trouver le cache d'un outil, dans l'ordre de préférence."""

    module: str
    label: str
    reason: str
    #: Commande de l'outil qui rend son propre chemin de cache, si elle existe.
    tool: tuple[str, ...] | None
    #: Variables d'environnement portant une base, avec le suffixe à y joindre.
    env: tuple[tuple[str, str], ...]
    #: Repli documenté : `(base, chemin relatif)`, base parmi LOCALAPPDATA /
    #: USERPROFILE / APPDATA.
    fallback: tuple[str, str]


CACHE_SPECS: dict[str, CacheSpec] = {
    "cargo-registry": CacheSpec(
        module="cargo-registry",
        label="registre cargo",
        reason="re-téléchargé par `cargo fetch`",
        tool=None,
        env=(("CARGO_HOME", "registry"),),
        fallback=("USERPROFILE", ".cargo\\registry"),
    ),
    "npm-cache": CacheSpec(
        module="npm-cache",
        label="cache npm",
        reason="re-téléchargé par `npm install`",
        tool=("npm", "config", "get", "cache"),
        env=(),
        fallback=("LOCALAPPDATA", "npm-cache"),
    ),
    "pnpm-store": CacheSpec(
        module="pnpm-store",
        label="store pnpm",
        reason="re-téléchargé par `pnpm install`",
        tool=("pnpm", "store", "path"),
        env=(),
        fallback=("LOCALAPPDATA", "pnpm\\store"),
    ),
    "yarn-cache": CacheSpec(
        module="yarn-cache",
        label="cache yarn",
        reason="re-téléchargé par `yarn install`",
        tool=("yarn", "cache", "dir"),
        env=(),
        fallback=("LOCALAPPDATA", "Yarn\\Cache"),
    ),
    "bun-cache": CacheSpec(
        module="bun-cache",
        label="cache bun",
        reason="re-téléchargé par `bun install`",
        tool=None,
        env=(("BUN_INSTALL", "install\\cache"),),
        fallback=("USERPROFILE", ".bun\\install\\cache"),
    ),
    "pip-cache": CacheSpec(
        module="pip-cache",
        label="cache pip",
        reason="re-téléchargé par `pip install`",
        tool=("pip", "cache", "dir"),
        env=(),
        fallback=("LOCALAPPDATA", "pip\\cache"),
    ),
    "uv-cache": CacheSpec(
        module="uv-cache",
        label="cache uv",
        reason="re-téléchargé par `uv sync`",
        tool=("uv", "cache", "dir"),
        env=(("UV_CACHE_DIR", ""),),
        fallback=("LOCALAPPDATA", "uv\\cache"),
    ),
    "nuget-packages": CacheSpec(
        module="nuget-packages",
        label="paquets NuGet globaux",
        reason="re-téléchargé par `dotnet restore`",
        tool=None,
        env=(("NUGET_PACKAGES", ""),),
        fallback=("USERPROFILE", ".nuget\\packages"),
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
            creationflags=_NO_WINDOW,
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
    """Chemin du cache : l'outil d'abord, puis l'environnement, puis le repli.

    L'outil est préféré parce que son chemin est le vrai : un store pnpm vit sur
    le volume du projet, et un repli `%LOCALAPPDATA%` désignerait un dossier qui
    n'a jamais rien contenu.
    """
    environment = os.environ if env is None else env
    if spec.tool is not None:
        from_tool = _ask_tool(spec.tool)
        if from_tool is not None:
            return from_tool
    for key, suffix in spec.env:
        base = environment.get(key)
        if base:
            return Path(base) / suffix if suffix else Path(base)
    base_key, relative = spec.fallback
    base = environment.get(base_key)
    if not base:
        return None
    return Path(base) / relative


def discover_cache(module: str, **_kw: object) -> list[CleanCandidate]:
    """Le cache d'un outil, en un candidat, ou rien s'il n'existe pas."""
    spec = CACHE_SPECS[module]
    path = resolve_cache_path(spec)
    if path is None or not path.is_dir():
        return []
    return [
        _candidate(
            module=module,
            path=path,
            label=spec.label,
            reason=spec.reason,
        )
    ]
