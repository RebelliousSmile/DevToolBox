"""Read-only Windows collectors for Uninstall, MSIX, Scoop and Chocolatey."""

from __future__ import annotations

import json
import ntpath
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path, PureWindowsPath
from typing import Iterable

from scripts.app_recommendations.models import (
    Candidate,
    CommandSuggestion,
    Protection,
    SizeEvidence,
)
from scripts.system_inventory.common import InventoryItem, dir_size_on_disk
from scripts.system_inventory.packages import scan_scoop_choco_windows
from scripts.system_inventory.registry import scan_uninstall_records

TOOL_TIMEOUT_SECONDS = 30
CREATE_NO_WINDOW = 0x08000000
GENERIC_EXECUTABLES = frozenset(
    {"msiexec.exe", "powershell.exe", "pwsh.exe", "rundll32.exe", "cmd.exe", "explorer.exe"}
)
UPDATE_RELEASE_TYPES = frozenset({"hotfix", "security update", "update rollup", "update"})


def _normalize_name(value: str) -> str:
    return " ".join(value.casefold().split())


def _dedupe_key(name: str, location: str) -> str | None:
    if not location.strip():
        return None
    normalized_path = ntpath.normcase(ntpath.normpath(location.strip().strip('"'))).rstrip("\\/")
    if not normalized_path:
        return None
    return f"windows-path:{normalized_path}|{_normalize_name(name)}"


def _display_icon_executable(value: str) -> str | None:
    if not value.strip():
        return None
    candidate = value.strip()
    if candidate.startswith('"'):
        closing = candidate.find('"', 1)
        candidate = candidate[1:closing] if closing > 1 else candidate.strip('"')
    else:
        candidate = candidate.rsplit(",", 1)[0].strip()
    path = PureWindowsPath(candidate)
    if not path.is_absolute() or path.suffix.casefold() != ".exe":
        return None
    if path.name.casefold() in GENERIC_EXECUTABLES:
        return None
    return str(path)


def _registry_protection(record: dict[str, object]) -> Protection:
    reasons: list[str] = []
    name = str(record.get("name", ""))
    release_type = str(record.get("release_type", "")).casefold()
    publisher = str(record.get("publisher", "")).casefold()
    if bool(record.get("system_component")):
        reasons.append("composant système")
    if bool(record.get("no_remove")):
        reasons.append("désinstallation désactivée")
    if record.get("parent_key_name"):
        reasons.append("composant enfant ou correctif")
    if release_type in UPDATE_RELEASE_TYPES or re.fullmatch(r"KB\d+", name, re.IGNORECASE):
        reasons.append("mise à jour ou correctif")
    if "microsoft" in publisher and any(word in name.casefold() for word in ("driver", "runtime", "redistributable")):
        reasons.append("driver ou runtime partagé")
    return Protection(bool(reasons), sorted(set(reasons)))


def registry_candidates(records: Iterable[dict[str, object]]) -> list[Candidate]:
    candidates: list[Candidate] = []
    for record in records:
        name = str(record.get("name", "")).strip()
        key = str(record.get("key", "")).strip()
        if not name or not key:
            continue
        location = str(record.get("install_location", ""))
        size_value = record.get("size_bytes")
        size = size_value if isinstance(size_value, int) and not isinstance(size_value, bool) else None
        protection = _registry_protection(record)
        uninstall_string = str(record.get("uninstall_string", ""))
        command = (
            CommandSuggestion(uninstall_string, "publisher_unverified")
            if uninstall_string.strip() and not protection.protected
            else None
        )
        hint = _display_icon_executable(str(record.get("display_icon", "")))
        metadata = {
            "hive": str(record.get("hive", "")),
            "view": str(record.get("view", "")),
            "publisher": str(record.get("publisher", "")),
        }
        dedupe = _dedupe_key(name, location)
        if dedupe:
            metadata["dedupe_key"] = dedupe
        candidates.append(
            Candidate(
                app_id=f"registry:{str(record.get('hive', 'HKCU')).casefold()}:{str(record.get('view', '64'))}:{key}",
                source="registry",
                name=name,
                size=SizeEvidence(size, None, "registry_estimated_size", "empreinte déclarée par l'installateur", "medium" if size is not None else "unknown"),
                executable_hints=[hint] if hint else [],
                protection=protection,
                command=command,
                metadata=metadata,
            )
        )
    return candidates


def collect_registry_apps() -> list[Candidate]:
    if sys.platform != "win32":
        raise RuntimeError("registre Uninstall indisponible hors Windows")
    return registry_candidates(scan_uninstall_records())


def _powershell_json(script: str) -> object:
    executable = shutil.which("powershell.exe") or shutil.which("powershell")
    if executable is None:
        raise RuntimeError("PowerShell absent")
    kwargs: dict[str, object] = {}
    if sys.platform == "win32":
        kwargs["creationflags"] = CREATE_NO_WINDOW
    completed = subprocess.run(
        [executable, "-NoProfile", "-NonInteractive", "-Command", script],
        capture_output=True,
        text=True,
        timeout=TOOL_TIMEOUT_SECONDS,
        check=False,
        **kwargs,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or f"PowerShell exit {completed.returncode}")
    if not completed.stdout.strip():
        return []
    return json.loads(completed.stdout)


