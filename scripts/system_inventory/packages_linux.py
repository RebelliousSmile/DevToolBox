"""apt/dnf/pacman package inventory — Linux equivalent of packages.py's Scoop/Chocolatey scanner.

One scanner lives here:

* ``scan_packages_linux`` — one ``InventoryItem`` per installed package,
  tagged ``source="packages-linux"``, ``detail={"manager": "apt"|"dnf"|"pacman"}``.

Package-manager auto-detection checks for each manager's query binary's
existence via ``shutil.which`` rather than inferring from ``/etc/os-release``
(Part 4 risk register): apt-based systems are detected via ``dpkg-query``,
dnf/rpm-based via ``rpm``, Arch-based via ``pacman``. Detection stops at the
first manager found — a given machine is expected to have exactly one of
these installed, not a mix. No manager recognized (e.g. a minimal container
base image, or an unsupported distro) is tolerated and yields ``[]``.

Each manager's real per-package installed size is read directly from its own
query tool (``dpkg-query``'s ``Installed-Size`` field in KiB, ``rpm``'s
``%{SIZE}`` in bytes, ``pacman -Qi``'s ``Installed Size`` field with a
human-readable unit) — sizes are read, never estimated or omitted in favor
of a stub ``None``, mirroring how ``packages.py``'s Scoop/Chocolatey items
carry a real ``size_bytes``.

Stdlib only (``subprocess`` + ``shutil``). Read-only: only each manager's
query subcommand is ever invoked (``dpkg-query -W``, ``rpm -qa``,
``pacman -Qi``) — never any mutating package-manager command.
"""

from __future__ import annotations

import shutil
import subprocess
from typing import Sequence

from scripts.system_inventory.common import InventoryItem

_TOOL_TIMEOUT_SECONDS = 30

# dpkg-query's Installed-Size field is documented as "the approximate space
# consumption in bytes, divided by 1024, rounded up" i.e. KiB. ${Status} is
# included to filter out packages that are only "deinstall ok config-files"
# (removed, config retained) — dpkg-query -W lists those too, but they are
# not installed packages.
_DPKG_QUERY_COMMAND: tuple[str, ...] = (
    "dpkg-query",
    "-W",
    "-f=${Package}\t${Installed-Size}\t${Status}\n",
)

# rpm's %{SIZE} queryformat tag is already plain bytes.
_RPM_QUERY_COMMAND: tuple[str, ...] = ("rpm", "-qa", "--queryformat", "%{NAME}\t%{SIZE}\n")

_PACMAN_QUERY_COMMAND: tuple[str, ...] = ("pacman", "-Qi")

_PACMAN_UNIT_MULTIPLIERS: dict[str, float] = {
    "B": 1,
    "KiB": 1024,
    "MiB": 1024**2,
    "GiB": 1024**3,
}


