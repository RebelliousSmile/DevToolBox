"""Scoop and Chocolatey package-manager app-tree scanner.

Sizes are computed only for the immediate ("first-level") entries under
each package manager's app-listing folder — one entry per installed
app/package — never by descending more than one level before handing off
to ``common.dir_size_on_disk`` for the recursive, error-tolerant,
reparse-point-skipping summation. This mirrors the split used by
``appdata.py``: a pure ``_resolve_manager_root`` (no IO, just path
resolution) versus an IO-performing ``_iter_first_level_dirs``.

One scanner lives here:

* ``scan_scoop_choco`` — sizes ``%USERPROFILE%\\scoop\\apps\\*`` (Scoop) and
  ``C:\\ProgramData\\chocolatey\\lib\\*`` (Chocolatey), tagged
  ``source="scoop-choco"``, ``detail={"manager": "scoop"|"chocolatey"}``.

Both package managers are optional: a machine may have neither, either, or
both installed. The scanner is present-only — a missing manager root, or a
manager root that exists but has no ``apps``/``lib`` subfolder yet (e.g.
Scoop installed with zero apps), is tolerated and contributes no items for
that manager. No exception is ever raised; absence of both managers yields
``[]``.

On Linux (production call, ``base is None``), this dispatches instead to
``packages_linux.scan_packages_linux()`` — apt/dnf/pacman are the Linux
equivalent of Scoop/Chocolatey for this same ``"scoop-choco"`` source slot.
Scoop/Chocolatey are inherently Windows-only tools, so there is no Linux
path resolution to perform here.

Stdlib only. Never writes to disk.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

from scripts.system_inventory import packages_linux
from scripts.system_inventory.common import InventoryItem, dir_size_on_disk

# Chocolatey does not expose its install root through an environment
# variable that reliably matches a machine-wide install (it is not derived
# from %ProgramData%), so the production root is fixed rather than resolved
# dynamically. Tests still override it via the `base` dict.
_CHOCOLATEY_HARDCODED_ROOT = r"C:\ProgramData\chocolatey"

# (manager name, app-listing subfolder name) pairs scanned by
# scan_scoop_choco. Scoop lists installed apps under "apps/"; Chocolatey
# lists installed packages under "lib/".
_MANAGER_SUBDIRS: tuple[tuple[str, str], ...] = (
    ("scoop", "apps"),
    ("chocolatey", "lib"),
)


def _resolve_manager_root(base: dict[str, str] | None, manager: str) -> Path | None:
    """Resolve one package manager's root path.

    ``base`` given (tests): a plain ``dict`` such as
    ``{"scoop": "C:/tmp/scoop", "chocolatey": "C:/tmp/choco"}`` — a key
    absent from ``base`` resolves to ``None``, exactly like that manager
    not being installed. ``base is None`` (production): Scoop's root is
    ``%USERPROFILE%\\scoop`` (``None`` if ``USERPROFILE`` is unset);
    Chocolatey's root is the hardcoded ``_CHOCOLATEY_HARDCODED_ROOT``.
    """
    if base is not None:
        value = base.get(manager)
        return Path(value) if value else None
    if manager == "scoop":
        userprofile = os.environ.get("USERPROFILE")
        return Path(userprofile) / "scoop" if userprofile else None
    if manager == "chocolatey":
        return Path(_CHOCOLATEY_HARDCODED_ROOT)
    return None  # pragma: no cover - defensive, _MANAGER_SUBDIRS is closed


def _iter_first_level_dirs(root: Path) -> list[os.DirEntry]:
    """List immediate directory entries under ``root``, files excluded.

    IO function: talks to ``os.scandir`` directly (unlike
    ``_resolve_manager_root``, which never touches the filesystem).
    Tolerant of a missing or unreadable root (returns ``[]``) and of
    individual entries whose ``is_dir()`` check raises (skipped, not
    propagated) — mirrors ``appdata.py``'s helper of the same name.
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


def scan_scoop_choco(base: dict[str, str] | None = None) -> list[InventoryItem]:
    """Size Scoop's ``apps/*`` and Chocolatey's ``lib/*`` app trees, if present.

    ``base``, when given, overrides root resolution for testability: a
    ``dict`` such as ``{"scoop": "C:/tmp/scoop", "chocolatey": "C:/tmp/choco"}``.
    A manager whose key is absent from ``base`` (or, in production, whose
    root does not exist as a directory) is skipped cleanly: no item is
    emitted for it and no exception is raised, while the other manager (if
    present) is still scanned in full. A manager root that exists but whose
    ``apps``/``lib`` subfolder does not (e.g. Scoop installed with no apps
    yet) is likewise tolerated and contributes no items. If neither manager
    is present, ``[]`` is returned.

    Only immediate entries under ``apps``/``lib`` are listed (a plain
    ``os.scandir`` first level) — each one is exactly one installed
    app/package. Each entry's on-disk size is summed recursively via
    ``common.dir_size_on_disk`` (reparse-point-skipping, error-tolerant);
    no ``exclude_paths`` is threaded through here since this source does not
    need to defer to any narrower source.

    Each emitted item's ``detail`` carries which manager it came from:
    ``{"manager": "scoop"}`` or ``{"manager": "chocolatey"}``.

    On Linux, when ``base`` is not overridden (i.e. this is a real
    production call, not a test), dispatches to
    ``packages_linux.scan_packages_linux()`` instead — Scoop/Chocolatey are
    Windows-only tools with no Linux equivalent path to resolve.
    """
    if base is None and sys.platform != "win32":
        return packages_linux.scan_packages_linux()
    items: list[InventoryItem] = []
    for manager, subdir_name in _MANAGER_SUBDIRS:
        root = _resolve_manager_root(base, manager)
        if root is None or not root.is_dir():
            continue
        subdir = root / subdir_name
        if not subdir.is_dir():
            continue
        for entry in _iter_first_level_dirs(subdir):
            entry_path = Path(entry.path)
            size = dir_size_on_disk(entry_path)
            items.append(
                InventoryItem(
                    source="scoop-choco",
                    name=entry.name,
                    path=str(entry_path),
                    size_bytes=size,
                    detail={"manager": manager},
                )
            )
    return items
