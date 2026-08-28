"""Read-only Ollama catalog and recognized manifest inventory."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from scripts.local_ai.ollama_http import OllamaHttpError, normalize_endpoint, request_json

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
from . import AdapterContext, AdapterObservation, executable_version


class OllamaAdapter:
    name = "ollama"

    def inventory(self, context: AdapterContext) -> AdapterObservation:
        executable = context.which("ollama")
        root, root_source, confidence = self._root(context)
        installation = ToolInstallation(
            tool=self.name,
            detected=executable is not None or root.exists(),
            version=executable_version(context, executable),
            executable=executable,
            roots=(str(root),),
            root_evidence=(RootEvidence(str(root), root_source, confidence),),
            discovery_source="executable+api+models-root",
            confidence=confidence,
            capabilities=AdapterCapabilities(inventory=True, load_validation=True),
        )
        observation = AdapterObservation(installation)
        tags: dict[str, dict[str, Any]] = {}
        loaded: set[str] = set()
        try:
            endpoint = normalize_endpoint(context.env)
            tags = self._rows(
                request_json(endpoint, "GET", "/api/tags", opener=context.ollama_opener)
            )
            loaded = set(
                self._rows(
                    request_json(endpoint, "GET", "/api/ps", opener=context.ollama_opener)
                )
            )
        except OllamaHttpError as exc:
            messages = {
                "ollama-endpoint-invalid": "L'origine Ollama est invalide.",
                "ollama-endpoint-unsafe": "L'origine Ollama doit utiliser HTTP.",
                "ollama-endpoint-remote": "L'origine Ollama doit rester sur la boucle locale.",
                "ollama-http-error": "Ollama a répondu avec une erreur HTTP.",
                "ollama-transport-error": "Ollama local est indisponible.",
                "ollama-payload-invalid": "Ollama a renvoyé un JSON invalide.",
            }
            observation.errors.append(
                SourceError(self.name, exc.code, messages.get(exc.code, "Échec Ollama local."))
            )
        except ValueError as exc:
            observation.errors.append(SourceError(self.name, "ollama-payload-invalid", str(exc)))
        artifacts, errors = self._manifest_artifacts(root, tags, loaded)
        observation.artifacts.extend(artifacts)
        observation.errors.extend(errors)
        return observation

    @staticmethod
    def _root(context: AdapterContext) -> tuple[Path, str, str]:
        override = context.env.get("OLLAMA_MODELS", "").strip()
        if override:
            return Path(override), "environment:OLLAMA_MODELS", "high"
        if context.platform_name == "windows":
            return context.home / ".ollama" / "models", "documented-default", "medium"
        return Path("/usr/share/ollama/.ollama/models"), "documented-default", "medium"

    @staticmethod
    def _rows(payload: Any) -> dict[str, dict[str, Any]]:
        if not isinstance(payload, dict) or not isinstance(payload.get("models"), list):
            raise ValueError("La réponse ne contient pas une liste de modèles")
        rows: dict[str, dict[str, Any]] = {}
        for row in payload["models"]:
            if not isinstance(row, dict) or not isinstance(row.get("name"), str):
                raise ValueError("Un modèle n'a pas de nom valide")
            rows[row["name"]] = row
        return rows

    def _manifest_artifacts(
        self, root: Path, tags: dict[str, dict[str, Any]], loaded: set[str]
    ) -> tuple[list[Artifact], list[SourceError]]:
        artifacts: list[Artifact] = []
        by_digest: dict[str, Artifact] = {}
        errors: list[SourceError] = []
        manifests = root / "manifests"
        if not manifests.is_dir():
            return artifacts, errors
        try:
            candidates = [path for path in manifests.rglob("*") if path.is_file()]
        except OSError as exc:
            return [], [SourceError(self.name, "root-inaccessible", str(exc))]
        for manifest in candidates[:10_000]:
            try:
                payload = json.loads(manifest.read_text(encoding="utf-8"))
                layers = payload.get("layers", [])
                layer = next(
                    row for row in layers
                    if row.get("mediaType") == "application/vnd.ollama.image.model"
                )
                digest = layer["digest"]
                if not isinstance(digest, str) or not digest.startswith("sha256:"):
                    raise ValueError("digest absent")
                sha256 = digest.removeprefix("sha256:").lower()
                blob = root / "blobs" / digest.replace(":", "-")
                if len(sha256) != 64 or not blob.is_file():
                    raise ValueError("blob reconnu absent")
                relative = manifest.relative_to(manifests).parts
                name = self._manifest_name(relative)
                evidence = file_evidence(blob)
                stat = blob.stat()
                tag = tags.get(name, {})
                reference = ToolReference(self.name, name, owner=True, loaded=name in loaded)
                existing = by_digest.get(digest)
                if existing is not None:
                    existing.references.append(reference)
                    continue
                artifact = Artifact(
                        artifact_id=f"ollama:{digest}",
                        path=str(blob.resolve()),
                        family="llm",
                        format="gguf",
                        identity=ArtifactIdentity("verified", "sha256", sha256, "ollama-manifest"),
                        logical_size=stat.st_size,
                        allocated_size=evidence.allocated_size,
                        quantization=self._quantization(tag),
                        relationship="owner_blob",
                        allocation_id=evidence.allocation_id,
                        references=[reference],
                        metadata={"manifest": str(manifest), "root_source": "ollama-model-store"},
                    )
                artifacts.append(artifact)
                by_digest[digest] = artifact
            except (OSError, ValueError, KeyError, StopIteration, json.JSONDecodeError) as exc:
                errors.append(SourceError(self.name, "manifest-unrecognized", f"{manifest}: {exc}"))
        if len(candidates) > 10_000:
            errors.append(SourceError(self.name, "traversal-truncated", "Limite de 10000 manifests"))
        return artifacts, errors

    @staticmethod
    def _manifest_name(parts: tuple[str, ...]) -> str:
        if len(parts) < 2:
            raise ValueError("chemin de manifeste incomplet")
        tag = parts[-1]
        repository_parts = parts[1:-1]
        if repository_parts[:1] == ("library",):
            repository_parts = repository_parts[1:]
        repository = "/".join(repository_parts) or parts[-2]
        return f"{repository}:{tag}"

    @staticmethod
    def _quantization(row: dict[str, Any]) -> str | None:
        details = row.get("details")
        if isinstance(details, dict) and isinstance(details.get("quantization_level"), str):
            return details["quantization_level"]
        return None
