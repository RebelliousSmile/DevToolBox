"""Read-only Linux application collectors for APT/dpkg, Snap and Flatpak."""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
from pathlib import Path
from typing import Iterable, Sequence

from scripts.app_recommendations.models import (
    Candidate,
    CommandSuggestion,
    Protection,
    SizeEvidence,
)
from scripts.system_inventory.packages_linux import query_dpkg_installed_sizes

TOOL_TIMEOUT_SECONDS = 20
GENERIC_WRAPPERS = frozenset({"env", "flatpak", "snap", "sh", "bash", "python", "python3"})
APT_PROTECTED_NAMES = frozenset(
    {
        "gnome-shell",
        "plasma-desktop",
        "snapd",
        "software-properties-common",
        "ubuntu-desktop",
        "ubuntu-desktop-minimal",
    }
)


def _run(command: Sequence[str]) -> str:
    completed = subprocess.run(
        list(command),
        capture_output=True,
        text=True,
        timeout=TOOL_TIMEOUT_SECONDS,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or f"exit {completed.returncode}"
        raise RuntimeError(f"{' '.join(command[:2])}: {detail}")
    return completed.stdout


def _run_partial(command: Sequence[str]) -> str:
    """Run a query whose non-zero status may still carry valid stdout rows."""
    completed = subprocess.run(
        list(command),
        capture_output=True,
        text=True,
        timeout=TOOL_TIMEOUT_SECONDS,
        check=False,
    )
    if completed.returncode != 0 and not completed.stdout.strip():
        detail = completed.stderr.strip() or f"exit {completed.returncode}"
        raise RuntimeError(f"{' '.join(command[:2])}: {detail}")
    return completed.stdout


def _desktop_roots() -> list[Path]:
    roots = [Path("/usr/share/applications"), Path("/usr/local/share/applications")]
    home = Path.home()
    roots.append(home / ".local/share/applications")
    for value in os.environ.get("XDG_DATA_DIRS", "").split(":"):
        if value:
            roots.append(Path(value) / "applications")
    return list(dict.fromkeys(roots))


def _parse_desktop_entry(text: str) -> dict[str, str]:
    values: dict[str, str] = {}
    in_desktop_entry = False
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if line.startswith("[") and line.endswith("]"):
            in_desktop_entry = line == "[Desktop Entry]"
            continue
        if not in_desktop_entry or not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        if key in {"Name", "Exec", "Type", "Hidden", "NoDisplay"}:
            values.setdefault(key, value.strip())
    return values


def _command_from_exec(exec_value: str) -> str | None:
    try:
        tokens = shlex.split(exec_value)
    except ValueError:
        return None
    tokens = [token for token in tokens if not token.startswith("%")]
    if not tokens:
        return None
    index = 0
    if Path(tokens[index]).name == "env":
        index += 1
        while index < len(tokens) and "=" in tokens[index] and not tokens[index].startswith("/"):
            index += 1
    if index >= len(tokens):
        return None
    command = tokens[index]
    if Path(command).name in {"flatpak", "snap"} or command.startswith("/snap/bin/"):
        return None
    if not Path(command).is_absolute():
        command = shutil.which(command) or command
    if Path(command).name in GENERIC_WRAPPERS:
        return None
    return command


def _iter_desktop_apps(roots: Iterable[Path] | None = None) -> list[tuple[str, str]]:
    apps: list[tuple[str, str]] = []
    for root in roots or _desktop_roots():
        try:
            entries = sorted(root.glob("*.desktop"))
        except OSError:
            continue
        for path in entries:
            try:
                values = _parse_desktop_entry(path.read_text(encoding="utf-8", errors="replace"))
            except OSError:
                continue
            if values.get("Type", "Application") != "Application":
                continue
            if values.get("Hidden", "false").lower() == "true" or values.get("NoDisplay", "false").lower() == "true":
                continue
            name = values.get("Name")
            command = _command_from_exec(values.get("Exec", ""))
            if name and command:
                apps.append((name, command))
    return apps


def _dpkg_owners(executables: Iterable[str]) -> dict[str, str]:
    """Resolve desktop executables in bounded batches, avoiding an N+1 query."""
    requested = sorted({executable for executable in executables if executable})
    owners_by_path: dict[str, set[str]] = {path: set() for path in requested}
    for offset in range(0, len(requested), 200):
        chunk = requested[offset : offset + 200]
        if not chunk:
            continue
        try:
            output = _run_partial(("dpkg-query", "-S", *chunk))
        except RuntimeError:
            continue
        for line in output.splitlines():
            owner_spec, separator, matched_path = line.rpartition(": ")
            if not separator or matched_path not in owners_by_path:
                continue
            if ", " in owner_spec:
                continue
            owner = owner_spec.split(":", 1)[0]
            if owner:
                owners_by_path[matched_path].add(owner)
    return {
        path: next(iter(owners))
        for path, owners in owners_by_path.items()
        if len(owners) == 1
    }


def _apt_auto_packages() -> set[str]:
    if shutil.which("apt-mark") is None:
        return set()
    return {line.strip() for line in _run(("apt-mark", "showauto")).splitlines() if line.strip()}


def collect_apt_apps(
    desktop_apps: list[tuple[str, str]] | None = None,
    sizes: dict[str, int | None] | None = None,
    auto_packages: set[str] | None = None,
) -> list[Candidate]:
    if shutil.which("dpkg-query") is None:
        raise RuntimeError("dpkg-query absent")
    sizes = query_dpkg_installed_sizes() if sizes is None else sizes
    auto_packages = _apt_auto_packages() if auto_packages is None else auto_packages
    apps = desktop_apps if desktop_apps is not None else _iter_desktop_apps()
    owners = _dpkg_owners(executable for _, executable in apps)
    by_package: dict[str, Candidate] = {}
    for display_name, executable in apps:
        package = owners.get(executable)
        if not package or package not in sizes:
            continue
        protected_reasons: list[str] = []
        if package in auto_packages:
            protected_reasons.append("dépendance installée automatiquement")
        if package in APT_PROTECTED_NAMES:
            protected_reasons.append("composant du bureau ou du système")
        candidate = Candidate(
            app_id=f"apt:{package}",
            source="apt",
            name=display_name,
            size=SizeEvidence(
                installed_bytes=sizes[package],
                method="dpkg_installed_size",
                scope="paquet hors données utilisateur",
                confidence="high" if sizes[package] is not None else "unknown",
            ),
            executable_hints=[executable] if Path(executable).is_absolute() else [],
            protection=Protection(bool(protected_reasons), protected_reasons),
            command=CommandSuggestion(f"sudo apt-get remove -- {shlex.quote(package)}"),
            metadata={"package": package},
        )
        existing = by_package.get(package)
        if existing is None or candidate.name.casefold() < existing.name.casefold():
            by_package[package] = candidate
        elif executable not in existing.executable_hints and Path(executable).is_absolute():
            existing.executable_hints.append(executable)
            existing.executable_hints.sort()
    return sorted(by_package.values(), key=lambda item: item.app_id)


def _parse_size(text: str) -> int | None:
    parts = text.strip().replace(",", ".").split()
    if len(parts) != 2:
        return None
    try:
        value = float(parts[0])
    except ValueError:
        return None
    multipliers = {"B": 1, "kB": 1000, "MB": 1000**2, "GB": 1000**3, "KiB": 1024, "MiB": 1024**2, "GiB": 1024**3}
    multiplier = multipliers.get(parts[1])
    return int(value * multiplier) if multiplier is not None else None


def _specific_executable(directory: Path, preferred: str) -> str | None:
    try:
        entries = [entry for entry in directory.iterdir() if entry.is_file() and os.access(entry, os.X_OK)]
    except OSError:
        return None
    preferred_names = {preferred, preferred.rsplit(".", 1)[-1]}
    matches = [entry for entry in entries if entry.name in preferred_names]
    if len(matches) == 1:
        return str(matches[0].resolve())
    if len(entries) == 1:
        return str(entries[0].resolve())
    return None


def _parse_snap_list(output: str) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for index, line in enumerate(output.splitlines()):
        if index == 0 or not line.strip():
            continue
        parts = line.split()
        if len(parts) >= 3:
            rows.append({"name": parts[0], "revision": parts[2], "notes": parts[-1]})
    return rows


def collect_snap_apps(output: str | None = None, snap_root: Path = Path("/snap"), archive_root: Path = Path("/var/lib/snapd/snaps")) -> list[Candidate]:
    if output is None:
        if shutil.which("snap") is None:
            raise RuntimeError("snap absent")
        output = _run(("snap", "list", "--unicode=never"))
    candidates: list[Candidate] = []
    for row in _parse_snap_list(output):
        name, revision, notes = row["name"], row["revision"], row["notes"]
        is_shared = "base" in notes or name in {"core", "core18", "core20", "core22", "core24", "snapd"}
        archive = archive_root / f"{name}_{revision}.snap"
        try:
            size = archive.stat().st_size
        except OSError:
            size = None
        hint = _specific_executable(snap_root / name / revision / "usr/bin", name)
        reasons = ["base ou composant partagé Snap"] if is_shared else []
        candidates.append(
            Candidate(
                app_id=f"snap:{name}",
                source="snap",
                name=name,
                size=SizeEvidence(size, None, "snap_archive", "archive de la révision active, hors données utilisateur", "high" if size is not None else "unknown"),
                executable_hints=[hint] if hint else [],
                protection=Protection(is_shared, reasons),
                command=CommandSuggestion(f"sudo snap remove {shlex.quote(name)}"),
                metadata={"revision": revision},
            )
        )
    return candidates


def _parse_flatpak_list(output: str) -> list[dict[str, str | int | None]]:
    rows: list[dict[str, str | int | None]] = []
    for line in output.splitlines():
        parts = line.split("\t")
        if len(parts) != 5:
            continue
        app_id, name, size, installation, runtime = parts
        rows.append({"app_id": app_id, "name": name, "size": _parse_size(size), "installation": installation, "runtime": runtime})
    return rows


def _flatpak_deployment(app_id: str, installation: str) -> Path:
    root = Path.home() / ".local/share/flatpak" if installation == "user" else Path("/var/lib/flatpak")
    return root / "app" / app_id / "current/active/files/bin"


def collect_flatpak_apps(output: str | None = None) -> list[Candidate]:
    if output is None:
        if shutil.which("flatpak") is None:
            raise RuntimeError("flatpak absent")
        output = _run(("flatpak", "list", "--app", "--columns=application,name,size,installation,runtime"))
    candidates: list[Candidate] = []
    for row in _parse_flatpak_list(output):
        app_id = str(row["app_id"])
        installation = str(row["installation"])
        hint = _specific_executable(_flatpak_deployment(app_id, installation), app_id)
        scope_flag = "--user" if installation == "user" else "--system"
        candidates.append(
            Candidate(
                app_id=f"flatpak:{app_id}",
                source="flatpak",
                name=str(row["name"]),
                size=SizeEvidence(row["size"] if isinstance(row["size"], int) else None, None, "flatpak_reported_size", "application hors runtime partagé", "medium" if row["size"] is not None else "unknown"),
                executable_hints=[hint] if hint else [],
                command=CommandSuggestion(f"flatpak uninstall {scope_flag} {shlex.quote(app_id)}"),
                metadata={"runtime": str(row["runtime"]), "installation": installation},
            )
        )
    return candidates


def linux_collectors():
    return {"apt": collect_apt_apps, "flatpak": collect_flatpak_apps, "snap": collect_snap_apps}
