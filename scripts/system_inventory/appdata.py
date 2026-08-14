"""AppData, USERPROFILE dotfolder, and ProgramData first-level scanners.

Sizes are computed only for the immediate ("first-level") subfolders of each
root — never for individual files sitting directly under the root, and never
by descending more than one level before handing off to
``common.dir_size_on_disk`` for the recursive, error-tolerant, reparse-point-
skipping summation. This keeps directory *listing* cheap (one
``os.scandir()`` call per root) while still producing an accurate byte
count per folder.

Three independent scanners live here, one per root:

* ``scan_appdata`` — first-level subfolders of ``%LOCALAPPDATA%`` and
  ``%APPDATA%``, tagged ``source="appdata"``.
* ``scan_dotfolders`` — first-level dot-prefixed folders directly under
  ``%USERPROFILE%`` (``.cargo``, ``.npm``, ``.cache``, ...), tagged
  ``source="dotfolder"``.
* ``scan_programdata`` — first-level subfolders of ``%ProgramData%``,
  tagged ``source="programdata"``. The ``chocolatey`` entry is always
  skipped (case-insensitive name match): Chocolatey's app tree is itemized
  per-app by the future ``packages.scan_scoop_choco()`` source, so summing
  it again here would double-count those bytes in the grand total.

Each function accepts an optional ``base`` override so tests can point it at
a temp directory tree instead of the real environment — see each function's
docstring for the exact shape expected. All three tolerate a missing root
(unset environment variable, override key absent, or the resolved path not
existing/not a directory): they skip that root and return whatever the
other root(s) produced, or an empty list if none exist. No exception is
ever raised for a missing or unreadable root.

On Linux, ``%LOCALAPPDATA%``/``%APPDATA%``/``%USERPROFILE%`` do not exist as
environment variables at all, so ``scan_appdata``/``scan_dotfolders`` fall
back to real equivalents instead of silently returning nothing: XDG
``cache``/``data`` home (via ``xdg_dirs.py``) for the former,
``$HOME`` for the latter. ``scan_programdata`` has no such fallback — there
is no single Linux directory that plays the same "machine-wide app data"
role as ``%ProgramData%`` — and simply returns ``[]`` there.

Stdlib only. Never writes to disk.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Callable

from scripts.system_inventory import xdg_dirs
from scripts.system_inventory.common import InventoryItem, dir_size_on_disk

# (environment variable name, detail "root" tag) pairs scanned by scan_appdata.
_APPDATA_ROOTS: tuple[tuple[str, str], ...] = (
    ("LOCALAPPDATA", "LOCALAPPDATA"),
    ("APPDATA", "APPDATA"),
)

# Linux has no LOCALAPPDATA/APPDATA environment variable at all, so a plain
# os.environ.get() always misses there. Real equivalents (not a stub):
# XDG_CACHE_HOME stands in for LOCALAPPDATA (both hold machine-local,
# non-roamed, often-large caches), XDG_DATA_HOME stands in for APPDATA
# (both hold per-app data meant to persist/roam with the user). Only
# consulted when the Windows-style env var is unset AND we are not on
# Windows, so real Windows behavior (including a genuinely unset
# LOCALAPPDATA/APPDATA edge case) is unchanged.
_APPDATA_XDG_FALLBACKS: dict[str, Callable[[], Path | None]] = {
    "LOCALAPPDATA": xdg_dirs.cache_home,
    "APPDATA": xdg_dirs.data_home,
}

# Hardcoded exclusion: chocolatey's app tree is itemized per-app by the
# future packages.scan_scoop_choco() source (Part 2 Phase 2). Summing it
# again as a plain ProgramData subfolder here would double-count those
# bytes in the grand total (see the plan's risk register).
_PROGRAMDATA_EXCLUDED_NAMES = frozenset({"chocolatey"})


def _resolve_mapped_root(base: dict[str, str] | None, env_var: str) -> Path | None:
    """Resolve one root path for ``scan_appdata``.

    When ``base`` is ``None`` (production), reads ``env_var`` straight from
    ``os.environ``. On Windows, an unset variable resolves to ``None``. On
    any other platform, an unset variable instead falls back to the
    matching XDG directory (see ``_APPDATA_XDG_FALLBACKS``) since
    ``LOCALAPPDATA``/``APPDATA`` do not exist there at all. When ``base`` is
    given (tests), it is a plain ``dict`` such as
    ``{"LOCALAPPDATA": "C:/tmp/local"}`` — a key absent from ``base`` is
    treated exactly like an unset environment variable and resolves to
    ``None`` (the XDG fallback is bypassed entirely, by design: it lets
    tests exercise the "one root missing" tolerance deterministically,
    regardless of the OS actually running the test).
    """
    if base is not None:
        value = base.get(env_var)
        return Path(value) if value else None
    value = os.environ.get(env_var)
    if value:
        return Path(value)
    if sys.platform != "win32":
        return _APPDATA_XDG_FALLBACKS[env_var]()
    return None


def _resolve_single_root(base: str | os.PathLike[str] | None, env_var: str) -> Path | None:
    """Resolve the single override root for ``scan_dotfolders``/``scan_programdata``.

    ``base`` given (tests): used verbatim as the root, no environment
    lookup at all. ``base is None`` (production): reads ``env_var`` from
    ``os.environ``; unset resolves to ``None`` — except for
    ``USERPROFILE`` on any non-Windows platform, which is not an
    environment variable that exists there at all: ``$HOME`` is its real
    equivalent (both are "the current user's home directory"), so it is
    used as the fallback instead of leaving ``scan_dotfolders`` silently
    stubbed out on Linux. ``ProgramData`` has no equivalent fallback: there
    is no single Linux directory that plays the same "machine-wide app
    data" role, so it is left unresolved (``scan_programdata`` naturally
    returns ``[]``) rather than guessing at a wrong mapping.
    """
    if base is not None:
        return Path(base)
    value = os.environ.get(env_var)
    if value:
        return Path(value)
    if env_var == "USERPROFILE" and sys.platform != "win32":
        home = os.environ.get("HOME")
        return Path(home) if home else None
    return None


def _iter_first_level_dirs(root: Path) -> list[os.DirEntry]:
    """List immediate directory entries under ``root``, files excluded.

    IO function: talks to ``os.scandir`` directly (unlike the module's
    ``_resolve_*`` helpers, which never touch the filesystem). Tolerant of
    a missing or unreadable root (returns ``[]``) and of individual entries
    whose ``is_dir()`` check raises (skipped, not propagated) — mirrors
    ``common.dir_size_on_disk``'s error tolerance at the listing level.
    """
    try:
        entries = list(os.scandir(root))
    except OSError:
        return []
    dirs: list[os.DirEntry] = []
    for entry in entries:
        try:
            if entry.is_dir():
                dirs.append(entry)
        except OSError:
            continue
    return dirs


def scan_appdata(
    base: dict[str, str] | None = None,
    exclude_paths: set[Path] | None = None,
) -> list[InventoryItem]:
    """Size the first-level subfolders of ``%LOCALAPPDATA%`` and ``%APPDATA%``.

    ``base``, when given, overrides environment resolution for testability:
    a ``dict`` such as ``{"LOCALAPPDATA": "C:/tmp/local", "APPDATA": "C:/tmp/roam"}``.
    A root whose key is absent from ``base`` (or, in production, whose
    environment variable is unset, or whose resolved path does not exist as
    a directory) is skipped cleanly: no item is emitted for it and no
    exception is raised, while the other root (if present) is still scanned
    in full — even if both roots are missing, ``[]`` is returned.

    Only immediate subfolders are listed (a plain ``os.scandir`` first
    level); files sitting directly under a root are ignored. Each
    subfolder's on-disk size is summed recursively via
    ``common.dir_size_on_disk`` (reparse-point-skipping, error-tolerant).
    ``exclude_paths`` is forwarded as-is to every ``dir_size_on_disk`` call,
    letting a future source that claims a specific nested path exactly
    (e.g. Part 3's Docker/WSL ``.vhdx``) opt out of being summed twice here.

    Each emitted item's ``detail`` carries which root it came from:
    ``{"root": "LOCALAPPDATA"}`` or ``{"root": "APPDATA"}``.
    """
    items: list[InventoryItem] = []
    for env_var, detail_root in _APPDATA_ROOTS:
        root = _resolve_mapped_root(base, env_var)
        if root is None or not root.is_dir():
            continue
        for entry in _iter_first_level_dirs(root):
            entry_path = Path(entry.path)
            size = dir_size_on_disk(entry_path, exclude_paths=exclude_paths)
            items.append(
                InventoryItem(
                    source="appdata",
                    name=entry.name,
                    path=str(entry_path),
                    size_bytes=size,
                    detail={"root": detail_root},
                )
            )
    return items


def scan_dotfolders(base: str | os.PathLike[str] | None = None) -> list[InventoryItem]:
    """Size dot-prefixed first-level folders directly under ``%USERPROFILE%``.

    ``base``, when given, overrides environment resolution for testability:
    used verbatim as the USERPROFILE root (no ``os.environ`` lookup at all).
    In production (``base=None``), ``%USERPROFILE%`` is resolved from
    ``os.environ``; an unset variable, or a resolved path that is not an
    existing directory, is tolerated and yields ``[]``.

    Only first-level entries whose name starts with ``"."`` AND whose
    ``is_dir()`` is true are considered — non-dot entries are ignored, and
    so are dotfiles that are plain files (e.g. ``.gitconfig``), since only
    folders are sized. Each matching folder's on-disk size is summed via
    ``common.dir_size_on_disk``. Emitted items carry an empty ``detail``
    dict (there is only one root, so no disambiguating tag is needed).
    """
    root = _resolve_single_root(base, "USERPROFILE")
    if root is None or not root.is_dir():
        return []

    items: list[InventoryItem] = []
    for entry in _iter_first_level_dirs(root):
        if not entry.name.startswith("."):
            continue
        entry_path = Path(entry.path)
        size = dir_size_on_disk(entry_path)
        items.append(
            InventoryItem(
                source="dotfolder",
                name=entry.name,
                path=str(entry_path),
                size_bytes=size,
                detail={},
            )
        )
    return items


def scan_programdata(
    base: str | os.PathLike[str] | None = None,
    exclude_paths: set[Path] | None = None,
) -> list[InventoryItem]:
    """Size the first-level subfolders of ``%ProgramData%``, minus ``chocolatey``.

    ``base``, when given, overrides environment resolution for testability:
    used verbatim as the ProgramData root (no ``os.environ`` lookup at all).
    In production (``base=None``), ``%ProgramData%`` is resolved from
    ``os.environ``; an unset variable, or a resolved path that is not an
    existing directory, is tolerated and yields ``[]``.

    The entry named ``chocolatey`` (case-insensitive) is always excluded:
    Chocolatey's app tree is itemized per-app by the future
    ``packages.scan_scoop_choco()`` source, so summing it again here would
    double-count those bytes in the grand total. Every other first-level
    subfolder is sized via ``common.dir_size_on_disk``, with
    ``exclude_paths`` forwarded as-is (same double-count-avoidance
    mechanism as ``scan_appdata``).
    """
    root = _resolve_single_root(base, "ProgramData")
    if root is None or not root.is_dir():
        return []

    items: list[InventoryItem] = []
    for entry in _iter_first_level_dirs(root):
        if entry.name.lower() in _PROGRAMDATA_EXCLUDED_NAMES:
            continue
        entry_path = Path(entry.path)
        size = dir_size_on_disk(entry_path, exclude_paths=exclude_paths)
        items.append(
            InventoryItem(
                source="programdata",
                name=entry.name,
                path=str(entry_path),
                size_bytes=size,
                detail={},
            )
        )
    return items
