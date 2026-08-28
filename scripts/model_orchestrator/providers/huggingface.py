"""Exact immutable Hugging Face acquisition through the provider-owned ``hf`` CLI."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
from pathlib import Path, PurePosixPath
from typing import Any, Callable, Mapping, Sequence

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

_HF_LOCATOR = re.compile(
    r"hf://(?P<repo>[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)@(?P<revision>[0-9a-fA-F]{40})/(?P<filename>.+)\Z"
)
Runner = Callable[[Sequence[str], Mapping[str, str]], subprocess.CompletedProcess[str]]
MetadataResolver = Callable[[str, str, str], Mapping[str, Any]]


def _run(command: Sequence[str], env: Mapping[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(command), capture_output=True, text=True, timeout=3600, check=False, env=dict(env)
    )


class HuggingFaceProvider:
    name = "huggingface"

    def __init__(
        self,
        *,
        which: Callable[[str], str | None] = shutil.which,
        runner: Runner = _run,
        metadata_resolver: MetadataResolver | None = None,
        env: Mapping[str, str] | None = None,
        high_performance: bool = True,
        cancelled: Callable[[], bool] | None = None,
    ):
        self._which = which
        self._runner = runner
        self._metadata_resolver = metadata_resolver
        self._env = dict(os.environ if env is None else env)
        self._high_performance = high_performance
        self._cancelled = cancelled or (lambda: False)

    def accepts(self, locator: str) -> bool:
        return locator.startswith("hf://")

    def status(self) -> ProviderStatus:
        executable = self._which("hf")
        if executable is None:
            return ProviderStatus(
                self.name,
                "unavailable",
                guidance="Installez huggingface_hub[cli] puis authentifiez hf si nécessaire.",
            )
        version_result = self._runner((executable, "--version"), self._env)
        version = _first_version(version_result.stdout, version_result.stderr)
        auth = self._runner((executable, "auth", "whoami"), self._env)
        return ProviderStatus(
            self.name,
            "available",
            version=version,
            authenticated=auth.returncode == 0,
            guidance=None if auth.returncode == 0 else "Utilisez `hf auth login` pour les dépôts privés.",
        )

    def resolve(self, request: AcquisitionRequest, locator: str) -> AcquisitionOffer:
        repo, revision, filename = parse_locator(locator)
        metadata: Mapping[str, Any] = {}
        if self._metadata_resolver is not None:
            metadata = self._metadata_resolver(repo, revision, filename)
        digest = metadata.get("sha256")
        if digest is not None:
            digest = _sha256(str(digest))
        size = metadata.get("size")
        if isinstance(size, bool) or not isinstance(size, int) or size < 0:
            size = None
        available = self._which("hf") is not None
        return AcquisitionOffer(
            provider=self.name,
            locator=locator,
            family=request.family,
            immutable_revision=revision,
            filename=filename,
            format=Path(filename).suffix.lower().lstrip(".") or "opaque",
            trusted_digest=digest,
            executable=available,
            conversion_required=False,
            network_bytes=size,
            local_copy_bytes=0,
            temporary_bytes=size,
            resume_supported=True,
            identity_evidence="hf-lfs-xet-sha256" if digest else "provider-metadata-unverified",
        )

    def download(
        self,
        offer: AcquisitionOffer,
        *,
        operation_id: str,
        library: NeutralLibrary,
        events: EventStream,
    ) -> LibraryRecord:
        executable = self._which("hf")
        if executable is None:
            raise ProviderError(
                "provider-unavailable", "Le CLI hf est absent; aucune installation automatique n'a été tentée."
            )
        repo, revision, remote_filename = parse_locator(offer.locator)
        local_filename = PurePosixPath(remote_filename).name
        journal = library.begin(
            operation_id, local_filename, expected_digest=offer.trusted_digest
        )
        operation_directory = Path(journal.staging_path).parent
        environment = dict(self._env)
        if self._high_performance:
            environment.setdefault("HF_XET_HIGH_PERFORMANCE", "1")
        if self._cancelled():
            error = ProviderError("download-cancelled", "Téléchargement annulé.")
            library.update_journal(journal, state="discardable", error=error.message)
            raise error
        events.progress(0, offer.network_bytes)
        command = (
            executable,
            "download",
            repo,
            remote_filename,
            "--revision",
            revision,
            "--local-dir",
            str(operation_directory),
        )
        try:
            result = self._runner(command, environment)
        except (OSError, subprocess.SubprocessError) as exc:
            code = "download-disk-full" if getattr(exc, "errno", None) == 28 else "provider-command-failed"
            message = "Espace disque insuffisant." if code == "download-disk-full" else "Le CLI hf n'a pas pu démarrer."
            error = ProviderError(code, message)
            library.update_journal(journal, state="resumable", error=error.message)
            raise error from exc
        if result.returncode != 0:
            error = ProviderError("provider-nonzero-exit", "Le CLI hf a signalé un échec.")
            library.update_journal(journal, state="resumable", error=error.message)
            raise error
        downloaded = operation_directory / PurePosixPath(remote_filename)
        if not downloaded.is_file():
            error = ProviderError("provider-output-invalid", "Le CLI hf n'a pas produit le fichier attendu.")
            library.update_journal(journal, state="manual-attention", error=error.message)
            raise error
        size = downloaded.stat().st_size
        if offer.network_bytes is not None and size != offer.network_bytes:
            error = ProviderError("download-truncated", "La taille téléchargée par hf est inattendue.")
            library.update_journal(journal, state="resumable", error=error.message)
            raise error
        stage = Path(journal.staging_path)
        stage.parent.mkdir(parents=True, exist_ok=True)
        os.replace(downloaded, stage)
        events.progress(size, size)
        resolved_digest = offer.trusted_digest or _local_metadata_digest(
            operation_directory, remote_filename, revision
        )
        identity = (
            ArtifactIdentity(
                "verified", "sha256", resolved_digest, "hf-lfs-xet-sha256"
            )
            if resolved_digest
            else ArtifactIdentity("provisional", source="hf-metadata-unverified")
        )
        try:
            return library.commit_staged(
                journal,
                family=offer.family,
                identity=identity,
                origin=offer.locator,
                revision=revision,
                format_name=offer.format,
            )
        except LibraryError as exc:
            raise ProviderError(
                "provider-output-invalid", "Le modèle produit par hf est structurellement invalide."
            ) from exc


def parse_locator(locator: str) -> tuple[str, str, str]:
    match = _HF_LOCATOR.fullmatch(locator)
    if match is None:
        raise ProviderError(
            "hf-locator-invalid",
            "Le locator HF doit contenir dépôt, révision immuable de 40 hex et fichier exact.",
        )
    filename = PurePosixPath(match.group("filename"))
    if filename.is_absolute() or ".." in filename.parts or filename.name == "":
        raise ProviderError("hf-locator-invalid", "Le chemin de fichier HF est invalide.")
    return match.group("repo"), match.group("revision").lower(), str(filename)


def _sha256(value: str) -> str:
    normalized = value.removeprefix("sha256:").lower()
    if len(normalized) != 64 or any(character not in "0123456789abcdef" for character in normalized):
        raise ProviderError("hf-metadata-invalid", "Le SHA-256 HF est invalide.")
    return normalized


def _first_version(stdout: str, stderr: str) -> str | None:
    for line in (stdout + "\n" + stderr).splitlines():
        if any(character.isdigit() for character in line):
            return line.strip()
    return None


def _local_metadata_digest(
    local_dir: Path, remote_filename: str, revision: str
) -> str | None:
    cache = local_dir / ".cache/huggingface/download"
    direct = cache / f"{remote_filename}.metadata"
    candidates = [direct]
    if cache.is_dir() and not direct.is_file():
        candidates.extend(list(cache.rglob("*.metadata"))[:1000])
    for candidate in candidates:
        try:
            lines = candidate.read_text(encoding="utf-8")[:4096].splitlines()
        except OSError:
            continue
        normalized = [line.strip().strip('"') for line in lines if line.strip()]
        if not normalized or normalized[0].lower() != revision:
            continue
        for value in normalized[1:]:
            digest = value.removeprefix("sha256:").lower()
            if len(digest) == 64 and all(character in "0123456789abcdef" for character in digest):
                return digest
    return None
