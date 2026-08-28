"""Transactional neutral model library with explicit recovery state."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
from dataclasses import asdict, replace
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Mapping
from urllib.parse import urlsplit, urlunsplit

from .formats import detect_format, validate_model_file
from .models import (
    FAMILIES,
    ArtifactIdentity,
    LibraryJournal,
    LibraryRecord,
    ValidationEvidence,
)
from .paths import file_evidence, same_filesystem

_EXACT_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")


class LibraryError(RuntimeError):
    pass


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _exact(value: str, field: str) -> str:
    if not isinstance(value, str) or _EXACT_ID.fullmatch(value) is None:
        raise LibraryError(f"{field} invalide")
    return value


def _family(value: str) -> str:
    if value not in FAMILIES:
        raise LibraryError("Famille de modèle invalide")
    return value


def redact_origin(origin: str) -> str:
    """Keep useful provenance while dropping credentials, query, and fragment."""

    try:
        parsed = urlsplit(origin)
        port = parsed.port
    except ValueError:
        return "redacted-origin"
    if not parsed.scheme or not parsed.netloc:
        return origin
    host = parsed.hostname or ""
    if port is not None:
        host = f"{host}:{port}"
    return urlunsplit((parsed.scheme, host, parsed.path, "", ""))


class NeutralLibrary:
    def __init__(self, root: str | Path):
        self.root = Path(root).absolute()
        self.staging_root = self.root / ".staging"
        self.artifacts_root = self.root / "artifacts"
        self.hash_queue_root = self.root / ".hash-queue"

    def initialize(self) -> None:
        self.staging_root.mkdir(parents=True, exist_ok=True)
        self.artifacts_root.mkdir(parents=True, exist_ok=True)
        self.hash_queue_root.mkdir(parents=True, exist_ok=True)
        if not same_filesystem(self.staging_root, self.artifacts_root):
            raise LibraryError("Le staging et les artefacts doivent partager le même volume")

    def begin(
        self,
        operation_id: str,
        filename: str,
        *,
        expected_digest: str | None = None,
    ) -> LibraryJournal:
        operation_id = _exact(operation_id, "operation_id")
        if Path(filename).name != filename or not filename:
            raise LibraryError("filename invalide")
        self.initialize()
        operation = self.staging_root / operation_id
        operation.mkdir(exist_ok=False)
        timestamp = _now()
        journal = LibraryJournal(
            operation_id=operation_id,
            state="staging",
            filename=filename,
            staging_path=str(operation / f"{filename}.partial"),
            expected_digest=expected_digest,
            created_at=timestamp,
            updated_at=timestamp,
        )
        self._write_journal(journal)
        return journal

    def commit_stream(
        self,
        operation_id: str,
        filename: str,
        chunks: Iterable[bytes],
        *,
        family: str,
        expected_digest: str | None = None,
        origin: str = "unknown",
        revision: str | None = None,
        compute_identity: bool = True,
        format_name: str | None = None,
    ) -> LibraryRecord:
        family = _family(family)
        expected = self._normalize_digest(expected_digest)
        journal = self.begin(operation_id, filename, expected_digest=expected)
        stage = Path(journal.staging_path)
        digest = hashlib.sha256()
        written = 0
        try:
            with stage.open("xb") as stream:
                for chunk in chunks:
                    if not isinstance(chunk, bytes):
                        raise LibraryError("Le flux doit produire des bytes")
                    stream.write(chunk)
                    digest.update(chunk)
                    written += len(chunk)
                stream.flush()
                os.fsync(stream.fileno())
            actual = digest.hexdigest()
            journal.bytes_written = written
            if expected is not None and actual != expected:
                raise LibraryError("Le SHA-256 reçu ne correspond pas au digest attendu")
            identity = (
                ArtifactIdentity("verified", "sha256", actual, "stream-sha256")
                if compute_identity or expected is not None
                else ArtifactIdentity("provisional", source="stream-unverified")
            )
            return self._commit_staged(
                journal,
                identity=identity,
                family=family,
                origin=origin,
                revision=revision,
                format_name=format_name,
            )
        except BaseException as exc:
            self._fail(journal, exc)
            raise

    def import_file(
        self,
        operation_id: str,
        source: str | Path,
        *,
        family: str,
        filename: str | None = None,
        origin: str = "local-import",
        revision: str | None = None,
    ) -> LibraryRecord:
        family = _family(family)
        candidate = Path(source)
        selected_name = filename or candidate.name
        journal = self.begin(operation_id, selected_name)
        stage = Path(journal.staging_path)
        try:
            if candidate.is_symlink():
                raise LibraryError("Un lien symbolique externe ne peut pas devenir canonique")
            if not candidate.is_file():
                raise LibraryError("Le fichier source est absent")
            if same_filesystem(candidate, self.staging_root):
                os.link(candidate, stage)
            else:
                shutil.copy2(candidate, stage)
            journal.bytes_written = stage.stat().st_size
            return self._commit_staged(
                journal,
                identity=ArtifactIdentity("provisional", source="local-import"),
                family=family,
                origin=origin,
                revision=revision,
                format_name=None,
            )
        except BaseException as exc:
            self._fail(journal, exc)
            raise

    def commit_staged(
        self,
        journal: LibraryJournal,
        *,
        family: str,
        identity: ArtifactIdentity,
        origin: str,
        revision: str | None = None,
        format_name: str | None = None,
    ) -> LibraryRecord:
        """Commit bytes written directly by a provider without another allocation."""

        family = _family(family)
        try:
            journal.bytes_written = Path(journal.staging_path).stat().st_size
            return self._commit_staged(
                journal,
                identity=identity,
                family=family,
                origin=origin,
                revision=revision,
                format_name=format_name,
            )
        except BaseException as exc:
            self._fail(journal, exc)
            raise

    def fail(self, journal: LibraryJournal, error: BaseException) -> None:
        self._fail(journal, error)

    def update_journal(
        self, journal: LibraryJournal, *, state: str, error: str | None = None
    ) -> LibraryJournal:
        journal.state = state
        journal.error = error
        journal.updated_at = _now()
        if Path(journal.staging_path).is_file():
            journal.bytes_written = Path(journal.staging_path).stat().st_size
        self._write_journal(journal)
        return journal

    def load_journal(self, operation_id: str) -> LibraryJournal | None:
        operation_id = _exact(operation_id, "operation_id")
        path = self.staging_root / operation_id / "journal.json"
        try:
            return LibraryJournal(**json.loads(path.read_text(encoding="utf-8")))
        except FileNotFoundError:
            return None
        except (OSError, TypeError, ValueError, json.JSONDecodeError) as exc:
            raise LibraryError(f"Journal illisible : {exc}") from exc

    def _commit_staged(
        self,
        journal: LibraryJournal,
        *,
        identity: ArtifactIdentity,
        family: str,
        origin: str,
        revision: str | None,
        format_name: str | None,
    ) -> LibraryRecord:
        stage = Path(journal.staging_path)
        selected_format = (format_name or detect_format(journal.filename)).lower()
        validation = validate_model_file(
            stage,
            format_name=selected_format,
            identity_verified=identity.state == "verified",
        )
        if not validation.valid:
            raise LibraryError(validation.message)
        artifact_id = identity.value if identity.state == "verified" else journal.operation_id
        if artifact_id is None:
            raise LibraryError("Identité canonique absente")
        artifact_id = _exact(artifact_id, "artifact_id")
        target_directory = self.artifacts_root / artifact_id
        if target_directory.is_dir() and identity.state == "verified":
            existing = next(
                (record for record in self.list_records() if record.artifact_id == artifact_id),
                None,
            )
            if existing is None or existing.identity.exact_key != identity.exact_key:
                raise LibraryError("Le répertoire canonique existant est incohérent")
            stage.unlink()
            journal.state = "completed"
            journal.artifact_id = artifact_id
            journal.target_path = existing.path
            journal.updated_at = _now()
            self._write_journal(journal)
            return existing
        target_directory.mkdir(exist_ok=False)
        target = target_directory / journal.filename
        journal.state = "committing"
        journal.artifact_id = artifact_id
        journal.target_path = str(target)
        journal.updated_at = _now()
        self._write_journal(journal)
        os.replace(stage, target)
        evidence = file_evidence(target)
        stat = target.stat()
        record = LibraryRecord(
            artifact_id=artifact_id,
            path=str(target),
            filename=journal.filename,
            family=family,
            format=selected_format,
            identity=identity,
            validation=validation,
            logical_size=stat.st_size,
            allocated_size=evidence.allocated_size,
            relationship=evidence.relationship,
            allocation_id=evidence.allocation_id,
            origin=redact_origin(origin),
            revision=revision,
            created_at=_now(),
            hash_pending=identity.state != "verified",
        )
        self._write_record(target_directory, record)
        if record.hash_pending:
            self._queue_hash(record)
        journal.state = "completed"
        journal.updated_at = _now()
        self._write_journal(journal)
        return record

    def list_records(self) -> list[LibraryRecord]:
        if not self.artifacts_root.is_dir():
            return []
        records: list[LibraryRecord] = []
        for path in sorted(self.artifacts_root.glob("*/record.json")):
            try:
                payload = json.loads(path.read_text(encoding="utf-8"))
                identity = ArtifactIdentity(**payload.pop("identity"))
                validation = ValidationEvidence(**payload.pop("validation"))
                records.append(LibraryRecord(identity=identity, validation=validation, **payload))
            except (OSError, TypeError, ValueError, json.JSONDecodeError):
                continue
        return records

    def reconcile(self) -> list[LibraryJournal]:
        if not self.staging_root.is_dir():
            return []
        journals: list[LibraryJournal] = []
        for operation in sorted(path for path in self.staging_root.iterdir() if path.is_dir()):
            journal_path = operation / "journal.json"
            try:
                journal = LibraryJournal(**json.loads(journal_path.read_text(encoding="utf-8")))
            except (OSError, TypeError, ValueError, json.JSONDecodeError) as exc:
                journals.append(
                    LibraryJournal(
                        operation_id=operation.name,
                        state="manual-attention",
                        filename="unknown",
                        staging_path=str(operation),
                        error=f"Journal illisible : {exc}",
                    )
                )
                continue
            stage_exists = Path(journal.staging_path).is_file()
            target_exists = bool(journal.target_path and Path(journal.target_path).is_file())
            record_exists = bool(
                journal.artifact_id
                and (self.artifacts_root / journal.artifact_id / "record.json").is_file()
            )
            if target_exists and record_exists:
                state = "completed"
            elif stage_exists and journal.state in {"staging", "resumable"}:
                state = "resumable"
            elif not stage_exists and not target_exists and journal.state == "staging":
                state = "discardable"
            else:
                state = "manual-attention"
            bytes_written = Path(journal.staging_path).stat().st_size if stage_exists else journal.bytes_written
            journals.append(replace(journal, state=state, bytes_written=bytes_written))
        return journals

    def discard(self, operation_id: str) -> None:
        operation_id = _exact(operation_id, "operation_id")
        operation = self.staging_root / operation_id
        if operation.parent != self.staging_root or not operation.is_dir():
            raise LibraryError("Opération de recovery inconnue")
        shutil.rmtree(operation)

    def _write_journal(self, journal: LibraryJournal) -> None:
        operation = self.staging_root / journal.operation_id
        self._atomic_json(operation / "journal.json", asdict(journal))

    def _write_record(self, directory: Path, record: LibraryRecord) -> None:
        self._atomic_json(directory / "record.json", asdict(record))

    @staticmethod
    def _atomic_json(path: Path, payload: Mapping[str, object]) -> None:
        temporary = path.with_suffix(path.suffix + ".tmp")
        with temporary.open("w", encoding="utf-8") as stream:
            json.dump(payload, stream, ensure_ascii=False, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)

    def _queue_hash(self, record: LibraryRecord) -> None:
        try:
            self._atomic_json(
                self.hash_queue_root / f"{record.artifact_id}.json",
                {"artifact_id": record.artifact_id, "path": record.path, "queued_at": _now()},
            )
        except OSError:
            # Hashing is deliberately optional and must never delay first availability.
            pass

    def _fail(self, journal: LibraryJournal, error: BaseException) -> None:
        journal.state = "manual-attention"
        journal.error = str(error)
        journal.updated_at = _now()
        self._write_journal(journal)

    @staticmethod
    def _normalize_digest(digest: str | None) -> str | None:
        if digest is None:
            return None
        normalized = digest.removeprefix("sha256:").lower()
        if len(normalized) != 64 or any(character not in "0123456789abcdef" for character in normalized):
            raise LibraryError("Digest SHA-256 invalide")
        return normalized
