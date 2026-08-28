"""Path normalization and ownership checks for model operations."""

from __future__ import annotations

import ntpath
import os
import posixpath
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping


class PathSafetyError(ValueError):
    """Raised before an unresolved or unsafe path becomes an operation target."""


_WINDOWS_ENV = re.compile(r"%([^%]+)%")
_POSIX_ENV = re.compile(r"\$(?:\{([^}]+)\}|([A-Za-z_][A-Za-z0-9_]*))")


def _expand_windows(raw: str, env: Mapping[str, str]) -> str:
    def replace(match: re.Match[str]) -> str:
        name = match.group(1)
        for key, value in env.items():
            if key.casefold() == name.casefold() and value:
                return value
        raise PathSafetyError(f"variable d'environnement non résolue : %{name}%")

    return _WINDOWS_ENV.sub(replace, raw)


def _expand_posix(raw: str, env: Mapping[str, str]) -> str:
    def replace(match: re.Match[str]) -> str:
        name = match.group(1) or match.group(2)
        value = env.get(name, "")
        if not value:
            raise PathSafetyError(f"variable d'environnement non résolue : ${name}")
        return value

    expanded = _POSIX_ENV.sub(replace, raw)
    if expanded == "~" or expanded.startswith("~/"):
        home = env.get("HOME", "")
        if not home:
            raise PathSafetyError("HOME est requis pour développer '~'")
        expanded = home + expanded[1:]
    return expanded


def normalize_absolute_path(
    raw: str, *, platform_name: str, env: Mapping[str, str] | None = None
) -> str:
    if not isinstance(raw, str) or not raw.strip():
        raise PathSafetyError("le chemin est vide")
    values = os.environ if env is None else env
    if platform_name == "windows":
        expanded = _expand_windows(raw.strip(), values)
        normalized = ntpath.normpath(expanded)
        drive, tail = ntpath.splitdrive(normalized)
        if not drive or not tail.startswith(("\\", "/")):
            raise PathSafetyError("un chemin Windows absolu est requis")
        if "%" in normalized:
            raise PathSafetyError("le chemin contient une variable non résolue")
        return normalized
    if platform_name == "linux":
        expanded = _expand_posix(raw.strip(), values)
        normalized = posixpath.normpath(expanded)
        if not normalized.startswith("/"):
            raise PathSafetyError("un chemin Linux absolu est requis")
        if "$" in normalized:
            raise PathSafetyError("le chemin contient une variable non résolue")
        return normalized
    raise PathSafetyError(f"plateforme non prise en charge : {platform_name}")


def ensure_owned_target(
    target: str,
    *,
    owned_root: str,
    platform_name: str,
    profile_root: str | None = None,
) -> None:
    path_mod = ntpath if platform_name == "windows" else posixpath
    target_norm = path_mod.normcase(path_mod.normpath(target))
    root_norm = path_mod.normcase(path_mod.normpath(owned_root))
    if target_norm == root_norm:
        raise PathSafetyError("la racine possédée ne peut pas être ciblée directement")
    try:
        common = path_mod.commonpath([target_norm, root_norm])
    except ValueError as exc:
        raise PathSafetyError("la cible et la racine ne partagent pas le même volume") from exc
    if common != root_norm:
        raise PathSafetyError("la cible sort de la racine possédée")
    if profile_root is not None:
        profile_norm = path_mod.normcase(path_mod.normpath(profile_root))
        if target_norm == profile_norm:
            raise PathSafetyError("le profil utilisateur ne peut pas être ciblé")


@dataclass(frozen=True)
class FileEvidence:
    relationship: str
    allocation_id: str | None
    allocated_size: int | None


def file_evidence(path: str | Path) -> FileEvidence:
    candidate = Path(path)
    metadata = candidate.lstat()
    if candidate.is_symlink():
        return FileEvidence("symbolic_link", None, 0)
    allocation_id = f"{metadata.st_dev}:{metadata.st_ino}"
    blocks = getattr(metadata, "st_blocks", None)
    allocated = metadata.st_size if blocks is None else int(blocks) * 512
    relationship = "hard_link" if metadata.st_nlink > 1 else "copy"
    return FileEvidence(relationship, allocation_id, allocated)


def same_filesystem(first: str | Path, second: str | Path) -> bool:
    return Path(first).stat().st_dev == Path(second).stat().st_dev
