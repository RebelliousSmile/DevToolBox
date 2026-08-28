"""Read-only Ollama catalog and recognized manifest inventory."""

from __future__ import annotations

import json
import http.client
import os
from pathlib import Path
from typing import Any, Protocol
from urllib.parse import urlsplit

from scripts.local_ai.ollama_http import OllamaHttpError, normalize_endpoint, request_json

from ..models import (
    AdapterCapabilities,
    Artifact,
    ArtifactIdentity,
    RootEvidence,
    SourceError,
    ToolInstallation,
    ToolReference,
    MigrationPlan,
    MigrationStep,
    MigrationValidation,
)
from ..migration import MigrationError
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
            capabilities=AdapterCapabilities(
                inventory=True,
                native_import=True,
                load_validation=True,
                inference_validation=True,
                native_delete=True,
            ),
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


class OllamaMigrationBackend(Protocol):
    def model_exists(self, native_id: str) -> bool: ...
    def blob_exists(self, digest: str) -> bool: ...
    def upload_blob(self, source: str, digest: str) -> None: ...
    def create_model(self, native_id: str, digest: str) -> None: ...
    def model_digest(self, native_id: str) -> str | None: ...
    def infer(self, native_id: str) -> bool: ...
    def delete_model(self, native_id: str) -> None: ...


class OllamaMigrationDriver:
    def __init__(self, backend: OllamaMigrationBackend):
        self.backend = backend

    def planned_steps(self, plan: MigrationPlan) -> list[MigrationStep]:
        return [
            MigrationStep(
                "blob", "ollama-blob", f"sha256:{plan.source_sha256}", None
            ),
            MigrationStep(
                "model", "ollama-model", plan.destination_native_id, "ollama-delete-model"
            ),
        ]

    def execute(self, plan, steps, persist):
        if self.backend.model_exists(plan.destination_native_id):
            raise MigrationError("destination-collision", "Le modèle Ollama existe déjà.")
        blob_step, model_step = steps
        if self.backend.blob_exists(plan.source_sha256):
            blob_step.state = "reused"
            persist()
        else:
            blob_step.state = "executing"
            persist()
            self.backend.upload_blob(plan.source_path, plan.source_sha256)
            blob_step.created_by_operation = True
            blob_step.state = "created"
            persist()
        model_step.state = "executing"
        persist()
        try:
            self.backend.create_model(plan.destination_native_id, plan.source_sha256)
        except Exception:
            if self.backend.model_exists(plan.destination_native_id):
                model_step.created_by_operation = True
                model_step.state = "created"
                persist()
            raise
        model_step.created_by_operation = True
        model_step.state = "created"
        persist()
        digest = self.backend.model_digest(plan.destination_native_id)
        identity = "passed" if digest == plan.source_sha256 else "failed"
        catalog = "passed" if digest is not None else "failed"
        inference = "passed" if identity == "passed" and self.backend.infer(plan.destination_native_id) else "failed"
        return MigrationValidation(
            identity=identity,
            catalog=catalog,
            load="passed" if inference == "passed" else "failed",
            inference=inference,
            destination_digest=digest,
        )

    def rollback(self, plan, steps, persist):
        refused = False
        for step in reversed(steps):
            if not step.created_by_operation or step.state == "rolled-back":
                continue
            if step.kind == "ollama-model":
                try:
                    if self.backend.model_digest(plan.destination_native_id) == plan.source_sha256:
                        self.backend.delete_model(plan.destination_native_id)
                        step.state = "rolled-back"
                    else:
                        step.state = "rollback-refused"
                        refused = True
                except Exception:
                    step.state = "rollback-refused"
                    refused = True
            else:
                # Ollama has no documented standalone blob deletion contract.
                step.state = "retained-native-cache"
            persist()
        if refused:
            raise MigrationError(
                "migration-rollback-incomplete", "Le modèle Ollama a changé avant rollback."
            )


class OllamaApiMigrationBackend:
    """Documented create/blob/generate API, with no direct store mutation."""

    def __init__(self, env=None):
        self.env = dict(os.environ if env is None else env)
        self.endpoint = normalize_endpoint(self.env)
        self._created: dict[str, str] = {}

    def model_exists(self, native_id: str) -> bool:
        try:
            request_json(self.endpoint, "POST", "/api/show", payload={"model": native_id})
            return True
        except OllamaHttpError as exc:
            if exc.code == "ollama-http-error" and exc.status == 404:
                return False
            raise MigrationError("ollama-api-failed", "Impossible d'interroger Ollama.") from exc

    def blob_exists(self, digest: str) -> bool:
        connection, path = self._connection(f"/api/blobs/sha256:{digest}")
        try:
            connection.request("HEAD", path)
            return connection.getresponse().status == 200
        finally:
            connection.close()

    def upload_blob(self, source: str, digest: str) -> None:
        connection, path = self._connection(f"/api/blobs/sha256:{digest}")
        size = Path(source).stat().st_size
        try:
            connection.putrequest("POST", path)
            connection.putheader("Content-Length", str(size))
            connection.endheaders()
            with Path(source).open("rb") as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    connection.send(chunk)
            if connection.getresponse().status not in {200, 201}:
                raise MigrationError("ollama-blob-upload-failed", "Upload blob Ollama refusé.")
        finally:
            connection.close()

    def create_model(self, native_id: str, digest: str) -> None:
        request_json(
            self.endpoint,
            "POST",
            "/api/create",
            payload={
                "model": native_id,
                "files": {"model.gguf": f"sha256:{digest}"},
                "stream": False,
            },
        )
        self._created[native_id] = digest

    def model_digest(self, native_id: str) -> str | None:
        request_json(self.endpoint, "POST", "/api/show", payload={"model": native_id})
        return self._created.get(native_id)

    def infer(self, native_id: str) -> bool:
        payload = request_json(
            self.endpoint,
            "POST",
            "/api/generate",
            payload={"model": native_id, "prompt": "Reply with OK", "stream": False},
        )
        return isinstance(payload, dict) and isinstance(payload.get("response"), str)

    def delete_model(self, native_id: str) -> None:
        request_json(
            self.endpoint, "DELETE", "/api/delete", payload={"model": native_id}
        )
        self._created.pop(native_id, None)

    def _connection(self, path: str):
        parsed = urlsplit(self.endpoint)
        connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=30)
        return connection, path
