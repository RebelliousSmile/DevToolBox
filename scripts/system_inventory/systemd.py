"""systemd service/timer inventory — Linux batch-report scanner.

Distinct from ``src/linux/automations.rs`` (Part 3), which powers the UI's
Automations view directly by shelling out to ``systemctl`` live for
interactive enable/disable actions. This module instead produces
``InventoryItem`` records for the offline, read-only ``inventory.py`` batch
report: one item per service unit and one per timer unit.

systemd units have no meaningful "disk size" the way an installed package or
an AppData folder does, so every item's ``size_bytes`` is ``None`` (rendered
as ``"unknown"`` by ``common.human_size``, and contributing 0 to the grand
total) — never fabricated. State information is carried in ``detail``
instead: ``load``/``active``/``sub`` for services (e.g. ``loaded`` /
``active`` / ``running``), ``activates`` (the service unit a timer triggers)
for timers.

Stdlib only (``subprocess`` + ``json`` + ``shutil``). Read-only: only
``systemctl list-units``/``list-timers`` (read-only subcommands) are ever
invoked — never any mutating ``systemctl`` subcommand (``start``, ``stop``,
``enable``, ``disable``, ...).
"""

from __future__ import annotations

import json
import shutil
import subprocess
from typing import Sequence

from scripts.system_inventory.common import InventoryItem

_TOOL_TIMEOUT_SECONDS = 30

_LIST_SERVICES_COMMAND: tuple[str, ...] = (
    "systemctl",
    "list-units",
    "--type=service",
    "--all",
    "--no-pager",
    "--output=json",
)

_LIST_TIMERS_COMMAND: tuple[str, ...] = (
    "systemctl",
    "list-timers",
    "--all",
    "--no-pager",
    "--output=json",
)


def _run(command: Sequence[str]) -> subprocess.CompletedProcess[str] | None:
    """Run a read-only ``systemctl`` query subprocess, tolerantly.

    Same tolerance contract as ``packages_linux._run``: any
    ``OSError``/``SubprocessError`` degrades to ``None`` (treated as "this
    half of the inventory is empty") rather than propagating.
    """
    try:
        return subprocess.run(
            list(command),
            capture_output=True,
            text=True,
            timeout=_TOOL_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError):
        return None


def _parse_services(output: str) -> list[InventoryItem]:
    """Parse ``systemctl list-units --type=service --output=json`` output."""
    try:
        entries = json.loads(output)
    except (json.JSONDecodeError, TypeError):
        return []
    items: list[InventoryItem] = []
    for entry in entries:
        unit = entry.get("unit")
        if not unit:
            continue
        items.append(
            InventoryItem(
                source="systemd",
                name=unit,
                path=None,
                size_bytes=None,
                detail={
                    "kind": "service",
                    "load": str(entry.get("load", "")),
                    "active": str(entry.get("active", "")),
                    "sub": str(entry.get("sub", "")),
                },
            )
        )
    return items


def _parse_timers(output: str) -> list[InventoryItem]:
    """Parse ``systemctl list-timers --output=json`` output."""
    try:
        entries = json.loads(output)
    except (json.JSONDecodeError, TypeError):
        return []
    items: list[InventoryItem] = []
    for entry in entries:
        unit = entry.get("unit")
        if not unit:
            continue
        items.append(
            InventoryItem(
                source="systemd",
                name=unit,
                path=None,
                size_bytes=None,
                detail={
                    "kind": "timer",
                    "activates": str(entry.get("activates", "")),
                },
            )
        )
    return items


def scan_systemd() -> list[InventoryItem]:
    """Inventory every systemd service and timer unit (loaded + not-loaded, all states).

    ``systemctl`` binary absent (this is not a systemd-based Linux system —
    e.g. some containers, other init systems) is tolerated and yields
    ``[]``. Either query failing independently (non-zero exit, malformed
    JSON) tolerantly contributes no items for that half, without preventing
    the other half from being reported.
    """
    if shutil.which("systemctl") is None:
        return []
    items: list[InventoryItem] = []
    services = _run(_LIST_SERVICES_COMMAND)
    if services is not None and services.returncode == 0:
        items.extend(_parse_services(services.stdout))
    timers = _run(_LIST_TIMERS_COMMAND)
    if timers is not None and timers.returncode == 0:
        items.extend(_parse_timers(timers.stdout))
    return items
