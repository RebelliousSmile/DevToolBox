"""Guarded exact-file HTTPS downloader with validator-bound resume."""

from __future__ import annotations

import hashlib
import json
import os
import re
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable
from urllib.parse import unquote, urljoin, urlsplit

from scripts.local_ai.ollama_http import LOOPBACK_HOSTS

from ..events import EventStream
from ..library import LibraryError, NeutralLibrary, redact_origin
from ..models import (
    AcquisitionOffer,
    AcquisitionRequest,
    ArtifactIdentity,
    LibraryRecord,
    ProviderStatus,
)

TIMEOUT_SECONDS = 30.0
CHUNK_SIZE = 1024 * 1024
MAX_REDIRECTS = 3
_CONTENT_RANGE = re.compile(r"bytes (\d+)-(\d+)/(\d+)\Z")


@dataclass(frozen=True)
class ProviderError(Exception):
    code: str
    message: str

    def __str__(self) -> str:
        return self.message


def validate_direct_url(raw: str) -> str:
    try:
        parsed = urlsplit(raw)
        port = parsed.port
    except ValueError as exc:
        raise ProviderError("direct-url-invalid", "URL directe invalide.") from exc
    if parsed.username is not None or parsed.password is not None:
        raise ProviderError("direct-url-credentials", "Les identifiants dans l'URL sont interdits.")
    host = (parsed.hostname or "").lower()
    if parsed.scheme == "https" and host:
        pass
    elif parsed.scheme == "http" and host in LOOPBACK_HOSTS:
        pass
    else:
        raise ProviderError(
            "direct-url-unsafe", "HTTPS est requis, sauf HTTP explicite sur la boucle locale."
        )
    if parsed.fragment or port is not None and not 1 <= port <= 65535:
        raise ProviderError("direct-url-invalid", "URL directe invalide.")
    filename = Path(unquote(parsed.path)).name
    if not filename:
        raise ProviderError("direct-url-invalid", "Le nom de fichier manque dans l'URL.")
    return raw


class SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    def __init__(self):
        self.count = 0

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        self.count += 1
        if self.count > MAX_REDIRECTS:
            raise urllib.error.HTTPError(req.full_url, code, "too-many-redirects", headers, fp)
        target = urlsplit(urljoin(req.full_url, newurl))
        source = urlsplit(req.full_url)
        if (
            target.scheme != source.scheme
            or target.hostname != source.hostname
            or (target.port or _default_port(target.scheme))
            != (source.port or _default_port(source.scheme))
            or target.username is not None
        ):
            raise urllib.error.HTTPError(req.full_url, code, "unsafe-redirect", headers, fp)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _default_port(scheme: str) -> int | None:
    return 443 if scheme == "https" else 80 if scheme == "http" else None


