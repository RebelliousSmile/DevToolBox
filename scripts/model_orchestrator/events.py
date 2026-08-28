"""Versioned NDJSON acquisition progress with terminal-event invariants."""

from __future__ import annotations

import json
import re
from dataclasses import asdict
from typing import Callable

from .library import redact_origin
from .models import ProgressEvent, SCHEMA_VERSION

_URL = re.compile(r"https?://[^\s]+")


def redact_message(message: str) -> str:
    return _URL.sub(lambda match: redact_origin(match.group(0)), message)


class EventStream:
    def __init__(self, operation_id: str, write: Callable[[str], object]):
        self.operation_id = operation_id
        self._write = write
        self._sequence = 0
        self._last_bytes = 0
        self._terminal = False
        self._emit("schema", message="model-orchestrator-acquisition")

    def progress(self, transferred_bytes: int, total_bytes: int | None = None) -> None:
        if transferred_bytes < self._last_bytes:
            raise ValueError("La progression ne peut pas reculer")
        if total_bytes is not None and transferred_bytes > total_bytes:
            raise ValueError("La progression dépasse la taille totale")
        self._last_bytes = transferred_bytes
        self._emit(
            "progress", transferred_bytes=transferred_bytes, total_bytes=total_bytes
        )

    def completed(self, artifact_id: str) -> None:
        self._emit("completed", transferred_bytes=self._last_bytes, artifact_id=artifact_id)

    def failed(self, message: str) -> None:
        self._emit("failed", transferred_bytes=self._last_bytes, message=message)

    def cancelled(self, message: str = "Téléchargement annulé.") -> None:
        self._emit("cancelled", transferred_bytes=self._last_bytes, message=message)

    def _emit(self, kind: str, **values) -> None:
        if self._terminal:
            raise RuntimeError("Un événement terminal a déjà été émis")
        self._sequence += 1
        message = values.get("message")
        if isinstance(message, str):
            values["message"] = redact_message(message)
        event = ProgressEvent(
            sequence=self._sequence,
            kind=kind,
            operation_id=self.operation_id,
            **values,
        )
        self._write(json.dumps(asdict(event), ensure_ascii=False, sort_keys=True) + "\n")
        if kind in {"completed", "failed", "cancelled"}:
            self._terminal = True

    @property
    def terminal(self) -> bool:
        return self._terminal


def schema_header() -> dict[str, object]:
    return {"schema_version": SCHEMA_VERSION, "protocol": "acquisition-ndjson"}
