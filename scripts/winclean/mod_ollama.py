"""Adaptateur Ollama local, strict et sans accès direct au stockage des modèles."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Mapping, Sequence

from scripts.local_ai.ollama_http import (
    REQUEST_TIMEOUT_SECONDS,
    OllamaHttpError,
    normalize_endpoint,
    request_json,
)

from scripts.winclean.common import (
    SKIP_GONE,
    SKIP_RUNNING,
    SKIP_UNATTEMPTED,
    CleanCandidate,
    CleanResult,
    CompletedResource,
    Level,
    ModuleDiscoveryError,
    OperationFailure,
    SkippedEntry,
    sum_known,
)

MODULE_NAME = "ollama-models"
_Opener = Callable[..., Any]


@dataclass(frozen=True)
class ModelInfo:
    name: str
    size: int


def normalise_endpoint(env: Mapping[str, str] | None = None) -> str:
    """Rend une origine HTTP loopback canonique ou refuse avant tout réseau."""
    try:
        return normalize_endpoint(env)
    except OllamaHttpError as exc:
        raise _translate_http_error(exc) from exc


def _request_json(
    endpoint: str,
    method: str,
    path: str,
    *,
    payload: Mapping[str, str] | None = None,
    opener: _Opener | None = None,
) -> Any:
    try:
        return request_json(endpoint, method, path, payload=payload, opener=opener)
    except OllamaHttpError as exc:
        raise _translate_http_error(exc) from exc


def _translate_http_error(error: OllamaHttpError) -> ModuleDiscoveryError:
    if error.code == "ollama-endpoint-unsafe":
        message = "Seul HTTP vers une adresse locale est autorisé."
    elif error.code == "ollama-endpoint-remote":
        message = "L'adresse Ollama doit désigner localhost, 127.0.0.1 ou ::1."
    elif error.code == "ollama-http-error":
        message = f"Ollama a répondu avec le statut HTTP {error.status}."
    elif error.code == "ollama-transport-error":
        message = f"Impossible de joindre Ollama localement : {error.detail}."
    elif error.code == "ollama-payload-invalid":
        message = "Ollama a renvoyé une réponse JSON invalide."
    else:
        endpoint_messages = {
            "empty-host": "OLLAMA_HOST est vide.",
            "credentials-or-origin-invalid": "L'adresse Ollama ne doit contenir aucun identifiant.",
            "origin-has-components": "L'adresse Ollama doit être une origine sans chemin, requête ni fragment.",
            "missing-port": "Le port Ollama est manquant.",
            "invalid-port": "Le port Ollama est invalide.",
        }
        message = endpoint_messages.get(error.detail, f"Adresse Ollama invalide : {error.detail}.")
    return ModuleDiscoveryError(error.code, message)


def _model_rows(payload: Any, *, require_size: bool) -> dict[str, ModelInfo]:
    if not isinstance(payload, dict) or not isinstance(payload.get("models"), list):
        raise ModuleDiscoveryError(
            "ollama-payload-invalid", "La réponse Ollama ne contient pas une liste de modèles."
        )
    models: dict[str, ModelInfo] = {}
    for row in payload["models"]:
        if (
            not isinstance(row, dict)
            or not isinstance(row.get("name"), str)
            or not row["name"].strip()
        ):
            raise ModuleDiscoveryError(
                "ollama-payload-invalid", "Un modèle Ollama n'a pas de nom valide."
            )
        name = row["name"]
        if name in models:
            raise ModuleDiscoveryError(
                "ollama-payload-duplicate", f"Ollama a renvoyé deux fois le modèle {name}."
            )
        if require_size and "size" not in row:
            raise ModuleDiscoveryError(
                "ollama-payload-invalid", f"La taille du modèle {name} est absente."
            )
        size = row.get("size", 0)
        if require_size and (isinstance(size, bool) or not isinstance(size, int) or size < 0):
            raise ModuleDiscoveryError(
                "ollama-payload-invalid", f"La taille du modèle {name} est invalide."
            )
        models[name] = ModelInfo(name=name, size=size if require_size else 0)
    return models


def _state(endpoint: str, opener: _Opener | None) -> tuple[dict[str, ModelInfo], set[str]]:
    installed = _model_rows(
        _request_json(endpoint, "GET", "/api/tags", opener=opener), require_size=True
    )
    running = set(
        _model_rows(
            _request_json(endpoint, "GET", "/api/ps", opener=opener), require_size=True
        )
    )
    return installed, running


def discover_ollama_models(
    *,
    requested_models: Sequence[str],
    env: Mapping[str, str] | None = None,
    opener: _Opener | None = None,
    **_kwargs: Any,
) -> list[CleanCandidate]:
    """Découvre uniquement les noms exacts explicitement demandés."""
    endpoint = normalise_endpoint(env)
    unique = tuple(dict.fromkeys(requested_models))
    installed, running = _state(endpoint, opener)
    missing = [name for name in unique if name not in installed]
    if missing:
        raise ModuleDiscoveryError(
            "ollama-model-missing", "Modèle(s) Ollama absent(s) : " + ", ".join(missing) + "."
        )
    active = [name for name in unique if name in running]
    if active:
        raise ModuleDiscoveryError(
            "ollama-model-running",
            "Arrêtez d'abord le(s) modèle(s) Ollama actif(s) : " + ", ".join(active) + ".",
        )
    return [
        CleanCandidate(
            module=MODULE_NAME,
            path=None,
            label=f"modèle Ollama {name}",
            estimated_bytes=installed[name].size,
            level=Level.AGGRESSIVE,
            reason="suppression déléguée à l'API Ollama locale",
            no_undo=True,
            needs_network=True,
            resource_id=name,
        )
        for name in unique
    ]


def clean_ollama_models(
    *,
    candidates: Sequence[CleanCandidate],
    env: Mapping[str, str] | None = None,
    opener: _Opener | None = None,
    **_kwargs: Any,
) -> CleanResult:
    """Revalide puis supprime chaque modèle, en s'arrêtant au premier échec API."""
    result = CleanResult(
        module=MODULE_NAME,
        estimated=sum_known(candidate.estimated_bytes for candidate in candidates),
    )
    try:
        endpoint = normalise_endpoint(env)
    except ModuleDiscoveryError as exc:
        if candidates:
            _record_failure_and_remainder(result, candidates, 0, exc)
        return result

    for index, candidate in enumerate(candidates):
        resource_id = candidate.resource_id
        if not resource_id:
            error = ModuleDiscoveryError(
                "ollama-resource-invalid", "Le candidat Ollama n'a pas d'identifiant de modèle."
            )
            _record_failure_and_remainder(result, candidates, index, error)
            break
        try:
            installed, running = _state(endpoint, opener)
        except ModuleDiscoveryError as exc:
            _record_failure_and_remainder(result, candidates, index, exc)
            break
        if resource_id not in installed:
            result.skipped.append(
                SkippedEntry(candidate.label, None, SKIP_GONE, "modèle déjà absent d'Ollama")
            )
            continue
        if resource_id in running:
            result.skipped.append(
                SkippedEntry(candidate.label, None, SKIP_RUNNING, "modèle actif dans Ollama")
            )
            continue
        try:
            _request_json(
                endpoint,
                "DELETE",
                "/api/delete",
                payload={"model": resource_id},
                opener=opener,
            )
        except ModuleDiscoveryError as exc:
            _record_failure_and_remainder(result, candidates, index, exc)
            break
        result.completed_resources.append(CompletedResource(resource_id))
    return result


def _record_failure_and_remainder(
    result: CleanResult,
    candidates: Sequence[CleanCandidate],
    index: int,
    error: ModuleDiscoveryError,
) -> None:
    current = candidates[index]
    resource_id = current.resource_id or current.label
    result.operation_failures.append(OperationFailure(resource_id, error.code, str(error)))
    for candidate in candidates[index + 1 :]:
        result.skipped.append(
            SkippedEntry(
                candidate.label,
                None,
                SKIP_UNATTEMPTED,
                "non tenté après l'échec d'une opération Ollama précédente",
            )
        )
