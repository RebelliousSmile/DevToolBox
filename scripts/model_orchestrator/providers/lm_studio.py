"""Native LM Studio download with supported-listing-only export."""

from __future__ import annotations

import json
import ntpath
import os
import posixpath
import re
import shutil
import subprocess
import sys
from pathlib import Path, PurePosixPath
from typing import Any, Callable, Mapping, Sequence

from ..events import EventStream, NativeChildRunner
from ..library import LibraryError, NeutralLibrary
from ..models import (
    AcquisitionOffer,
    AcquisitionRequest,
    ArtifactIdentity,
    LibraryRecord,
    ProviderStatus,
)
from .direct import ProviderError

_LOCATOR = re.compile(r"lmstudio://(?P<id>[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+){1,})\Z")
ListRunner = Callable[[Sequence[str], Mapping[str, str]], subprocess.CompletedProcess[str]]


def _list_run(command: Sequence[str], env: Mapping[str, str]):
    return subprocess.run(
        list(command), capture_output=True, text=True, timeout=30, check=False, env=dict(env)
    )


class LMStudioProvider:
    name = "lm-studio"

    def __init__(
        self,
        *,
        which: Callable[[str], str | None] = shutil.which,
        child_runner: NativeChildRunner | None = None,
        list_runner: ListRunner = _list_run,
        env: Mapping[str, str] | None = None,
        home: Path | None = None,
        platform_name: str | None = None,
        cancelled: Callable[[], bool] | None = None,
        timeout_seconds: float = 3600.0,
    ):
        self._which = which
        self._child_runner = child_runner or NativeChildRunner()
        self._list_runner = list_runner
        self._env = dict(os.environ if env is None else env)
        self._home = Path.home() if home is None else home
        self._platform = platform_name or ("windows" if sys.platform == "win32" else "linux")
        self._cancelled = cancelled or (lambda: False)
        self._timeout = timeout_seconds

    def accepts(self, locator: str) -> bool:
        return locator.startswith("lmstudio://")

    def status(self) -> ProviderStatus:
        executable = self._which("lms")
        if executable is None:
            return ProviderStatus(
                self.name,
                "unavailable",
                guidance="Installez LM Studio et activez son CLI `lms`.",
            )
        try:
            result = self._list_runner((executable, "--version"), self._env)
        except (OSError, subprocess.SubprocessError):
            return ProviderStatus(self.name, "error", guidance="Le CLI lms ne démarre pas.")
        version = next(
            (
                line.strip()
                for line in (result.stdout + "\n" + result.stderr).splitlines()
                if any(character.isdigit() for character in line)
            ),
            None,
        )
        return ProviderStatus(self.name, "available", version=version)

    def resolve(self, request: AcquisitionRequest, locator: str) -> AcquisitionOffer:
        model_id = parse_locator(locator)
        executable = self._which("lms") is not None
        filename = PurePosixPath(model_id).name
        return AcquisitionOffer(
            provider=self.name,
            locator=locator,
            family=request.family,
            immutable_revision=None,
            filename=filename,
            format=Path(filename).suffix.lower().lstrip(".") or "gguf",
            executable=executable,
            network_bytes=None,
            local_copy_bytes=None,
            temporary_bytes=None,
            resume_supported=True,
            identity_evidence="lms-supported-listing-sha256",
            owner_tool="lm-studio",
            export_method="supported-listing-hard-link-or-copy" if executable else None,
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
        model_id = parse_locator(offer.locator)
        executable = self._which("lms")
        if executable is None:
            raise ProviderError("provider-unavailable", "Le CLI lms est absent.")
        journal = library.begin(operation_id, offer.filename)
        progress_error: list[ProviderError] = []
        last_progress = [0]

        def progress(line: str) -> None:
            if not line.strip():
                return
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                progress_error.append(
                    ProviderError("lms-progress-invalid", "Progression lms non structurée.")
                )
                return
            if not isinstance(row, dict):
                progress_error.append(
                    ProviderError("lms-progress-invalid", "Progression lms invalide.")
                )
                return
            completed = row.get("downloadedBytes", row.get("completed"))
            total = row.get("totalBytes", row.get("total"))
            if isinstance(completed, int) and not isinstance(completed, bool):
                last_progress[0] = max(last_progress[0], completed)
                selected_total = (
                    max(total, last_progress[0]) if isinstance(total, int) else None
                )
                events.progress(last_progress[0], selected_total)

        try:
            result = self._child_runner.run(
                (executable, "get", model_id, "--yes", "--json"),
                env=self._env,
                on_stdout=progress,
                cancelled=self._cancelled,
                timeout_seconds=self._timeout,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            error = ProviderError("provider-command-failed", "Le CLI lms n'a pas démarré.")
            library.update_journal(journal, state="resumable", error=error.message)
            raise error from exc
        if result.cancelled:
            error = ProviderError("download-cancelled", "Téléchargement LM Studio annulé.")
            library.update_journal(journal, state="resumable", error=error.message)
            raise error
        if result.timed_out:
            error = ProviderError("download-timeout", "Téléchargement LM Studio expiré.")
            library.update_journal(journal, state="resumable", error=error.message)
            raise error
        if result.returncode != 0:
            error = ProviderError("provider-nonzero-exit", "Le CLI lms a signalé un échec.")
            library.update_journal(journal, state="resumable", error=error.message)
            raise error
        if progress_error:
            library.update_journal(
                journal, state="manual-attention", error=progress_error[0].message
            )
            raise progress_error[0]
        row = self._listed_model(executable, model_id)
        if row is None:
            error = ProviderError(
                "lms-export-evidence-unknown",
                "Le téléchargement reste géré par LM Studio; aucun chemin fiable n'est exportable.",
            )
            library.update_journal(journal, state="manual-attention", error=error.message)
            raise error
        source = row.get("path")
        digest = row.get("sha256")
        if not isinstance(source, str) or not self._owned_path(source):
            error = ProviderError(
                "lms-export-path-unsafe",
                "LM Studio n'a pas fourni un chemin exact sous sa racine possédée.",
            )
            library.update_journal(journal, state="manual-attention", error=error.message)
            raise error
        normalized_digest = _sha256(digest)
        if normalized_digest is None:
            error = ProviderError(
                "lms-export-identity-unknown",
                "Le modèle reste géré par LM Studio faute d'identité SHA-256 fiable.",
            )
            library.update_journal(journal, state="manual-attention", error=error.message)
            raise error
        identity = ArtifactIdentity(
            "verified", "sha256", normalized_digest, "lms-supported-listing-sha256"
        )
        try:
            return library.stage_file(
                journal,
                source,
                family=offer.family,
                identity=identity,
                origin=offer.locator,
                revision=normalized_digest,
                format_name=offer.format,
            )
        except (LibraryError, OSError) as exc:
            error = ProviderError("lms-export-invalid", "L'export LM Studio a échoué.")
            library.update_journal(journal, state="manual-attention", error=error.message)
            raise error from exc

    def _listed_model(self, executable: str, model_id: str) -> dict[str, Any] | None:
        try:
            result = self._list_runner((executable, "ls", "--json"), self._env)
            if result.returncode != 0:
                return None
            payload = json.loads(result.stdout)
        except (OSError, subprocess.SubprocessError, json.JSONDecodeError):
            return None
        if isinstance(payload, dict):
            payload = payload.get("models", payload.get("data"))
        if not isinstance(payload, list):
            return None
        matches = [
            row
            for row in payload
            if isinstance(row, dict)
            and (row.get("modelKey") == model_id or row.get("id") == model_id)
        ]
        return matches[0] if len(matches) == 1 else None

    def _owned_path(self, raw: str) -> bool:
        path_module = ntpath if self._platform == "windows" else posixpath
        if not path_module.isabs(raw):
            return False
        root = self._models_root()
        try:
            lexical = path_module.commonpath(
                [path_module.normcase(raw), path_module.normcase(root)]
            ) == path_module.normcase(root)
            if not lexical:
                return False
            if (self._platform == "windows") == (sys.platform == "win32"):
                resolved = str(Path(raw).resolve())
                resolved_root = str(Path(root).resolve())
                return path_module.commonpath(
                    [path_module.normcase(resolved), path_module.normcase(resolved_root)]
                ) == path_module.normcase(resolved_root)
            return True
        except ValueError:
            return False

    def _models_root(self) -> str:
        override = self._env.get("LM_STUDIO_MODELS_DIR", "").strip()
        if override:
            return override
        if self._platform == "windows":
            return ntpath.join(str(self._home), ".lmstudio", "models")
        return str(self._home / ".lmstudio/models")


def parse_locator(locator: str) -> str:
    match = _LOCATOR.fullmatch(locator)
    if match is None:
        raise ProviderError(
            "lms-locator-invalid", "Le locator LM Studio doit être un identifiant natif exact."
        )
    return match.group("id")


def _sha256(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    normalized = value.removeprefix("sha256:").lower()
    if len(normalized) != 64 or any(character not in "0123456789abcdef" for character in normalized):
        return None
    return normalized