def _find_windows_executable(location: str, preferred: str) -> str | None:
    root = Path(location)
    if not root.is_dir():
        return None
    preferred_names = {preferred.casefold(), preferred.rsplit(".", 1)[-1].casefold()}
    seen: list[Path] = []
    for current, dirs, files in os.walk(root):
        relative_depth = len(Path(current).relative_to(root).parts)
        if relative_depth >= 2:
            dirs[:] = []
        for filename in files:
            if filename.casefold().endswith(".exe"):
                seen.append(Path(current) / filename)
                if len(seen) >= 200:
                    break
        if len(seen) >= 200:
            break
    matches = [path for path in seen if path.stem.casefold() in preferred_names]
    chosen = matches[0] if len(matches) == 1 else seen[0] if len(seen) == 1 else None
    return str(chosen) if chosen is not None and chosen.name.casefold() not in GENERIC_EXECUTABLES else None


def _powershell_quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def appx_candidates(records: Iterable[dict[str, object]]) -> list[Candidate]:
    candidates: list[Candidate] = []
    for record in records:
        name = str(record.get("Name", "")).strip()
        family = str(record.get("PackageFamilyName", "")).strip() or name
        full_name = str(record.get("PackageFullName", "")).strip()
        if not name or not family or not full_name:
            continue
        reasons: list[str] = []
        if bool(record.get("IsFramework")):
            reasons.append("framework MSIX partagé")
        if bool(record.get("IsResourcePackage")):
            reasons.append("paquet de ressources MSIX")
        if bool(record.get("NonRemovable")):
            reasons.append("paquet MSIX non supprimable")
        protection = Protection(bool(reasons), reasons)
        location = str(record.get("InstallLocation", ""))
        size_value = record.get("SizeBytes")
        if isinstance(size_value, int) and not isinstance(size_value, bool):
            size = size_value
        elif location and Path(location).is_dir():
            size = dir_size_on_disk(location)
        else:
            size = None
        hint = _find_windows_executable(location, name) if not protection.protected else None
        candidates.append(
            Candidate(
                app_id=f"appx:{family}",
                source="appx",
                name=name,
                size=SizeEvidence(size, None, "appx_install_location", "répertoire du paquet hors dépendances", "medium" if size is not None else "unknown"),
                executable_hints=[hint] if hint else [],
                protection=protection,
                command=CommandSuggestion(f"Remove-AppxPackage -Package {_powershell_quote(full_name)}") if not protection.protected else None,
                metadata={"package_full_name": full_name, "signature_kind": str(record.get("SignatureKind", ""))},
            )
        )
    return candidates


def collect_appx_apps() -> list[Candidate]:
    if sys.platform != "win32":
        raise RuntimeError("MSIX/AppX indisponible hors Windows")
    script = (
        "Get-AppxPackage | Select-Object Name,PackageFullName,PackageFamilyName,"
        "InstallLocation,IsFramework,IsResourcePackage,NonRemovable,SignatureKind | ConvertTo-Json -Compress"
    )
    payload = _powershell_json(script)
    records = payload if isinstance(payload, list) else [payload] if isinstance(payload, dict) else []
    return appx_candidates(records)


def _windows_arg(value: str) -> str:
    return subprocess.list2cmdline([value])


def package_manager_candidates(items: Iterable[InventoryItem]) -> list[Candidate]:
    candidates: list[Candidate] = []
    for item in items:
        manager = item.detail.get("manager", "")
        if manager not in {"scoop", "chocolatey"}:
            continue
        source = manager
        command = f"scoop uninstall {_windows_arg(item.name)}" if manager == "scoop" else f"choco uninstall {_windows_arg(item.name)}"
        hint = _find_windows_executable(item.path or "", item.name)
        metadata = {"manager": manager}
        dedupe = _dedupe_key(item.name, item.path or "")
        if dedupe:
            metadata["dedupe_key"] = dedupe
        candidates.append(
            Candidate(
                app_id=f"{source}:{item.name.casefold()}",
                source=source,
                name=item.name,
                size=SizeEvidence(item.size_bytes, None, "manager_directory", "répertoire du gestionnaire", "high" if item.size_bytes is not None else "unknown"),
                executable_hints=[hint] if hint else [],
                command=CommandSuggestion(command),
                metadata=metadata,
            )
        )
    return candidates


def collect_package_manager_apps() -> list[Candidate]:
    if sys.platform != "win32":
        raise RuntimeError("Scoop/Chocolatey indisponibles hors Windows")
    return package_manager_candidates(scan_scoop_choco_windows())


def windows_collectors():
    return {
        "appx": collect_appx_apps,
        "registry": collect_registry_apps,
        "scoop-chocolatey": collect_package_manager_apps,
    }
