"""Native (non-WSL) Docker inventory — Linux counterpart of ``docker_wsl.py``.

One scanner: ``scan_docker_native``, one ``InventoryItem`` per
``docker system df`` category (``Images``/``Containers``/``Local Volumes``/
``Build Cache``), tagged ``source="docker-native"``, ``path=None`` (a Docker
usage category has no single filesystem path the way an installed package or
a vhdx file does — mirrors how ``systemd.py`` leaves ``path=None`` for the
same reason). ``detail`` carries ``active``/``total_count``/``reclaimable``
as reported by Docker itself, always as strings.

``docker system df --format json`` emits **NDJSON** (one JSON object per
line, one per category) rather than a single JSON array — confirmed against
a real Docker 29.6.2 install. Its ``Size``/``Reclaimable`` fields use
Docker's own human-readable size strings in **decimal SI units** (go-units
``HumanSize``: ``B``/``kB``/``MB``/``GB``/``TB``, base 1000) — distinct from
the binary KiB/MiB/GiB units ``packages_linux.py``'s pacman-size parser
uses, so this module has its own unit table.

When the ``docker`` binary is absent, or present but ``docker system df``
fails or yields nothing parseable (e.g. daemon not running), this falls back
to sizing ``/var/lib/docker`` directly via ``common.dir_size_on_disk`` — a
single item tagged ``detail={"kind": "fallback-dir"}``. A missing
``/var/lib/docker`` (Docker never installed at all) is tolerated and yields
``[]``. A present-but-unreadable ``/var/lib/docker`` (confirmed on this dev
machine: a non-root user gets ``Permission denied``) still yields an item,
via ``dir_size_on_disk``'s existing per-entry ``OSError``/``PermissionError``
tolerance — whatever partial size could be read, possibly 0, never raising.

Stdlib only (``subprocess`` + ``json`` + ``shutil`` + ``re``). Read-only:
only ``docker system df`` (a read-only subcommand) is ever invoked — never
any mutating ``docker`` command.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from typing import Sequence

from scripts.system_inventory.common import InventoryItem, dir_size_on_disk

_TOOL_TIMEOUT_SECONDS = 30

_DOCKER_SYSTEM_DF_COMMAND: tuple[str, ...] = ("docker", "system", "df", "--format", "json")

_VAR_LIB_DOCKER_PATH = "/var/lib/docker"

_DOCKER_SIZE_PATTERN = re.compile(r"^([\d.]+)\s*([A-Za-z]+)$")

# Decimal SI units (go-units HumanSize, base 1000) — Docker's own size
# strings, distinct from the binary (base 1024) units used elsewhere in this
# package (see packages_linux._PACMAN_UNIT_MULTIPLIERS).
_DOCKER_UNIT_MULTIPLIERS: dict[str, float] = {
    "B": 1,
    "kB": 1000,
    "MB": 1000**2,
    "GB": 1000**3,
    "TB": 1000**4,
    "PB": 1000**5,
}


def _run(command: Sequence[str]) -> subprocess.CompletedProcess[str] | None:
    """Run a read-only ``docker`` query subprocess, tolerantly.

    Mirrors ``packages_linux._run``/``systemd._run``: any
    ``OSError``/``SubprocessError`` degrades to ``None`` rather than
    propagating.
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


def _parse_docker_size(text: str) -> int | None:
    """Parse a Docker human-size string (e.g. ``"14.9GB"``, ``"0B"``) into bytes.

    A trailing ``" (NN%)"`` suffix (seen on the ``Reclaimable`` field, not
    ``Size``) is stripped before matching, defensively. An empty, missing,
    or unrecognized-unit value yields ``None`` rather than a fabricated
    size.
    """
    value = text.strip()
    paren_index = value.find("(")
    if paren_index != -1:
        value = value[:paren_index].strip()
    match = _DOCKER_SIZE_PATTERN.match(value)
    if match is None:
        return None
    number_str, unit = match.groups()
    multiplier = _DOCKER_UNIT_MULTIPLIERS.get(unit)
    if multiplier is None:
        return None
    try:
        number = float(number_str)
    except ValueError:
        return None
    return int(number * multiplier)


def _parse_docker_system_df(output: str) -> list[InventoryItem]:
    """Parse ``docker system df --format json`` NDJSON output.

    One JSON object per line, one line per category. A blank line or a line
    that fails to parse as JSON (or has no ``Type``) is skipped rather than
    aborting the whole parse.
    """
    items: list[InventoryItem] = []
    for line in output.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        type_name = entry.get("Type")
        if not type_name:
            continue
        items.append(
            InventoryItem(
                source="docker-native",
                name=type_name,
                path=None,
                size_bytes=_parse_docker_size(str(entry.get("Size", ""))),
                detail={
                    "active": str(entry.get("Active", "")),
                    "total_count": str(entry.get("TotalCount", "")),
                    "reclaimable": str(entry.get("Reclaimable", "")),
                },
            )
        )
    return items


def _fallback_var_lib_docker() -> list[InventoryItem]:
    """Size ``/var/lib/docker`` directly when the ``docker`` CLI/daemon is unusable.

    Present-only: a missing ``/var/lib/docker`` yields ``[]``. A
    present-but-unreadable directory still yields an item, sized via
    ``dir_size_on_disk``'s existing error tolerance.
    """
    if not os.path.isdir(_VAR_LIB_DOCKER_PATH):
        return []
    size = dir_size_on_disk(_VAR_LIB_DOCKER_PATH)
    return [
        InventoryItem(
            source="docker-native",
            name="Docker (/var/lib/docker)",
            path=_VAR_LIB_DOCKER_PATH,
            size_bytes=size,
            detail={"kind": "fallback-dir"},
        )
    ]


def scan_docker_native() -> list[InventoryItem]:
    """Inventory Docker's own reported disk usage, falling back to a directory size.

    ``docker`` binary present and ``docker system df --format json``
    succeeding with at least one parseable category: one item per category.
    Otherwise (binary absent, non-zero exit, or nothing parseable — e.g. the
    daemon is not running): falls back to sizing ``/var/lib/docker``
    directly (see ``_fallback_var_lib_docker``). Neither path yielding
    anything is tolerated and returns ``[]``.
    """
    if shutil.which("docker") is not None:
        completed = _run(_DOCKER_SYSTEM_DF_COMMAND)
        if completed is not None and completed.returncode == 0:
            items = _parse_docker_system_df(completed.stdout)
            if items:
                return items
    return _fallback_var_lib_docker()
