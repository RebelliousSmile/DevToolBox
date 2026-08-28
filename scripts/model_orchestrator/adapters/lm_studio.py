"""LM Studio inventory preferring the supported ``lms`` CLI."""

from __future__ import annotations

from pathlib import Path
from typing import Any

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
from . import AdapterContext, AdapterObservation, bounded_files, command_json, executable_version


class LMStudioAdapter:
    name = "lm-studio"

    def inventory(self, context: AdapterContext) -> AdapterObservation:
        executable = context.which("lms")
        root, source, confidence = self._root(context)
        installation = ToolInstallation(
            tool=self.name,
            detected=executable is not None or root.exists(),
            version=executable_version(context, executable),
            executable=executable,
            roots=(str(root),),
            root_evidence=(RootEvidence(str(root), source, confidence),),
            discovery_source="lms-cli+models-root",
            confidence=confidence,
            capabilities=AdapterCapabilities(
                inventory=True,
                reference=True,
                hard_link=True,
                symbolic_link=True,
                copy=True,
                native_import=True,
                load_validation=True,
            ),
        )
        observation = AdapterObservation(installation)
        loaded: set[str] = set()
        rows: list[dict[str, Any]] = []
        if executable:
            listed, error = command_json(context, (executable, "ls", "--json"), source=self.name)
            if error:
                observation.errors.append(error)
            else:
                rows = self._rows(listed, observation)
            processes, ps_error = command_json(context, (executable, "ps", "--json"), source=self.name)
            if ps_error:
                observation.errors.append(ps_error)
            else:
                loaded = self._loaded_ids(processes)
        paths: dict[str, tuple[Path, str]] = {}
        for row in rows:
            raw_path = row.get("path")
            identifier = str(row.get("modelKey") or row.get("id") or raw_path or "")
            if isinstance(raw_path, str) and raw_path:
                paths[str(Path(raw_path).absolute())] = (Path(raw_path), identifier)
        if not paths and root.is_dir():
            files, error = bounded_files(root, {".gguf", ".safetensors"})
            if error:
                observation.errors.append(SourceError(self.name, error.code, error.message))
            for path in files:
                paths[str(path.absolute())] = (path, path.stem)
        for rendered, (path, identifier) in paths.items():
            try:
                evidence = file_evidence(path)
                stat = path.stat()
            except OSError as exc:
                observation.errors.append(SourceError(self.name, "file-inaccessible", str(exc)))
                continue
            suffix = path.suffix.lower()
            observation.artifacts.append(
                Artifact(
                    artifact_id=f"lm-studio:{rendered}",
                    path=rendered,
                    family="llm",
                    format="gguf" if suffix == ".gguf" else "safetensors",
                    identity=ArtifactIdentity("unknown", source="lms-cli"),
                    logical_size=stat.st_size,
                    allocated_size=evidence.allocated_size,
                    relationship=evidence.relationship,
                    allocation_id=evidence.allocation_id,
                    references=[ToolReference(self.name, identifier, owner=True, loaded=identifier in loaded)],
                )
            )
        return observation

    @staticmethod
    def _root(context: AdapterContext) -> tuple[Path, str, str]:
        override = context.env.get("LM_STUDIO_MODELS_DIR", "").strip()
        if override:
            return Path(override), "environment:LM_STUDIO_MODELS_DIR", "high"
        return context.home / ".lmstudio/models", "documented-default", "medium"

    @staticmethod
    def _rows(payload: Any, observation: AdapterObservation) -> list[dict[str, Any]]:
        if isinstance(payload, list) and all(isinstance(row, dict) for row in payload):
            return payload
        if isinstance(payload, dict):
            for key in ("models", "data"):
                rows = payload.get(key)
                if isinstance(rows, list) and all(isinstance(row, dict) for row in rows):
                    return rows
        observation.errors.append(SourceError("lm-studio", "cli-payload-invalid", "Liste de modèles absente"))
        return []

    @staticmethod
    def _loaded_ids(payload: Any) -> set[str]:
        if isinstance(payload, dict):
            payload = payload.get("models", payload.get("data", []))
        if not isinstance(payload, list):
            return set()
        return {
            str(row.get("modelKey") or row.get("id"))
            for row in payload if isinstance(row, dict) and (row.get("modelKey") or row.get("id"))
        }
