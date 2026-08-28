"""Short-lived, state-bound retirement for eligible Ollama owners only."""

from __future__ import annotations

import hashlib
import json
import os
import secrets
import shutil
import time
from dataclasses import asdict
from pathlib import Path
from typing import Callable, Protocol

from scripts.local_ai.ollama_http import normalize_endpoint, request_json

from .models import (
    Artifact,
    CatalogSnapshot,
    MigrationResult,
    RetirementPlan,
    RetirementResult,
    RetirementToken,
)


class RetirementError(RuntimeError):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


class OllamaDeleteBackend(Protocol):
    def delete(self, native_id: str) -> None: ...


class OllamaApiDeleteBackend:
    def __init__(self, env=None):
        self.endpoint = normalize_endpoint(os.environ if env is None else env)

    def delete(self, native_id: str) -> None:
        request_json(
            self.endpoint,
            "DELETE",
            "/api/delete",
            payload={"model": native_id},
        )


def create_retirement_plan(
    *,
    plan_id: str,
    source_artifact_id: str,
    source_native_id: str,
    snapshot: CatalogSnapshot,
    migration_result: MigrationResult,
    migration_plan_digest: str,
    now_iso: str,
) -> RetirementPlan:
    source = _artifact(snapshot, source_artifact_id)
    _assert_eligible(source, source_native_id, snapshot, migration_result)
    same_allocation = [
        artifact
        for artifact in snapshot.artifacts
        if artifact.artifact_id != source.artifact_id
        and source.allocation_id is not None
        and artifact.allocation_id == source.allocation_id
        and artifact.identity.exact_key == source.identity.exact_key
    ]
    logical = source.logical_size or 0
    avoided = logical if same_allocation else 0
    estimated = 0 if same_allocation or source.allocation_id is None else (
        source.allocated_size if source.allocated_size is not None else logical
    )
    return RetirementPlan(
        plan_id=plan_id,
        owner_tool="ollama",
        source_artifact_id=source.artifact_id,
        source_path=source.path,
        source_native_id=source_native_id,
        source_sha256=source.identity.value or "",
        references_digest=_references_digest(source),
        migration_plan_digest=migration_plan_digest,
        logical_bytes=logical,
        avoided_bytes=avoided,
        estimated_reclaimable_bytes=estimated,
        allocation_id=source.allocation_id,
        created_at=now_iso,
    )


