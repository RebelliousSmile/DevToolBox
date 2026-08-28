"""Fast native Ollama pull with manifest-verified, non-fragile GGUF export."""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path
from typing import Any, Callable, Mapping

from scripts.local_ai.ollama_http import (
    OllamaHttpError,
    normalize_endpoint,
    open_stream,
)

from ..events import EventStream
from ..library import LibraryError, NeutralLibrary
from ..models import (
    AcquisitionOffer,
    AcquisitionRequest,
    ArtifactIdentity,
    LibraryRecord,
    ProviderStatus,
)
from .direct import ProviderError

_MODEL = re.compile(
    r"(?:(?P<namespace>[A-Za-z0-9_.-]+)/)?(?P<model>[A-Za-z0-9_.-]+):(?P<tag>[A-Za-z0-9_.-]+)\Z"
)


class OllamaProvider:
    name = "ollama"

    def __init__(
        self,
        *,
        env: Mapping[str, str] | None = None,
        home: Path | None = None,
        platform_name: str | None = None,
        opener: Callable[..., Any] | None = None,
        cancelled: Callable[[], bool] | None = None,
    ):
        self._env = dict(os.environ if env is None else env)
        self._home = Path.home() if home is None else home
        self._platform = platform_name or ("windows" if sys.platform == "win32" else "linux")
        self._opener = opener
        self._cancelled = cancelled or (lambda: False)

    def accepts(self, locator: str) -> bool:
        return locator.startswith("ollama://")

    def status(self) -> ProviderStatus:
        try:
            normalize_endpoint(self._env)
        except OllamaHttpError:
            return ProviderStatus(
                self.name, "error", guidance="Configurez une origine Ollama HTTP loopback."
            )
        if not self._layout_supported(self._root()):
            return ProviderStatus(
                self.name,
                "unavailable",
                guidance="Le store Ollama reconnu (manifests/blobs) est introuvable.",
            )
        return ProviderStatus(self.name, "available", authenticated=None)

    def resolve(self, request: AcquisitionRequest, locator: str) -> AcquisitionOffer:
        model = parse_locator(locator)
        supported = self._layout_supported(self._root())
        try:
            normalize_endpoint(self._env)
        except OllamaHttpError:
            supported = False
        filename = model.replace("/", "-").replace(":", "-") + ".gguf"
        return AcquisitionOffer(
            provider=self.name,
            locator=locator,
            family=request.family,
            immutable_revision=None,
            filename=filename,
            format="gguf",
            trusted_digest=None,
            executable=supported,
            network_bytes=None,
            local_copy_bytes=None,
            temporary_bytes=None,
            resume_supported=True,
            identity_evidence="ollama-manifest-layer-sha256",
            owner_tool="ollama",
            export_method="hard-link-or-copy" if supported else None,
            retirement_supported=False,
        )

    def download(
        self,
        offer: AcquisitionOffer,
        *,
        operation_id: str,
        library: NeutralLibrary,
        events: EventStream,
    ) -> LibraryRecord:
        model = parse_locator(offer.locator)
        root = self._root()
        if not self._layout_supported(root):
            raise ProviderError(
                "ollama-layout-unsupported",
                "Le layout Ollama est inconnu; aucun pull ni export n'a été tenté.",
            )
        journal = library.begin(operation_id, offer.filename)
        try:
            endpoint = normalize_endpoint(self._env)
            response = open_stream(
                endpoint,
                "POST",
                "/api/pull",
                payload={"model": model, "stream": True},
                opener=self._opener,
                timeout=30.0,
            )
            completed = 0
            total: int | None = None
            with response:
                while True:
                    if self._cancelled():
                        raise ProviderError("download-cancelled", "Pull Ollama annulé.")
                    raw = response.readline()
                    if not raw:
                        break
                    try:
                        row = json.loads(raw.decode("utf-8"))
                    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                        raise ProviderError(
                            "ollama-pull-payload-invalid", "Progression Ollama invalide."
                        ) from exc
                    if not isinstance(row, dict):
                        raise ProviderError(
                            "ollama-pull-payload-invalid", "Progression Ollama invalide."
                        )
                    if isinstance(row.get("error"), str):
                        raise ProviderError("ollama-pull-failed", "Ollama a refusé le pull exact.")
                    row_completed = row.get("completed")
                    row_total = row.get("total")
                    if isinstance(row_completed, int) and not isinstance(row_completed, bool):
                        completed = max(completed, row_completed)
                    if isinstance(row_total, int) and not isinstance(row_total, bool):
                        total = max(total or 0, row_total)
                    if completed or total is not None:
                        events.progress(completed, max(total or completed, completed))
            blob, digest = resolve_model_blob(root, model)
            identity = ArtifactIdentity(
                "verified", "sha256", digest, "ollama-manifest-layer-sha256"
            )
            return library.stage_file(
                journal,
                blob,
                family=offer.family,
                identity=identity,
                origin=offer.locator,
                revision=digest,
                format_name="gguf",
            )
        except OllamaHttpError as exc:
            error = ProviderError(exc.code, "Le transport Ollama local a échoué.")
            library.update_journal(journal, state="resumable", error=error.message)
            raise error from exc
        except ProviderError as exc:
            state = "resumable" if exc.code in {"download-cancelled", "ollama-pull-failed"} else "manual-attention"
            library.update_journal(journal, state=state, error=exc.message)
            raise
        except (LibraryError, OSError) as exc:
            error = ProviderError(
                "ollama-export-invalid", "Le blob Ollama reconnu n'a pas pu être exporté."
            )
            library.update_journal(journal, state="manual-attention", error=error.message)
            raise error from exc

    def _root(self) -> Path:
        override = self._env.get("OLLAMA_MODELS", "").strip()
        if override:
            return Path(override)
        if self._platform == "windows":
            return self._home / ".ollama/models"
        return Path("/usr/share/ollama/.ollama/models")

    @staticmethod
    def _layout_supported(root: Path) -> bool:
        return (root / "manifests").is_dir() and (root / "blobs").is_dir()


