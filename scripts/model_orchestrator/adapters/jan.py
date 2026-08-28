"""Read-only Jan data-folder inventory with link/copy evidence."""

from __future__ import annotations

import json
from pathlib import Path

from ..models import (
    AdapterCapabilities,
    Artifact,
    ArtifactIdentity,
    RootEvidence,
    SourceError,
    ToolInstallation,
    ToolReference,
)
from ..paths import file_evidence
from . import (
    AdapterContext,
    AdapterObservation,
    bounded_files,
    command_json,
    executable_version,
)


class JanAdapter:
    name = "jan"

    def inventory(self, context: AdapterContext) -> AdapterObservation:
        executable = context.which("jan")
        roots = self._roots(context)
        installation = ToolInstallation(
            tool=self.name,
            detected=executable is not None or any(path.exists() for path, _, _ in roots),
            version=executable_version(context, executable),
            executable=executable,
            roots=tuple(str(path) for path, _, _ in roots),
            root_evidence=tuple(
                RootEvidence(str(path), source, confidence)
                for path, source, confidence in roots
            ),
            discovery_source="cli+settings+data-folders",
            confidence=(
                "high"
                if any(
                    source.startswith("environment") or source.startswith("settings")
                    for _, source, _ in roots
                )
                else "medium"
            ),
            capabilities=AdapterCapabilities(
                inventory=True, reference=True, symbolic_link=True, copy=True, native_import=True
            ),
        )
        observation = AdapterObservation(installation)
        observation.errors.extend(self._settings_errors(context))
        seen: set[str] = set()
        cli_files: list[Path] = []
        if executable:
            payload, error = command_json(
                context, (executable, "models", "list", "--json"), source=self.name
            )
            if error:
                observation.errors.append(error)
            elif isinstance(payload, list):
                for row in payload:
                    raw = row.get("path") if isinstance(row, dict) else None
                    if isinstance(raw, str) and raw:
                        cli_files.append(Path(raw))
            else:
                observation.errors.append(
                    SourceError(self.name, "cli-payload-invalid", "Liste de modèles absente")
                )
        for root, _, _ in roots:
            if not root.is_dir():
                continue
            files, error = bounded_files(root, {".gguf"})
            if error:
                observation.errors.append(SourceError(self.name, error.code, error.message))
            cli_files.extend(files)
        for path in cli_files:
            resolved = str(path.absolute())
            if resolved in seen:
                continue
            seen.add(resolved)
            try:
                evidence = file_evidence(path)
                stat = path.stat()
            except OSError as exc:
                observation.errors.append(
                    SourceError(self.name, "file-inaccessible", str(exc))
                )
                continue
            observation.artifacts.append(
                Artifact(
                    artifact_id=f"jan:{resolved}",
                    path=resolved,
                    family="llm",
                    format="gguf",
                    identity=ArtifactIdentity("unknown", source="jan-file"),
                    logical_size=stat.st_size,
                    allocated_size=evidence.allocated_size,
                    relationship=evidence.relationship,
                    allocation_id=evidence.allocation_id,
                    references=[ToolReference(self.name, path.stem, owner=True)],
                    metadata={"import_mode": evidence.relationship},
                )
            )
        return observation

    def _roots(self, context: AdapterContext) -> list[tuple[Path, str, str]]:
        override = context.env.get("JAN_DATA_FOLDER", "").strip()
        if override:
            return [(Path(override), "environment:JAN_DATA_FOLDER", "high")]
        configured = self._settings_root(context)
        if configured is not None:
            return [(configured, "settings:data_folder", "high")]
        if context.platform_name == "windows":
            appdata = Path(context.env.get("APPDATA", str(context.home / "AppData/Roaming")))
            return [(appdata / "Jan" / "data", "documented-default", "medium")]
        return [
            (context.home / ".local/share/Jan/data", "documented-default-current", "medium"),
            (context.home / ".config/Jan/data", "documented-default-legacy", "low"),
        ]

    @staticmethod
    def _settings_root(context: AdapterContext) -> Path | None:
        for settings in JanAdapter._settings_candidates(context):
            try:
                payload = json.loads(settings.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            for key in ("data_folder", "dataFolder", "dataPath"):
                value = payload.get(key) if isinstance(payload, dict) else None
                if isinstance(value, str) and value.strip():
                    return Path(value)
        return None

    @staticmethod
    def _settings_candidates(context: AdapterContext) -> list[Path]:
        configured = context.env.get("JAN_SETTINGS", "").strip()
        if configured:
            return [Path(configured)]
        return [
            context.home / ".config/Jan/settings.json",
            context.home / ".local/share/Jan/settings.json",
        ]

    def _settings_errors(self, context: AdapterContext) -> list[SourceError]:
        errors: list[SourceError] = []
        for settings in self._settings_candidates(context):
            if not settings.exists():
                continue
            try:
                payload = json.loads(settings.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as exc:
                errors.append(
                    SourceError(self.name, "settings-invalid", f"{settings}: {exc}")
                )
                continue
            if not isinstance(payload, dict):
                errors.append(
                    SourceError(
                        self.name, "settings-invalid", f"{settings}: objet attendu"
                    )
                )
        return errors