class DirectProvider:
    name = "direct"

    def __init__(
        self,
        opener: Callable[..., Any] | None = None,
        cancelled: Callable[[], bool] | None = None,
    ):
        self._opener = opener
        self._cancelled = cancelled or (lambda: False)

    def accepts(self, locator: str) -> bool:
        return locator.startswith(("https://", "http://"))

    def status(self) -> ProviderStatus:
        return ProviderStatus(self.name, "available", authenticated=None)

    def resolve(self, request: AcquisitionRequest, locator: str) -> AcquisitionOffer:
        validate_direct_url(locator)
        filename = Path(unquote(urlsplit(locator).path)).name
        digest = _digest(request.user_sha256) if request.user_sha256 else None
        return AcquisitionOffer(
            provider=self.name,
            locator=locator,
            family=request.family,
            immutable_revision=digest,
            filename=filename,
            format=Path(filename).suffix.lower().lstrip(".") or "opaque",
            trusted_digest=digest,
            executable=True,
            network_bytes=None,
            local_copy_bytes=0,
            temporary_bytes=None,
            resume_supported=True,
            identity_evidence="user-sha256" if digest else "stream-sha256",
        )

    def download(
        self,
        offer: AcquisitionOffer,
        *,
        operation_id: str,
        library: NeutralLibrary,
        events: EventStream,
    ) -> LibraryRecord:
        validate_direct_url(offer.locator)
        journal = library.load_journal(operation_id)
        if journal is None:
            journal = library.begin(
                operation_id, offer.filename, expected_digest=offer.trusted_digest
            )
        stage = Path(journal.staging_path)
        metadata_path = stage.parent / "resume.json"
        try:
            record = self._transfer(offer, journal, stage, metadata_path, library, events)
            return record
        except LibraryError as exc:
            error = ProviderError(
                "download-output-invalid", "Le fichier reçu n'est pas un modèle structurellement valide."
            )
            library.update_journal(journal, state="manual-attention", error=error.message)
            raise error from exc
        except ProviderError as exc:
            validator_safe = self._resume_metadata(metadata_path) is not None
            if exc.code in {"download-transport-error", "download-truncated", "download-cancelled"}:
                state = "resumable" if stage.is_file() and validator_safe else "discardable"
            elif exc.code == "download-checksum-mismatch":
                state = "manual-attention"
            else:
                state = "discardable"
            library.update_journal(journal, state=state, error=exc.message)
            raise
        except OSError as exc:
            code = "download-disk-full" if getattr(exc, "errno", None) == 28 else "download-io-error"
            error = ProviderError(code, "Écriture locale impossible.")
            library.update_journal(journal, state="manual-attention", error=error.message)
            raise error from exc

    def _transfer(self, offer, journal, stage, metadata_path, library, events):
        resume = self._resume_metadata(metadata_path)
        offset = stage.stat().st_size if stage.is_file() else 0
        response = None
        append = False
        if offset and resume is not None:
            try:
                response = self._request(
                    offer.locator,
                    headers={"Range": f"bytes={offset}-", "If-Range": resume["validator"]},
                )
                append = self._resume_is_stable(response, resume, offset)
                if not append:
                    response.close()
            except ProviderError as exc:
                if exc.code != "direct-http-status":
                    raise
                response = None
        if response is None or not append:
            offset = 0
            response = self._request(offer.locator, headers={})
            if self._status(response) != 200:
                response.close()
                raise ProviderError("direct-http-status", "Le serveur n'a pas renvoyé le fichier complet.")
        total = self._total(response, offset)
        validator = self._validator(response)
        if validator is not None and total is not None:
            self._write_resume(metadata_path, validator, total)
        elif metadata_path.exists():
            metadata_path.unlink()
        digest = hashlib.sha256()
        if append:
            with stage.open("rb") as existing:
                for chunk in iter(lambda: existing.read(CHUNK_SIZE), b""):
                    digest.update(chunk)
        mode = "ab" if append else "wb"
        transferred = offset
        try:
            with response, stage.open(mode) as stream:
                events.progress(transferred, total)
                while True:
                    if self._cancelled():
                        raise ProviderError("download-cancelled", "Téléchargement annulé.")
                    try:
                        chunk = response.read(CHUNK_SIZE)
                    except (TimeoutError, urllib.error.URLError, OSError) as exc:
                        raise ProviderError("download-transport-error", "Transfert direct interrompu.") from exc
                    if not chunk:
                        break
                    stream.write(chunk)
                    digest.update(chunk)
                    transferred += len(chunk)
                    events.progress(transferred, total)
                stream.flush()
                os.fsync(stream.fileno())
        except urllib.error.HTTPError as exc:
            raise ProviderError("direct-http-status", f"Erreur HTTP {exc.code}.") from exc
        if total is not None and transferred != total:
            raise ProviderError("download-truncated", "Le fichier direct est tronqué.")
        actual = digest.hexdigest()
        if offer.trusted_digest and actual != offer.trusted_digest:
            raise ProviderError("download-checksum-mismatch", "Le SHA-256 direct ne correspond pas.")
        identity = ArtifactIdentity(
            "verified", "sha256", actual,
            "user-sha256" if offer.trusted_digest else "direct-stream-sha256",
        )
        return library.commit_staged(
            journal,
            family=offer.family,
            identity=identity,
            origin=offer.locator,
            revision=offer.immutable_revision,
            format_name=offer.format,
        )

    def _request(self, url: str, *, headers: dict[str, str]):
        request = urllib.request.Request(url, headers=headers)
        try:
            if self._opener is not None:
                return self._opener(request, timeout=TIMEOUT_SECONDS)
            opener = urllib.request.build_opener(
                urllib.request.ProxyHandler({}), SafeRedirectHandler()
            )
            return opener.open(request, timeout=TIMEOUT_SECONDS)
        except urllib.error.HTTPError as exc:
            raise ProviderError("direct-http-status", f"Erreur HTTP {exc.code}.") from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise ProviderError("download-transport-error", "Serveur direct indisponible.") from exc

    @staticmethod
    def _status(response) -> int:
        return getattr(response, "status", None) or response.getcode()

    def _resume_is_stable(self, response, previous: dict[str, Any], offset: int) -> bool:
        if self._status(response) != 206:
            return False
        match = _CONTENT_RANGE.fullmatch(self._header(response, "Content-Range") or "")
        if match is None or int(match.group(1)) != offset:
            return False
        return (
            int(match.group(3)) == previous["total"]
            and self._validator(response) == previous["validator"]
        )

    def _total(self, response, offset: int) -> int | None:
        content_range = self._header(response, "Content-Range")
        match = _CONTENT_RANGE.fullmatch(content_range or "")
        if match:
            return int(match.group(3))
        length = self._header(response, "Content-Length")
        return offset + int(length) if length and length.isdigit() else None

    def _validator(self, response) -> str | None:
        etag = self._header(response, "ETag")
        modified = self._header(response, "Last-Modified")
        return f"etag:{etag}" if etag else f"last-modified:{modified}" if modified else None

    @staticmethod
    def _header(response, name: str) -> str | None:
        headers = getattr(response, "headers", {})
        return headers.get(name) or headers.get(name.lower())

    @staticmethod
    def _write_resume(path: Path, validator: str, total: int) -> None:
        temporary = path.with_suffix(".tmp")
        temporary.write_text(
            json.dumps({"validator": validator, "total": total}, sort_keys=True),
            encoding="utf-8",
        )
        os.replace(temporary, path)

    @staticmethod
    def _resume_metadata(path: Path) -> dict[str, Any] | None:
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
            if (
                isinstance(payload.get("validator"), str)
                and isinstance(payload.get("total"), int)
                and payload["total"] >= 0
            ):
                return payload
        except (OSError, AttributeError, json.JSONDecodeError):
            pass
        return None


def _digest(value: str) -> str:
    normalized = value.removeprefix("sha256:").lower()
    if len(normalized) != 64 or any(character not in "0123456789abcdef" for character in normalized):
        raise ProviderError("checksum-invalid", "Le SHA-256 fourni est invalide.")
    return normalized
