"""Registry source: Windows Uninstall keys (HKLM + HKCU, 32/64-bit views).

Reads the "Add/Remove Programs" inventory straight from the registry —
``DisplayName``, ``InstallDate``, ``EstimatedSize``, ``InstallLocation`` —
and emits ``InventoryItem`` records with ``source="registry"``.

Read-only contract: only ``OpenKey`` / ``EnumKey`` / ``QueryValueEx`` are
used. No write API (``SetValueEx``, ``CreateKey``, ``CreateKeyEx``,
``DeleteKey``, ``DeleteValue``, ...) is imported or referenced anywhere in
this module — enforced by ``tests/test_registry.py`` via a source-text scan,
in addition to the read-only contract being upheld by construction here.

``winreg`` is Windows-only stdlib. Importing it unconditionally would crash
module *load* on any other platform, which would in turn break simply
importing this package (e.g. for tooling, linting, or an orchestrator that
wants to enumerate available sources before deciding what to run). Instead:
the import is guarded, ``WINREG_AVAILABLE`` records whether it succeeded,
and ``scan_registry()`` raises the catchable ``WinregUnavailableError`` at
*call* time if it did not. This lets Part 3's orchestrator do
``try: scan_registry() except WinregUnavailableError:`` and degrade with a
clear message instead of the whole program dying at import time.
"""

from __future__ import annotations

from scripts.system_inventory.common import InventoryItem

try:
    import winreg

    WINREG_AVAILABLE = True
except ImportError:  # pragma: no cover - exercised only on non-Windows.
    winreg = None  # type: ignore[assignment]
    WINREG_AVAILABLE = False


class WinregUnavailableError(RuntimeError):
    """Raised by ``scan_registry()`` when ``winreg`` failed to import.

    Callers (the Part 3 orchestrator) should catch this specifically to
    report "registry source unavailable on this platform" rather than let
    an unhandled exception propagate.
    """


UNINSTALL_SUBKEY = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"

_UNINSTALL_VALUE_NAMES = ("DisplayName", "InstallDate", "EstimatedSize", "InstallLocation")

# Scan targets as (hive_label, hive_const_attr, view_label, view_flag_attr).
# The winreg attribute *names* (not the objects) are stored here because
# winreg may be unavailable at import time (see module docstring); they are
# resolved via getattr(winreg, ...) lazily, inside scan_registry(), once we
# know winreg actually imported.
#
# HKLM genuinely has two distinct views: 32-bit installers are redirected
# to a WOW6432Node-backed view and are invisible under the 64-bit view (and
# vice versa for 64-bit installers) — both are scanned and kept as distinct
# nodes per the plan's risk register (they are not "the same app twice").
#
# HKCU is deliberately scanned *once*, not twice. Unlike HKLM\Software,
# HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall is not subject to
# WOW64 registry redirection: opening it with KEY_WOW64_32KEY vs
# KEY_WOW64_64KEY returns the *same* key content. Mirroring the two-view
# HKLM loop verbatim for HKCU would therefore enumerate every per-user
# entry twice under two different "view" labels — a real double-count of
# EstimatedSize into the grand total, which is exactly the inflated-
# inventory risk the plan's risk register warns about. We scan it once,
# tagged view="64" for schema consistency with the HKLM rows.
_SCAN_TARGETS: tuple[tuple[str, str, str, str], ...] = (
    ("HKLM", "HKEY_LOCAL_MACHINE", "64", "KEY_WOW64_64KEY"),
    ("HKLM", "HKEY_LOCAL_MACHINE", "32", "KEY_WOW64_32KEY"),
    ("HKCU", "HKEY_CURRENT_USER", "64", "KEY_WOW64_64KEY"),
)


