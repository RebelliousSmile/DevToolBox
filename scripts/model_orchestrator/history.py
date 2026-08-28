"""Bounded machine-local acquisition performance history."""

from __future__ import annotations

import json
import os
from dataclasses import asdict
from pathlib import Path

from .models import PerformanceObservation

HISTORY_SCHEMA_VERSION = 1
MAX_OBSERVATIONS_PER_PROVIDER_KIND = 10


class HistoryStore:
    def __init__(self, path: str | Path):
        self.path = Path(path)

    def load(self) -> list[PerformanceObservation]:
        try:
            payload = json.loads(self.path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            return []
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(f"Historique de performance illisible : {exc}") from exc
        if (
            not isinstance(payload, dict)
            or payload.get("schema_version") != HISTORY_SCHEMA_VERSION
            or not isinstance(payload.get("observations"), list)
        ):
            raise ValueError("Historique de performance incompatible")
        return [PerformanceObservation(**row) for row in payload["observations"]]

    def append(self, observation: PerformanceObservation) -> None:
        rows = self.load()
        rows.append(observation)
        grouped: dict[tuple[str, str], list[PerformanceObservation]] = {}
        for row in rows:
            grouped.setdefault((row.provider, row.kind), []).append(row)
        kept = []
        for key in sorted(grouped):
            kept.extend(grouped[key][-MAX_OBSERVATIONS_PER_PROVIDER_KIND:])
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.path.with_suffix(".tmp")
        temporary.write_text(
            json.dumps(
                {
                    "schema_version": HISTORY_SCHEMA_VERSION,
                    "observations": [asdict(row) for row in kept],
                },
                ensure_ascii=False,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        os.replace(temporary, self.path)
