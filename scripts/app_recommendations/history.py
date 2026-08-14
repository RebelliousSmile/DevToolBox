"""Read and interpret the privacy-preserving local usage history."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import date, datetime
from pathlib import Path
from typing import Any

from .models import UsageEvidence

HISTORY_VERSION = 1


@dataclass
class AppHistory:
    tracked_since: datetime
    last_seen: datetime | None = None


@dataclass
class UsageHistory:
    apps: dict[str, AppHistory] = field(default_factory=dict)
    coverage: dict[date, int] = field(default_factory=dict)
    warnings: list[str] = field(default_factory=list)


def _parse_datetime(value: Any) -> datetime:
    if not isinstance(value, str):
        raise ValueError("timestamp must be a string")
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("timestamp must include a timezone")
    return parsed


def load_history(path: str | Path | None) -> UsageHistory:
    if path is None:
        return UsageHistory()
    history_path = Path(path)
    if not history_path.is_file():
        return UsageHistory()
    try:
        payload = json.loads(history_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        return UsageHistory(warnings=[f"Historique d'usage illisible : {exc}"])
    if not isinstance(payload, dict) or payload.get("version") != HISTORY_VERSION:
        return UsageHistory(warnings=["Version d'historique d'usage non reconnue"])

    result = UsageHistory()
    raw_apps = payload.get("apps", {})
    if isinstance(raw_apps, dict):
        for app_id, raw in raw_apps.items():
            try:
                if not isinstance(app_id, str) or not isinstance(raw, dict):
                    raise ValueError("invalid application entry")
                tracked_since = _parse_datetime(raw.get("tracked_since"))
                last_seen_value = raw.get("last_seen")
                last_seen = _parse_datetime(last_seen_value) if last_seen_value else None
                if last_seen is not None and last_seen < tracked_since:
                    raise ValueError("last_seen predates tracked_since")
                result.apps[app_id] = AppHistory(tracked_since, last_seen)
            except (TypeError, ValueError) as exc:
                result.warnings.append(f"Entrée d'historique ignorée ({app_id!r}) : {exc}")

    raw_coverage = payload.get("coverage", {})
    if isinstance(raw_coverage, dict):
        for raw_day, raw_count in raw_coverage.items():
            try:
                day = date.fromisoformat(raw_day)
                if not isinstance(raw_count, int) or isinstance(raw_count, bool) or raw_count <= 0:
                    raise ValueError("sample count must be a positive integer")
                result.coverage[day] = raw_count
            except (TypeError, ValueError) as exc:
                result.warnings.append(f"Jour de couverture ignoré ({raw_day!r}) : {exc}")
    return result


def usage_for(app_id: str, history: UsageHistory) -> UsageEvidence:
    entry = history.apps.get(app_id)
    if entry is None:
        return UsageEvidence()
    anchor = entry.last_seen or entry.tracked_since
    covered_days = sum(1 for day, count in history.coverage.items() if count > 0 and day > anchor.date())
    confidence = "low" if covered_days < 30 else "medium"
    if entry.last_seen is not None:
        return UsageEvidence(
            kind="known_last_seen",
            last_seen=entry.last_seen.isoformat(),
            tracked_since=entry.tracked_since.isoformat(),
            covered_days=covered_days,
            confidence=confidence,
        )
    return UsageEvidence(
        kind="not_observed",
        tracked_since=entry.tracked_since.isoformat(),
        covered_days=covered_days,
        confidence=confidence,
    )
