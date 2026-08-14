"""Adaptateur Ollama local, strict et sans accès direct au stockage des modèles."""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Callable, Mapping, Sequence
from urllib.parse import urlsplit

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

DEFAULT_ENDPOINT = "http://127.0.0.1:11434"
MODULE_NAME = "ollama-models"
DEFAULT_PORT = 11434
REQUEST_TIMEOUT_SECONDS = 5.0
_LOOPBACK_HOSTS = frozenset({"localhost", "127.0.0.1", "::1"})
_Opener = Callable[..., Any]


class _RejectRedirects(urllib.request.HTTPRedirectHandler):
    """Interdit qu'une origine loopback redirige la requête hors de la machine."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(
            req.full_url, code, "Redirection Ollama interdite.", headers, fp
        )


def _build_local_opener():
    return urllib.request.build_opener(
        urllib.request.ProxyHandler({}),
        _RejectRedirects(),
    )


_LOCAL_OPENER = _build_local_opener()


def _local_urlopen(request, *, timeout):
    return _LOCAL_OPENER.open(request, timeout=timeout)


@dataclass(frozen=True)
class ModelInfo:
    name: str
    size: int


def normalise_endpoint(env: Mapping[str, str] | None = None) -> str:
    """Rend une origine HTTP loopback canonique ou refuse avant tout réseau."""
    environment = os.environ if env is None else env
    raw = environment.get("OLLAMA_HOST", DEFAULT_ENDPOINT).strip()
    if not raw:
        raise ModuleDiscoveryError("ollama-endpoint-invalid", "OLLAMA_HOST est vide.")
    candidate = raw if "://" in raw else f"http://{raw}"
    try:
        parsed = urlsplit(candidate)
        port = parsed.port
    except ValueError as exc:
        raise ModuleDiscoveryError(
            "ollama-endpoint-invalid", f"Adresse Ollama invalide : {exc}."
        ) from exc
    if parsed.scheme.lower() != "http":
        raise ModuleDiscoveryError(
            "ollama-endpoint-unsafe", "Seul HTTP vers une adresse locale est autorisé."
        )
    if not parsed.netloc or parsed.username is not None or parsed.password is not None:
        raise ModuleDiscoveryError(
            "ollama-endpoint-invalid", "L'adresse Ollama ne doit contenir aucun identifiant."
        )
    if parsed.path not in ("", "/") or parsed.query or parsed.fragment:
        raise ModuleDiscoveryError(
            "ollama-endpoint-invalid",
            "L'adresse Ollama doit être une origine sans chemin, requête ni fragment.",
        )
    if parsed.netloc.endswith(":"):
        raise ModuleDiscoveryError("ollama-endpoint-invalid", "Le port Ollama est manquant.")
    host = (parsed.hostname or "").lower()
    if host not in _LOOPBACK_HOSTS:
        raise ModuleDiscoveryError(
            "ollama-endpoint-remote",
            "L'adresse Ollama doit désigner localhost, 127.0.0.1 ou ::1.",
        )
    selected_port = DEFAULT_PORT if port is None else port
    if not 1 <= selected_port <= 65535:
        raise ModuleDiscoveryError("ollama-endpoint-invalid", "Le port Ollama est invalide.")
    rendered_host = f"[{host}]" if host == "::1" else host
    return f"http://{rendered_host}:{selected_port}"


def _request_json(
    endpoint: str,
    method: str,
    path: str,
    *,
    payload: Mapping[str, str] | None = None,
    opener: _Opener | None = None,
) -> Any:
    body = None if payload is None else json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        endpoint + path,
        data=body,
        method=method,
        headers={"Content-Type": "application/json"} if body is not None else {},
    )
    try:
        transport = _local_urlopen if opener is None else opener
        with transport(request, timeout=REQUEST_TIMEOUT_SECONDS) as response:
            status = getattr(response, "status", None)
            if status is None:
                status = response.getcode()
            raw = response.read()
    except urllib.error.HTTPError as exc:
        raise ModuleDiscoveryError(
            "ollama-http-error", f"Ollama a répondu avec le statut HTTP {exc.code}."
        ) from exc
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        raise ModuleDiscoveryError(
            "ollama-transport-error", f"Impossible de joindre Ollama localement : {exc}."
        ) from exc
    if status != 200:
        raise ModuleDiscoveryError(
            "ollama-http-error", f"Ollama a répondu avec le statut HTTP {status}."
        )
    if not raw:
        return {}
    try:
        return json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ModuleDiscoveryError(
            "ollama-payload-invalid", "Ollama a renvoyé une réponse JSON invalide."
        ) from exc


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
