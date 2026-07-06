"""Docker/WSL2 source: ``.vhdx`` virtual disks + registered WSL distros.

Two independent sub-scans, merged into a single list tagged
``source="docker-wsl"``:

* Glob every ``*.vhdx`` directly under ``%LOCALAPPDATA%\\Docker\\wsl\\``, and
  under its ``data``/``disk`` subfolders — this catches Docker Desktop's own
  WSL2 utility-VM disks (``ext4.vhdx``, ``data.vhdx``, ...) wherever it
  happens to place them. Each is sized via ``os.stat().st_size``,
  ``detail={"kind": "vhdx"}``.
* Enumerate ``HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Lxss\\*``:
  each subkey describes one registered WSL distro (``DistributionName`` +
  ``BasePath``). When ``BasePath\\ext4.vhdx`` exists, it is sized and
  emitted with ``detail={"distro": <name>, "kind": "wsl-distro"}``. This
  covers both Docker-internal distros (``BasePath`` under
  ``Docker\\wsl\\``) and Microsoft-Store-installed distros such as Ubuntu or
  Debian (``BasePath`` under ``Packages\\<PackageFamilyName>\\LocalState\\``).

A distro's vhdx can be the exact same file the glob already found (this
genuinely happens when a distro's ``BasePath`` sits under
``Docker\\wsl\\``): when both sub-scans resolve to the same absolute path,
only the distro-tagged item is kept (it carries the distro name, which is
strictly more information than the bare ``kind: vhdx`` glob item).

Every ``InventoryItem`` this module emits has a non-``None`` ``path`` set to
the vhdx's absolute filesystem path — the orchestrator (``inventory.py``)
relies on collecting these into an ``exclude_paths`` set passed to
``appdata.scan_appdata()``, so the same bytes are never summed twice under a
generic ``LOCALAPPDATA`` first-level folder (``Docker`` or ``Packages``).

Present-only: a missing Docker/WSL root and a missing/empty ``Lxss`` key are
both tolerated independently — if neither contributes anything, ``[]`` is
returned without raising. ``winreg`` unavailability (non-Windows) is *not*
tolerated the same way: it raises ``WinregUnavailableError`` (imported from
``registry.py`` and reused verbatim), exactly like ``registry.py`` and
``path_env.py``, since this source fundamentally requires the registry for
its distro half.

Read-only contract: only ``winreg.OpenKey``/``winreg.EnumKey``/
``winreg.QueryValueEx`` are used. No write API is imported or referenced —
enforced by ``tests/test_docker_wsl.py`` via the same source-text scan
pattern used by ``tests/test_registry.py``.

Stdlib only. Never writes to disk or the registry.
"""

from __future__ import annotations

import os
from pathlib import Path

from scripts.system_inventory.common import InventoryItem
from scripts.system_inventory.registry import WinregUnavailableError

try:
    import winreg

    WINREG_AVAILABLE = True
except ImportError:  # pragma: no cover - exercised only on non-Windows.
    winreg = None  # type: ignore[assignment]
    WINREG_AVAILABLE = False

LXSS_SUBKEY = r"Software\Microsoft\Windows\CurrentVersion\Lxss"

_VHDX_GLOB_SUBDIRS = ("", "data", "disk")

_VHDX_FILENAME = "ext4.vhdx"


def _resolve_docker_wsl_root(base: dict[str, str] | None) -> Path | None:
    """Resolve the Docker/WSL vhdx root: ``%LOCALAPPDATA%\\Docker\\wsl`` by default.

    ``base``, when given, overrides resolution for testability: a ``dict``
    with an optional ``"docker_wsl_root"`` key, used verbatim as the root
    (no ``LOCALAPPDATA`` lookup at all) — analogous to ``appdata.py``'s
    ``base`` dict pattern. A ``base`` given without that key resolves to
    ``None``, exactly like the root not existing. ``base is None``
    (production): the root is derived from ``%LOCALAPPDATA%``; an unset
    environment variable resolves to ``None``.
    """
    if base is not None:
        value = base.get("docker_wsl_root")
        return Path(value) if value else None
    localappdata = os.environ.get("LOCALAPPDATA")
    if not localappdata:
        return None
    return Path(localappdata) / "Docker" / "wsl"


def _glob_vhdx_files(root: Path) -> list[Path]:
    """List every ``*.vhdx`` directly under ``root``, ``root/data``, and ``root/disk``.

    Tolerant of a missing root or missing subfolder: each of the three
    locations independently contributes zero files when absent, and no
    exception is ever raised. Covers ``ext4.vhdx`` (or any other ``.vhdx``)
    wherever Docker Desktop happens to place it — directly under ``wsl\\``,
    or one level down under ``wsl\\data\\``/``wsl\\disk\\``. Each location is
    globbed non-recursively (``*.vhdx``, not ``**/*.vhdx``), so there is no
    overlap between the three: a file in ``root/data`` is never also matched
    by the ``root`` glob.
    """
    if not root.is_dir():
        return []
    files: list[Path] = []
    for subdir_name in _VHDX_GLOB_SUBDIRS:
        candidate_dir = root / subdir_name if subdir_name else root
        try:
            if not candidate_dir.is_dir():
                continue
            files.extend(sorted(candidate_dir.glob("*.vhdx")))
        except OSError:
            continue
    return files