def _read_uninstall_entry(values: dict[str, object]) -> dict[str, object] | None:
    """Parse one Uninstall subkey's values into plain (non-winreg) data.

    ``values`` is a plain ``dict`` of value-name -> raw value, as if each of
    ``_UNINSTALL_VALUE_NAMES`` had been read via ``winreg.QueryValueEx`` and
    collected into a dict (absent value names are simply absent keys). This
    function never touches ``winreg`` itself, so tests can call it directly
    with fake dicts — only ``scan_registry()`` talks to the real registry.

    Returns ``None`` when ``DisplayName`` is missing or blank: a nameless
    entry isn't reportable, so the subkey is skipped entirely. Any other
    missing/malformed value is tolerated:

    * missing/non-int ``EstimatedSize`` -> ``size_bytes=None`` (never
      fabricated);
    * missing/non-str ``InstallDate`` / ``InstallLocation`` -> ``""``.
    """
    name = values.get("DisplayName")
    if not isinstance(name, str) or not name.strip():
        return None

    estimated_size_kb = values.get("EstimatedSize")
    size_bytes = estimated_size_kb * 1024 if isinstance(estimated_size_kb, int) else None

    install_date = values.get("InstallDate")
    install_location = values.get("InstallLocation")

    return {
        "name": name,
        "size_bytes": size_bytes,
        "install_date": install_date if isinstance(install_date, str) else "",
        "install_location": install_location if isinstance(install_location, str) else "",
    }


def _query_uninstall_values(subkey: object) -> dict[str, object]:
    """Read ``_UNINSTALL_VALUE_NAMES`` off an already-open ``winreg`` subkey.

    Talks to ``winreg`` directly (unlike ``_read_uninstall_entry``, which is
    pure); kept separate so the pure-parsing logic stays unit-testable
    without any live registry handle.
    """
    values: dict[str, object] = {}
    for value_name in _UNINSTALL_VALUE_NAMES:
        try:
            value, _value_type = winreg.QueryValueEx(subkey, value_name)
        except OSError:
            continue
        values[value_name] = value
    return values


def scan_registry() -> list[InventoryItem]:
    """Scan the Uninstall registry (HKLM x2 views + HKCU) into ``InventoryItem``s.

    Deduplicates on the exact ``(hive, view, subkey name)`` triple only —
    genuinely distinct HKLM-64 / HKLM-32 / HKCU entries for what might look
    like "the same" DisplayName are kept as separate items (see the
    ``_SCAN_TARGETS`` docstring above); only an exact duplicate triple
    (the same key somehow enumerated twice) is collapsed.

    Raises ``WinregUnavailableError`` if ``winreg`` failed to import (i.e.
    this is not Windows). Never raises for a single unreadable subkey or
    missing value: those are skipped and the scan continues.
    """
    if not WINREG_AVAILABLE:
        raise WinregUnavailableError(
            "winreg n'est pas disponible sur cette plateforme (module stdlib "
            "réservé à Windows) : la source registre ne peut pas être scannée."
        )

    items: list[InventoryItem] = []
    seen: set[tuple[str, str, str]] = set()

    for hive_label, hive_const_attr, view_label, view_flag_attr in _SCAN_TARGETS:
        hive_const = getattr(winreg, hive_const_attr)
        view_flag = getattr(winreg, view_flag_attr)

        try:
            uninstall_key = winreg.OpenKey(hive_const, UNINSTALL_SUBKEY, 0, winreg.KEY_READ | view_flag)
        except OSError:
            continue

        with uninstall_key:
            index = 0
            while True:
                try:
                    subkey_name = winreg.EnumKey(uninstall_key, index)
                except OSError:
                    break
                index += 1

                dedup_key = (hive_label, view_label, subkey_name)
                if dedup_key in seen:
                    continue
                seen.add(dedup_key)

                try:
                    with winreg.OpenKey(uninstall_key, subkey_name, 0, winreg.KEY_READ | view_flag) as subkey:
                        values = _query_uninstall_values(subkey)
                except OSError:
                    continue

                parsed = _read_uninstall_entry(values)
                if parsed is None:
                    continue

                items.append(
                    InventoryItem(
                        source="registry",
                        name=str(parsed["name"]),
                        path=str(parsed["install_location"]) or None,
                        size_bytes=parsed["size_bytes"],  # type: ignore[arg-type]
                        detail={
                            "install_date": str(parsed["install_date"]),
                            "hive": hive_label,
                            "view": view_label,
                        },
                    )
                )

    return items