def parse_locator(locator: str) -> str:
    raw = locator.removeprefix("ollama://") if locator.startswith("ollama://") else ""
    if _MODEL.fullmatch(raw) is None:
        raise ProviderError(
            "ollama-locator-invalid", "Le locator Ollama doit nommer exactement modèle et tag."
        )
    return raw


def resolve_model_blob(root: Path, model: str) -> tuple[Path, str]:
    match = _MODEL.fullmatch(model)
    if match is None:
        raise ProviderError("ollama-locator-invalid", "Modèle Ollama exact invalide.")
    namespace = match.group("namespace") or "library"
    manifest = (
        root
        / "manifests/registry.ollama.ai"
        / namespace
        / match.group("model")
        / match.group("tag")
    )
    try:
        if manifest.is_symlink():
            raise ValueError("manifeste lié symboliquement")
        payload = json.loads(manifest.read_text(encoding="utf-8"))
        layers = payload.get("layers")
        if not isinstance(layers, list):
            raise ValueError("layers absent")
        candidates = [
            layer
            for layer in layers
            if isinstance(layer, dict)
            and layer.get("mediaType") == "application/vnd.ollama.image.model"
        ]
        if len(candidates) != 1:
            raise ValueError("couche modèle ambiguë")
        digest_value = candidates[0].get("digest")
        if not isinstance(digest_value, str) or not digest_value.startswith("sha256:"):
            raise ValueError("digest absent")
        digest = digest_value.removeprefix("sha256:").lower()
        if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
            raise ValueError("digest invalide")
        blob = root / "blobs" / f"sha256-{digest}"
        if not blob.is_file() or blob.is_symlink():
            raise ValueError("blob absent ou lié symboliquement")
        try:
            blob.resolve().relative_to(root.resolve())
        except ValueError as exc:
            raise ValueError("blob hors du store possédé") from exc
        return blob, digest
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        raise ProviderError(
            "ollama-export-evidence-unknown",
            "Le manifeste ou blob Ollama exact n'est pas reconnu; export désactivé.",
        ) from exc
