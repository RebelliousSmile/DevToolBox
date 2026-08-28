"""Inventory adapter protocol, safe helpers, and built-in registry."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Mapping, Protocol, Sequence

from ..models import Artifact, SourceError, ToolInstallation

MAX_FILES_PER_ROOT = 10_000
JsonRequest = Callable[[str], Any]
Runner = Callable[[Sequence[str]], subprocess.CompletedProcess[str]]


def _run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(command), capture_output=True, text=True, timeout=5, check=False
    )


@dataclass(frozen=True)
class AdapterContext:
    platform_name: str = "windows" if sys.platform == "win32" else "linux"
    env: Mapping[str, str] = field(default_factory=lambda: dict(os.environ))
    home: Path = field(default_factory=Path.home)
    which: Callable[[str], str | None] = shutil.which
    run: Runner = _run
    ollama_opener: Callable[..., Any] | None = None
    comfy_request: JsonRequest | None = None


@dataclass
class AdapterObservation:
    installation: ToolInstallation
    artifacts: list[Artifact] = field(default_factory=list)
    errors: list[SourceError] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)


class InventoryAdapter(Protocol):
    name: str

    def inventory(self, context: AdapterContext) -> AdapterObservation: ...


def command_json(
    context: AdapterContext, command: Sequence[str], *, source: str
) -> tuple[Any | None, SourceError | None]:
    try:
        result = context.run(command)
    except (OSError, subprocess.SubprocessError) as exc:
        return None, SourceError(source, "command-failed", str(exc))
    if result.returncode != 0:
        detail = result.stderr.strip() or f"exit {result.returncode}"
        return None, SourceError(source, "command-failed", detail)
    try:
        return json.loads(result.stdout), None
    except json.JSONDecodeError:
        return None, SourceError(source, "command-payload-invalid", "JSON invalide")


def executable_version(context: AdapterContext, executable: str | None) -> str | None:
    if executable is None:
        return None
    try:
        result = context.run((executable, "--version"))
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    rendered = result.stdout.strip() or result.stderr.strip()
    lines = [line.strip() for line in rendered.splitlines() if line.strip()]
    for line in reversed(lines):
        if any(character.isdigit() for character in line):
            return line
    return None


def bounded_files(root: Path, suffixes: set[str]) -> tuple[list[Path], SourceError | None]:
    found: list[Path] = []
    try:
        for candidate in root.rglob("*"):
            if not candidate.is_file() or candidate.suffix.lower() not in suffixes:
                continue
            found.append(candidate)
            if len(found) >= MAX_FILES_PER_ROOT:
                return found, SourceError(
                    str(root), "traversal-truncated", f"Limite de {MAX_FILES_PER_ROOT} fichiers"
                )
    except OSError as exc:
        return found, SourceError(str(root), "root-inaccessible", str(exc))
    return found, None


def builtin_adapters() -> tuple[InventoryAdapter, ...]:
    from .comfyui import ComfyUIAdapter
    from .jan import JanAdapter
    from .lm_studio import LMStudioAdapter
    from .ollama import OllamaAdapter

    return (OllamaAdapter(), JanAdapter(), LMStudioAdapter(), ComfyUIAdapter())
