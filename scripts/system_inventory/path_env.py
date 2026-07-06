"""PATH source: user + system ``PATH`` entries, dead-entry flagging.

Reads the raw (unexpanded — ``REG_EXPAND_SZ``) ``Path`` value from
``HKCU\\Environment`` (user PATH) and from
``HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment``
(system PATH), splits each on ``;``, and emits one ``InventoryItem`` per
entry with ``source="path"``, ``size_bytes=None`` (a PATH entry is not a
sized object — it is a *pointer*, possibly to nothing at all) and
``detail={"origin": "user"|"system", "status": "alive"|"dead"}``.

Signalling only: this module never modifies the registry, never modifies
``PATH``, and never modifies anything else. It only *reads* the raw values
and *reports* whether each entry currently resolves to an existing folder.

Read-only contract: only ``winreg.OpenKey`` / ``winreg.QueryValueEx`` are
used. No write API (``SetValueEx``, ``CreateKey``, ``CreateKeyEx``,
``DeleteKey``, ``DeleteValue``, ...) is imported or referenced anywhere in
this module — enforced by ``tests/test_path_env.py`` via the same
source-text scan pattern used by ``tests/test_registry.py``.

``winreg`` is Windows-only stdlib; the import is guarded exactly like
``registry.py``'s (``WINREG_AVAILABLE`` records success/failure). The
``WinregUnavailableError`` class itself is *not* redefined here: it is
imported straight from ``registry.py`` and reused verbatim, so a single
``except WinregUnavailableError:`` in the orchestrator (``inventory.py``)
catches it regardless of which winreg-dependent source raised it.
"""

from __future__ import annotations

import os
from typing import Callable

from scripts.system_inventory.common import InventoryItem
from scripts.system_inventory.registry import WinregUnavailableError

try:
    import winreg

    WINREG_AVAILABLE = True
except ImportError:  # pragma: no cover - exercised only on non-Windows.
    winreg = None  # type: ignore[assignment]
    WINREG_AVAILABLE = False

_USER_PATH_HIVE_ATTR = "HKEY_CURRENT_USER"
_USER_PATH_SUBKEY = "Environment"

_SYSTEM_PATH_HIVE_ATTR = "HKEY_LOCAL_MACHINE"
_SYSTEM_PATH_SUBKEY = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"

_PATH_VALUE_NAME = "Path"


def split_path_value(raw: str) -> list[str]:
    """Split a raw ``;``-delimited ``PATH`` string into stripped, non-empty entries.

    Pure: no registry, no filesystem access. Handles the common noise found
    in real-world ``PATH`` values — a trailing ``;``, doubled ``;;``,
    leading/trailing whitespace around an entry — by stripping each segment
    and dropping any that end up empty. ``""`` or falsy ``raw`` yields ``[]``.
    """
    if not raw:
        return []
    return [segment.strip() for segment in raw.split(";") if segment.strip()]


def _default_exists(entry: str) -> bool:
    """Production existence check: ``os.path.isdir(os.path.expandvars(entry))``.

    Entries are reported to the caller in their original, unexpanded form
    (e.g. ``%JAVA_HOME%\\bin``), but existence is checked against the
    *expanded* path, since that is what the shell would actually resolve at
    runtime. If an entry has no ``%VAR%`` placeholders, ``expandvars`` is a
    no-op.
    """
    return os.path.isdir(os.path.expandvars(entry))


def _build_path_items(
    raw_values: dict[str, str | None],
    exists_fn: Callable[[str], bool] | None = None,
) -> list[InventoryItem]:
    """Turn raw ``{"user": <raw or None>, "system": <raw or None>}`` into items.

    Pure(-ish): the only external dependency is ``exists_fn`` (defaults to
    ``_default_exists``, i.e. real ``os.path.isdir``/``os.path.expandvars``),
    which tests can override to avoid touching the real filesystem, or can
    leave at the default and point entries at real ``tempfile`` directories
    instead. Never touches ``winreg`` — only ``scan_path()`` does that.

    A missing/``None``/empty origin is tolerated: that origin simply
    contributes no entries (this is how "only user PATH present", "only
    system PATH present", and "both absent" all degrade to fewer or zero
    items instead of raising).

    Entries are deduplicated by ``(origin, normalized path)`` — normalized
    via ``os.path.normcase(os.path.normpath(entry))`` — so trailing
    separators or case-only differences within the *same* origin collapse
    to a single item. The *first* occurrence's original (unexpanded) text is
    kept as both ``name`` and ``path``, preserving casing and any
    ``%VAR%`` placeholders for display. The same path duplicated across
    *different* origins (user vs system) is kept as two separate items,
    since "origin" is itself meaningful information here, not noise.

    Every emitted item has ``size_bytes=None`` (a PATH entry is not sized)
    and ``detail={"origin": "user"|"system", "status": "alive"|"dead"}``,
    where ``status`` reflects ``exists_fn(entry)`` at scan time.
    """
    check_exists = exists_fn or _default_exists
    items: list[InventoryItem] = []
    seen: set[tuple[str, str]] = set()

    for origin in ("user", "system"):
        raw = raw_values.get(origin)
        if not raw:
            continue
        for entry in split_path_value(raw):
            normalized = os.path.normcase(os.path.normpath(entry))
            dedup_key = (origin, normalized)
            if dedup_key in seen:
                continue
            seen.add(dedup_key)

            status = "alive" if check_exists(entry) else "dead"
            items.append(
                InventoryItem(
                    source="path",
                    name=entry,
                    path=entry,
                    size_bytes=None,
                    detail={"origin": origin, "status": status},
                )
            )
    return items


def _read_raw_path_value(hive_attr: str, subkey: str) -> str | None:
    """Read the raw (unexpanded) ``Path`` value off one registry key.

    IO function: talks to ``winreg`` directly (unlike ``_build_path_items``,
    which is pure). Tolerant of the key or value being entirely absent
    (returns ``None``) — this is how "only one origin present" degrades
    without raising. Only ``winreg.OpenKey``/``winreg.QueryValueEx`` (read
    APIs) are used.
    """
    hive_const = getattr(winreg, hive_attr)
    try:
        with winreg.OpenKey(hive_const, subkey, 0, winreg.KEY_READ) as key:
            value, _value_type = winreg.QueryValueEx(key, _PATH_VALUE_NAME)
    except OSError:
        return None
    return value if isinstance(value, str) else None


def scan_path() -> list[InventoryItem]:
    """Scan the user and system ``PATH`` registry values into ``InventoryItem``s.

    Raises ``WinregUnavailableError`` (imported from ``registry.py``) if
    ``winreg`` failed to import (i.e. this is not Windows) — this source
    fundamentally requires the registry, so there is no meaningful degraded
    behavior to fall back to. Once ``winreg`` is available, any other
    absence (either registry key, or the ``Path`` value under it) is
    tolerated: that origin simply contributes no entries, and both absent
    means ``scan_path()`` returns ``[]`` without raising.

    Never writes to the registry, never modifies ``PATH``: only
    ``winreg.OpenKey``/``winreg.QueryValueEx`` are used.
    """
    if not WINREG_AVAILABLE:
        raise WinregUnavailableError(
            "winreg n'est pas disponible sur cette plateforme (module stdlib "
            "réservé à Windows) : la source PATH ne peut pas être scannée."
        )

    raw_values = {
        "user": _read_raw_path_value(_USER_PATH_HIVE_ATTR, _USER_PATH_SUBKEY),
        "system": _read_raw_path_value(_SYSTEM_PATH_HIVE_ATTR, _SYSTEM_PATH_SUBKEY),
    }
    return _build_path_items(raw_values)