class RetirementTokenStore:
    def __init__(self, root: str | Path, *, clock: Callable[[], float] = time.time):
        self.root = Path(root)
        self.clock = clock

    def issue(
        self,
        plan: RetirementPlan,
        snapshot: CatalogSnapshot,
        *,
        ttl_seconds: int = 300,
    ) -> RetirementToken:
        if not 1 <= ttl_seconds <= 600:
            raise RetirementError("retirement-ttl-invalid", "Durée de jeton invalide.")
        source = _artifact(snapshot, plan.source_artifact_id)
        _assert_plan_state(plan, source)
        _assert_source_unblocked(source, plan.source_native_id, snapshot)
        token = secrets.token_urlsafe(24)
        record = RetirementToken(
            token=token,
            plan_digest=_digest(asdict(plan)),
            state_digest=_retirement_state_digest(snapshot, source),
            expires_at=self.clock() + ttl_seconds,
        )
        self.root.mkdir(parents=True, exist_ok=True)
        self._path(token).write_text(
            json.dumps(
                {
                    "plan_digest": record.plan_digest,
                    "state_digest": record.state_digest,
                    "expires_at": record.expires_at,
                },
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        return record

    def confirm(
        self,
        token: str,
        plan: RetirementPlan,
        *,
        fresh_snapshot: CatalogSnapshot,
        backend: OllamaDeleteBackend,
        reinventory: Callable[[], CatalogSnapshot],
        measure_free: Callable[[], int] | None = None,
    ) -> RetirementResult:
        path = self._path(token)
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise RetirementError("retirement-token-invalid", "Jeton inconnu.") from exc
        if self.clock() > payload.get("expires_at", 0):
            raise RetirementError("retirement-token-expired", "Jeton expiré.")
        if payload.get("plan_digest") != _digest(asdict(plan)):
            raise RetirementError("retirement-plan-stale", "Le plan ne correspond plus au jeton.")
        source = _artifact(fresh_snapshot, plan.source_artifact_id)
        _assert_plan_state(plan, source)
        _assert_source_unblocked(source, plan.source_native_id, fresh_snapshot)
        if payload.get("state_digest") != _retirement_state_digest(fresh_snapshot, source):
            raise RetirementError("retirement-state-stale", "L'état source a changé.")
        if plan.owner_tool != "ollama":
            raise RetirementError("retirement-owner-unsupported", "Seul Ollama est supprimable en v1.")
        used = path.with_suffix(".used")
        os.replace(path, used)
        meter = measure_free or _disk_meter(plan.source_path)
        before = meter()
        backend.delete(plan.source_native_id)
        after_snapshot = reinventory()
        if _has_owner_reference(after_snapshot, plan.source_native_id):
            raise RetirementError(
                "retirement-delete-unconfirmed", "Le propriétaire Ollama reste visible après suppression."
            )
        after = meter()
        return RetirementResult(
            plan.plan_id,
            plan.source_native_id,
            plan.logical_bytes,
            plan.avoided_bytes,
            plan.estimated_reclaimable_bytes,
            max(after - before, 0),
        )

    def _path(self, token: str) -> Path:
        return self.root / f"{hashlib.sha256(token.encode()).hexdigest()}.json"


def _assert_eligible(
    source: Artifact,
    native_id: str,
    snapshot: CatalogSnapshot,
    migration: MigrationResult,
) -> None:
    if source.identity.state != "verified" or source.identity.value is None:
        raise RetirementError("retirement-identity-unverified", "Identité source non vérifiée.")
    if source.relationship != "owner_blob":
        raise RetirementError("retirement-owner-unsupported", "La source n'est pas un blob Ollama possédé.")
    _assert_source_unblocked(source, native_id, snapshot)
    if (
        not migration.success
        or not migration.retirement_eligible
        or migration.validation.identity != "passed"
        or not (
            migration.validation.load == "passed"
            or migration.validation.inference == "passed"
        )
    ):
        raise RetirementError(
            "retirement-migration-weak", "La migration destination n'est pas assez validée."
        )


def _assert_source_unblocked(
    source: Artifact, native_id: str, snapshot: CatalogSnapshot
) -> None:
    if source.identity.state != "verified" or source.relationship != "owner_blob":
        raise RetirementError("retirement-state-stale", "La source n'est plus éligible.")
    targets = [
        reference
        for reference in source.references
        if reference.tool == "ollama" and reference.reference_id == native_id and reference.owner
    ]
    if len(targets) != 1 or len(source.references) != 1:
        raise RetirementError("retirement-references-block", "Une autre référence protège la source.")
    if targets[0].loaded or targets[0].workflow:
        raise RetirementError("retirement-source-active", "La source est active ou dans un workflow.")
    allowed_reasons = {"referenced:ollama"}
    if set(source.protection.reasons) - allowed_reasons:
        raise RetirementError("retirement-protected", "Une protection interdit le retrait.")
    exact_key = source.identity.exact_key
    for artifact in snapshot.artifacts:
        if artifact.identity.exact_key != exact_key:
            continue
        for reference in artifact.references:
            if reference.loaded or reference.workflow or reference.tool != "ollama":
                raise RetirementError(
                    "retirement-references-block", "Une référence tierce ou active protège le contenu."
                )
            if artifact.artifact_id != source.artifact_id:
                raise RetirementError(
                    "retirement-references-block", "Un autre propriétaire référence le contenu."
                )


def _assert_plan_state(plan: RetirementPlan, source: Artifact) -> None:
    if (
        source.path != plan.source_path
        or source.identity.value != plan.source_sha256
        or source.allocation_id != plan.allocation_id
        or _references_digest(source) != plan.references_digest
    ):
        raise RetirementError("retirement-state-stale", "La source ne correspond plus au plan.")


def _artifact(snapshot: CatalogSnapshot, artifact_id: str) -> Artifact:
    matches = [artifact for artifact in snapshot.artifacts if artifact.artifact_id == artifact_id]
    if len(matches) != 1:
        raise RetirementError("retirement-source-missing", "Source de retrait introuvable.")
    return matches[0]


def _references_digest(source: Artifact) -> str:
    return _digest([asdict(reference) for reference in source.references])


def _retirement_state_digest(snapshot: CatalogSnapshot, source: Artifact) -> str:
    exact = [
        {
            "artifact_id": artifact.artifact_id,
            "path": artifact.path,
            "identity": asdict(artifact.identity),
            "allocation_id": artifact.allocation_id,
            "logical_size": artifact.logical_size,
            "allocated_size": artifact.allocated_size,
            "references": [asdict(reference) for reference in artifact.references],
            "protection": asdict(artifact.protection),
        }
        for artifact in snapshot.artifacts
        if artifact.identity.exact_key == source.identity.exact_key
    ]
    return _digest(sorted(exact, key=lambda row: (row["artifact_id"], row["path"])))


def _digest(payload) -> str:
    encoded = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def _has_owner_reference(snapshot: CatalogSnapshot, native_id: str) -> bool:
    return any(
        reference.tool == "ollama" and reference.reference_id == native_id
        for artifact in snapshot.artifacts
        for reference in artifact.references
    )


def _disk_meter(source_path: str) -> Callable[[], int]:
    parent = Path(source_path).parent
    return lambda: shutil.disk_usage(parent).free