def _vhdx_glob_items(base: dict[str, str] | None = None) -> list[InventoryItem]:
    """Build ``InventoryItem``s for every vhdx found by ``_glob_vhdx_files``.

    IO function: resolves the root, globs, and stats each file. A file that
    disappears or becomes unreadable between the glob and the ``stat()``
    call is skipped rather than raising (mirrors the error tolerance used
    throughout ``common.dir_size_on_disk``).
    """
    root = _resolve_docker_wsl_root(base)
    if root is None:
        return []

    items: list[InventoryItem] = []
    for vhdx_path in _glob_vhdx_files(root):
        try:
            size = vhdx_path.stat().st_size
            resolved_path = vhdx_path.resolve()
        except OSError:
            continue
        items.append(
            InventoryItem(
                source="docker-wsl",
                name=vhdx_path.name,
                path=str(resolved_path),
                size_bytes=size,
                detail={"kind": "vhdx"},
            )
        )
    return items


def _parse_lxss_distros(raw_subkeys: dict[str, dict[str, str]]) -> list[dict[str, str]]:
    """Parse enumerated Lxss subkey values into plain distro records.

    ``raw_subkeys`` mirrors what live enumeration collects: a ``dict`` keyed
    by subkey name (an opaque GUID string, never inspected beyond
    iteration), each mapping to a ``dict`` of value-name -> raw value as if
    read via ``winreg.QueryValueEx``. Pure: never touches ``winreg``, so
    tests can call it directly with a fake dict.

    A subkey missing ``DistributionName`` or ``BasePath`` (or with a
    non-``str``/blank value for either) is skipped entirely — both are
    required to locate and label a distro's vhdx.
    """
    records: list[dict[str, str]] = []
    for values in raw_subkeys.values():
        name = values.get("DistributionName")
        base_path = values.get("BasePath")
        if not isinstance(name, str) or not name.strip():
            continue
        if not isinstance(base_path, str) or not base_path.strip():
            continue
        records.append({"name": name, "base_path": base_path})
    return records


def _iter_lxss_subkeys() -> dict[str, dict[str, str]]:
    """Enumerate ``HKCU\\...\\Lxss`` subkeys and read ``DistributionName``/``BasePath``.

    IO function: talks to ``winreg`` directly (unlike ``_parse_lxss_distros``,
    which is pure). Tolerant of the ``Lxss`` key being entirely absent
    (returns ``{}``) and of any individual subkey failing to open or read a
    value (that subkey simply contributes whichever of the two values *did*
    read successfully, possibly neither — ``_parse_lxss_distros`` then skips
    it). Only ``winreg.OpenKey``/``winreg.EnumKey``/``winreg.QueryValueEx``
    (read APIs) are used.
    """
    subkeys: dict[str, dict[str, str]] = {}
    try:
        lxss_key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, LXSS_SUBKEY, 0, winreg.KEY_READ)
    except OSError:
        return subkeys

    with lxss_key:
        index = 0
        while True:
            try:
                subkey_name = winreg.EnumKey(lxss_key, index)
            except OSError:
                break
            index += 1

            values: dict[str, str] = {}
            try:
                with winreg.OpenKey(lxss_key, subkey_name, 0, winreg.KEY_READ) as subkey:
                    for value_name in ("DistributionName", "BasePath"):
                        try:
                            value, _value_type = winreg.QueryValueEx(subkey, value_name)
                        except OSError:
                            continue
                        if isinstance(value, str):
                            values[value_name] = value
            except OSError:
                pass
            subkeys[subkey_name] = values
    return subkeys


def _distro_items(records: list[dict[str, str]]) -> list[InventoryItem]:
    """Build ``InventoryItem``s for distro records whose ``ext4.vhdx`` exists on disk.

    A record whose ``base_path`` does not exist, or whose ``ext4.vhdx`` is
    missing/unreadable, is skipped entirely — a registered distro with no
    on-disk vhdx (e.g. a stale registration) is not reportable as a sized
    item.
    """
    items: list[InventoryItem] = []
    for record in records:
        vhdx_path = Path(record["base_path"]) / _VHDX_FILENAME
        try:
            if not vhdx_path.is_file():
                continue
            size = vhdx_path.stat().st_size
            resolved_path = vhdx_path.resolve()
        except OSError:
            continue
        items.append(
            InventoryItem(
                source="docker-wsl",
                name=record["name"],
                path=str(resolved_path),
                size_bytes=size,
                detail={"distro": record["name"], "kind": "wsl-distro"},
            )
        )
    return items


def scan_docker_wsl(base: dict[str, str] | None = None) -> list[InventoryItem]:
    """Scan Docker/WSL2 ``.vhdx`` files and registered WSL distros into ``InventoryItem``s.

    ``base``, when given, overrides the vhdx-glob root for testability (see
    ``_resolve_docker_wsl_root``); it has no effect on the ``Lxss`` registry
    enumeration, which always reads the live registry (tests instead patch
    ``_iter_lxss_subkeys`` directly, or exercise ``_parse_lxss_distros`` in
    isolation with an injected dict).

    Raises ``WinregUnavailableError`` (imported from ``registry.py``) if
    ``winreg`` failed to import (i.e. this is not Windows). Once ``winreg``
    is available, a missing vhdx root and a missing/empty ``Lxss`` key are
    both tolerated independently — if neither contributes anything, ``[]``
    is returned without raising.

    When the same absolute vhdx path is found by both the glob and a
    registered distro, only the distro-tagged item is kept (it carries the
    distro name — strictly more information than the bare glob item).
    """
    if not WINREG_AVAILABLE:
        raise WinregUnavailableError(
            "winreg n'est pas disponible sur cette plateforme (module stdlib "
            "réservé à Windows) : la source docker-wsl ne peut pas être scannée."
        )

    glob_items = _vhdx_glob_items(base)

    raw_subkeys = _iter_lxss_subkeys()
    records = _parse_lxss_distros(raw_subkeys)
    distro_items = _distro_items(records)

    distro_paths = {item.path for item in distro_items}
    deduped_glob_items = [item for item in glob_items if item.path not in distro_paths]

    return deduped_glob_items + distro_items
