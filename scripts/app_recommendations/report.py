"""Collector aggregation and stable report generation."""

from __future__ import annotations

import json
import platform as platform_module
import queue
import threading
import time
from collections.abc import Callable, Mapping
from dataclasses import replace
from datetime import datetime, timezone
from typing import Any

from .history import UsageHistory, usage_for
from .models import (
    Candidate,
    RecommendationReport,
    SizeEvidence,
    SourceError,
)
from .scoring import score_candidate

Collector = Callable[[], list[Candidate]]


def _merge_candidates(left: Candidate, right: Candidate) -> Candidate:
    left_size = left.size.installed_bytes
    right_size = right.size.installed_bytes
    size = right.size if right_size is not None and (left_size is None or right_size > left_size) else left.size
    protected = left.protection if left.protection.protected else right.protection
    command = None if protected.protected else left.command or right.command
    return replace(
        left,
        name=min(left.name, right.name, key=str.casefold),
        size=size,
        executable_hints=sorted(set(left.executable_hints + right.executable_hints)),
        protection=protected,
        command=command,
        metadata={**right.metadata, **left.metadata},
    )


def _collect_with_timeouts(
    collectors: Mapping[str, Collector], timeout_seconds: float
) -> tuple[list[Candidate], list[SourceError]]:
    outcomes: dict[str, queue.Queue[tuple[str, Any]]] = {}
    threads: dict[str, threading.Thread] = {}
    started_at: dict[str, float] = {}

    def run(source: str, collector: Collector) -> None:
        try:
            outcomes[source].put(("ok", collector()))
        except Exception as exc:  # collector isolation is the contract
            outcomes[source].put(("error", exc))

    for source, collector in collectors.items():
        outcomes[source] = queue.Queue(maxsize=1)
        thread = threading.Thread(target=run, args=(source, collector), daemon=True)
        threads[source] = thread
        started_at[source] = time.monotonic()
        thread.start()

    candidates: list[Candidate] = []
    errors: list[SourceError] = []
    for source in sorted(collectors):
        thread = threads[source]
        remaining = max(0.0, started_at[source] + timeout_seconds - time.monotonic())
        thread.join(remaining)
        if thread.is_alive():
            errors.append(SourceError(source, "timeout", f"Délai de {timeout_seconds:g}s dépassé"))
            continue
        try:
            state, value = outcomes[source].get_nowait()
        except queue.Empty:
            errors.append(SourceError(source, "empty_result", "Le collecteur n'a produit aucun résultat"))
            continue
        if state == "error":
            errors.append(SourceError(source, "collector_error", str(value)))
        else:
            candidates.extend(value)
    return candidates, errors


def build_report(
    collectors: Mapping[str, Collector],
    history: UsageHistory | None = None,
    *,
    timeout_seconds: float = 10.0,
    generated_at: datetime | None = None,
    platform_name: str | None = None,
) -> RecommendationReport:
    if timeout_seconds <= 0:
        raise ValueError("timeout_seconds must be positive")
    history = history or UsageHistory()
    raw_candidates, errors = _collect_with_timeouts(collectors, timeout_seconds)

    deduplicated: dict[str, Candidate] = {}
    for candidate in raw_candidates:
        existing = deduplicated.get(candidate.app_id)
        deduplicated[candidate.app_id] = (
            _merge_candidates(existing, candidate) if existing else candidate
        )

    scored: list[Candidate] = []
    for candidate in deduplicated.values():
        with_usage = replace(candidate, usage=usage_for(candidate.app_id, history))
        scored.append(score_candidate(with_usage))
    scored.sort(key=lambda item: (-item.score, item.name.casefold(), item.app_id))

    timestamp = generated_at or datetime.now(timezone.utc)
    return RecommendationReport(
        generated_at=timestamp.isoformat(),
        platform=platform_name or platform_module.system().lower(),
        candidates=scored,
        source_errors=sorted(errors, key=lambda error: (error.source, error.code)),
        warnings=list(history.warnings),
    )


def to_json(report: RecommendationReport) -> str:
    return json.dumps(report.to_dict(), ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def empty_candidate(source: str, app_id: str, name: str) -> Candidate:
    """Small public fixture helper used by collector tests."""
    return Candidate(app_id=f"{source}:{app_id}", source=source, name=name, size=SizeEvidence())


def default_collectors() -> dict[str, Collector]:
    """Resolve platform collectors lazily to keep the domain model OS-neutral."""
    from .collectors import available_collectors

    return available_collectors()
