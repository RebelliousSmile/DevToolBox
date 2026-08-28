"""Versioned NDJSON acquisition progress with terminal-event invariants."""

from __future__ import annotations

import json
import os
import queue
import re
import signal
import subprocess
import sys
import threading
import time
from dataclasses import asdict
from dataclasses import dataclass
from typing import Mapping, Sequence
from typing import Callable

from .library import redact_origin
from .models import ProgressEvent, SCHEMA_VERSION

_URL = re.compile(r"https?://[^\s]+")


def redact_message(message: str) -> str:
    return _URL.sub(lambda match: redact_origin(match.group(0)), message)


class EventStream:
    def __init__(
        self,
        operation_id: str,
        write: Callable[[str], object],
        *,
        clock: Callable[[], float] = time.monotonic,
    ):
        self.operation_id = operation_id
        self._write = write
        self._sequence = 0
        self._last_bytes = 0
        self._terminal = False
        self._clock = clock
        self._started_at = clock()
        self._first_progress_at: float | None = None
        self._emit("schema", message="model-orchestrator-acquisition")

    def progress(self, transferred_bytes: int, total_bytes: int | None = None) -> None:
        if transferred_bytes < self._last_bytes:
            raise ValueError("La progression ne peut pas reculer")
        if total_bytes is not None and transferred_bytes > total_bytes:
            raise ValueError("La progression dépasse la taille totale")
        self._last_bytes = transferred_bytes
        if self._first_progress_at is None:
            self._first_progress_at = self._clock()
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

    @property
    def startup_seconds(self) -> float:
        endpoint = self._first_progress_at if self._first_progress_at is not None else self._clock()
        return max(endpoint - self._started_at, 0.0)


def schema_header() -> dict[str, object]:
    return {"schema_version": SCHEMA_VERSION, "protocol": "acquisition-ndjson"}


@dataclass(frozen=True)
class ChildResult:
    returncode: int
    stdout: tuple[str, ...]
    stderr: tuple[str, ...]
    cancelled: bool = False
    timed_out: bool = False


class NativeChildRunner:
    """Own a native provider process group and terminate its descendants on exit."""

    def __init__(self, popen=subprocess.Popen):
        self._popen = popen

    def run(
        self,
        command: Sequence[str],
        *,
        env: Mapping[str, str],
        on_stdout=lambda _line: None,
        cancelled=lambda: False,
        timeout_seconds: float = 3600.0,
    ) -> ChildResult:
        options = {
            "stdout": subprocess.PIPE,
            "stderr": subprocess.PIPE,
            "text": True,
            "env": dict(env),
        }
        if sys.platform == "win32":
            options["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        else:
            options["start_new_session"] = True
        process = self._popen(list(command), **options)
        messages: queue.Queue[tuple[str, str | None]] = queue.Queue()

        def read_stream(name: str, stream) -> None:
            try:
                for line in iter(stream.readline, ""):
                    messages.put((name, line.rstrip("\r\n")))
            finally:
                messages.put((name, None))

        threads = [
            threading.Thread(target=read_stream, args=("stdout", process.stdout), daemon=True),
            threading.Thread(target=read_stream, args=("stderr", process.stderr), daemon=True),
        ]
        for thread in threads:
            thread.start()
        stdout: list[str] = []
        stderr: list[str] = []
        completed_streams = 0
        started = time.monotonic()
        was_cancelled = False
        timed_out = False
        while process.poll() is None or completed_streams < 2:
            if process.poll() is None and cancelled():
                was_cancelled = True
                self.terminate_tree(process)
            if process.poll() is None and time.monotonic() - started >= timeout_seconds:
                timed_out = True
                self.terminate_tree(process)
            try:
                name, line = messages.get(timeout=0.05)
            except queue.Empty:
                continue
            if line is None:
                completed_streams += 1
            elif name == "stdout":
                stdout.append(line)
                try:
                    on_stdout(line)
                except BaseException:
                    self.terminate_tree(process)
                    raise
            else:
                stderr.append(line)
        returncode = process.wait()
        for thread in threads:
            thread.join(timeout=1)
        process.stdout.close()
        process.stderr.close()
        return ChildResult(
            returncode,
            tuple(stdout),
            tuple(stderr),
            cancelled=was_cancelled,
            timed_out=timed_out,
        )

    @staticmethod
    def terminate_tree(process) -> None:
        if process.poll() is not None:
            return
        try:
            if sys.platform == "win32":
                process.send_signal(getattr(signal, "CTRL_BREAK_EVENT", signal.SIGTERM))
            else:
                os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            try:
                if sys.platform == "win32":
                    subprocess.run(
                        ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                        capture_output=True,
                        timeout=5,
                        check=False,
                    )
                else:
                    os.killpg(process.pid, signal.SIGKILL)
            except (OSError, subprocess.SubprocessError):
                process.kill()