def _run(command: Sequence[str]) -> subprocess.CompletedProcess[str] | None:
    """Run a read-only package-manager query subprocess, tolerantly.

    Mirrors the ``_run`` helper already used by ``scripts/winclean/mod_apps.py``
    for its Docker probe: any ``OSError``/``SubprocessError`` (binary
    vanished between the ``shutil.which`` check and this call, a timeout,
    ...) is swallowed and reported as ``None`` rather than propagated, so a
    flaky or slow package manager degrades the whole source to "empty"
    instead of crashing the orchestrator.
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


def _parse_dpkg_query(output: str) -> list[InventoryItem]:
    """Parse ``dpkg-query -W -f='${Package}\\t${Installed-Size}\\t${Status}\\n'`` output.

    Only rows whose ``Status`` ends with ``"installed"`` are kept — dpkg
    keeps entries for removed-but-not-purged packages
    (``"deinstall ok config-files"``) in the same query, and those should
    not count as installed packages. A blank or non-numeric
    ``Installed-Size`` (seen on some third-party ``.deb`` packages that
    don't populate it) yields ``size_bytes=None`` rather than dropping the
    package entirely.
    """
    items: list[InventoryItem] = []
    for line in output.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) != 3:
            continue
        name, size_kib_str, status = parts
        if not status.endswith("installed"):
            continue
        try:
            size_bytes: int | None = int(size_kib_str) * 1024
        except ValueError:
            size_bytes = None
        items.append(
            InventoryItem(
                source="packages-linux",
                name=name,
                path=None,
                size_bytes=size_bytes,
                detail={"manager": "apt"},
            )
        )
    return items


def _parse_rpm_query(output: str) -> list[InventoryItem]:
    """Parse ``rpm -qa --queryformat '%{NAME}\\t%{SIZE}\\n'`` output."""
    items: list[InventoryItem] = []
    for line in output.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) != 2:
            continue
        name, size_bytes_str = parts
        try:
            size_bytes = int(size_bytes_str)
        except ValueError:
            continue
        items.append(
            InventoryItem(
                source="packages-linux",
                name=name,
                path=None,
                size_bytes=size_bytes,
                detail={"manager": "dnf"},
            )
        )
    return items


def _parse_pacman_size(text: str) -> int | None:
    """Parse a pacman ``Installed Size`` value like ``"12.34 MiB"`` into bytes."""
    parts = text.strip().split()
    if len(parts) != 2:
        return None
    value_str, unit = parts
    multiplier = _PACMAN_UNIT_MULTIPLIERS.get(unit)
    if multiplier is None:
        return None
    try:
        value = float(value_str)
    except ValueError:
        return None
    return int(value * multiplier)


def _parse_pacman_query(output: str) -> list[InventoryItem]:
    """Parse ``pacman -Qi`` output: blank-line-separated ``Field : value`` blocks."""
    items: list[InventoryItem] = []
    name: str | None = None
    size_bytes: int | None = None

    def _flush() -> None:
        if name is not None:
            items.append(
                InventoryItem(
                    source="packages-linux",
                    name=name,
                    path=None,
                    size_bytes=size_bytes,
                    detail={"manager": "pacman"},
                )
            )

    for line in output.splitlines():
        if not line.strip():
            _flush()
            name = None
            size_bytes = None
            continue
        if ":" not in line:
            continue
        field, _, value = line.partition(":")
        field = field.strip()
        if field == "Name":
            name = value.strip()
        elif field == "Installed Size":
            size_bytes = _parse_pacman_size(value)
    _flush()
    return items


def scan_packages_linux() -> list[InventoryItem]:
    """Inventory every installed package via the first recognized manager.

    Detection order (first binary found via ``shutil.which`` wins):
    ``dpkg-query`` (apt-based distros) -> ``rpm`` (dnf/RHEL-based distros)
    -> ``pacman`` (Arch-based distros). No recognized manager, or the found
    manager's query command failing (non-zero exit, or unavailable at call
    time despite ``shutil.which`` finding it), tolerantly yields ``[]``
    rather than raising.
    """
    if shutil.which("dpkg-query") is not None:
        completed = _run(_DPKG_QUERY_COMMAND)
        if completed is not None and completed.returncode == 0:
            return _parse_dpkg_query(completed.stdout)
        return []
    if shutil.which("rpm") is not None:
        completed = _run(_RPM_QUERY_COMMAND)
        if completed is not None and completed.returncode == 0:
            return _parse_rpm_query(completed.stdout)
        return []
    if shutil.which("pacman") is not None:
        completed = _run(_PACMAN_QUERY_COMMAND)
        if completed is not None and completed.returncode == 0:
            return _parse_pacman_query(completed.stdout)
        return []
    return []


def query_dpkg_installed_sizes() -> dict[str, int | None]:
    """Return installed dpkg package sizes without exposing scanner internals.

    This read-only primitive is shared with the application recommendation
    report, which starts from desktop launchers and then looks up only their
    owning packages instead of presenting every installed library.
    """
    if shutil.which("dpkg-query") is None:
        return {}
    completed = _run(_DPKG_QUERY_COMMAND)
    if completed is None or completed.returncode != 0:
        return {}
    return {item.name: item.size_bytes for item in _parse_dpkg_query(completed.stdout)}
