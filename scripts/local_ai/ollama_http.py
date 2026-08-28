"""Strict loopback-only JSON transport for local Ollama callers."""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Callable, Mapping
from urllib.parse import urlsplit

DEFAULT_ENDPOINT = "http://127.0.0.1:11434"
DEFAULT_PORT = 11434
REQUEST_TIMEOUT_SECONDS = 5.0
LOOPBACK_HOSTS = frozenset({"localhost", "127.0.0.1", "::1"})
Opener = Callable[..., Any]


@dataclass(frozen=True)
class OllamaHttpError(Exception):
    """Technical transport failure, intentionally free of caller domain types."""

    code: str
    detail: str
    status: int | None = None

    def __str__(self) -> str:
        return self.detail


class RejectRedirects(urllib.request.HTTPRedirectHandler):
    """Reject every redirect so a validated origin cannot change underneath us."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(req.full_url, code, "redirect-refused", headers, fp)


def build_local_opener():
    """Build an opener that cannot inherit environment proxy configuration."""

    return urllib.request.build_opener(urllib.request.ProxyHandler({}), RejectRedirects())


_LOCAL_OPENER = build_local_opener()


def normalize_endpoint(env: Mapping[str, str] | None = None) -> str:
    """Return a canonical loopback HTTP origin or fail before network access."""

    environment = os.environ if env is None else env
    raw = environment.get("OLLAMA_HOST", DEFAULT_ENDPOINT).strip()
    if not raw:
        raise OllamaHttpError("ollama-endpoint-invalid", "empty-host")
    candidate = raw if "://" in raw else f"http://{raw}"
    try:
        parsed = urlsplit(candidate)
        port = parsed.port
    except ValueError as exc:
        raise OllamaHttpError("ollama-endpoint-invalid", str(exc)) from exc
    if parsed.scheme.lower() != "http":
        raise OllamaHttpError("ollama-endpoint-unsafe", "non-http-scheme")
    if not parsed.netloc or parsed.username is not None or parsed.password is not None:
        raise OllamaHttpError("ollama-endpoint-invalid", "credentials-or-origin-invalid")
    if parsed.path not in ("", "/") or parsed.query or parsed.fragment:
        raise OllamaHttpError("ollama-endpoint-invalid", "origin-has-components")
    if parsed.netloc.endswith(":"):
        raise OllamaHttpError("ollama-endpoint-invalid", "missing-port")
    host = (parsed.hostname or "").lower()
    if host not in LOOPBACK_HOSTS:
        raise OllamaHttpError("ollama-endpoint-remote", host or "missing-host")
    selected_port = DEFAULT_PORT if port is None else port
    if not 1 <= selected_port <= 65535:
        raise OllamaHttpError("ollama-endpoint-invalid", "invalid-port")
    rendered_host = f"[{host}]" if host == "::1" else host
    return f"http://{rendered_host}:{selected_port}"


def request_json(
    endpoint: str,
    method: str,
    path: str,
    *,
    payload: Mapping[str, str] | None = None,
    opener: Opener | None = None,
    timeout: float = REQUEST_TIMEOUT_SECONDS,
) -> Any:
    """Perform one bounded JSON request against a previously validated endpoint."""

    body = None if payload is None else json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        endpoint + path,
        data=body,
        method=method,
        headers={"Content-Type": "application/json"} if body is not None else {},
    )
    try:
        transport = _LOCAL_OPENER.open if opener is None else opener
        with transport(request, timeout=timeout) as response:
            status = getattr(response, "status", None)
            if status is None:
                status = response.getcode()
            raw = response.read()
    except urllib.error.HTTPError as exc:
        raise OllamaHttpError("ollama-http-error", str(exc), status=exc.code) from exc
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        raise OllamaHttpError("ollama-transport-error", str(exc)) from exc
    if status != 200:
        raise OllamaHttpError("ollama-http-error", f"HTTP {status}", status=status)
    if not raw:
        return {}
    try:
        return json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise OllamaHttpError("ollama-payload-invalid", "invalid-json") from exc
