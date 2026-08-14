"""Stable JSON model for the application recommendation report."""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any

SCHEMA_VERSION = 1
CONFIDENCE_LEVELS = frozenset({"unknown", "low", "medium", "high"})
COMMAND_ORIGINS = frozenset({"manager_verified", "publisher_unverified"})
USAGE_KINDS = frozenset({"unknown", "known_last_seen", "not_observed"})


@dataclass
class SizeEvidence:
    installed_bytes: int | None = None
    reclaimable_bytes: int | None = None
    method: str = "unknown"
    scope: str = "unknown"
    confidence: str = "unknown"

    def __post_init__(self) -> None:
        if self.installed_bytes is not None and self.installed_bytes < 0:
            raise ValueError("installed_bytes must be non-negative")
        if self.reclaimable_bytes is not None and self.reclaimable_bytes < 0:
            raise ValueError("reclaimable_bytes must be non-negative")
        if self.confidence not in CONFIDENCE_LEVELS:
            raise ValueError(f"invalid size confidence: {self.confidence}")


@dataclass
class UsageEvidence:
    kind: str = "unknown"
    last_seen: str | None = None
    tracked_since: str | None = None
    covered_days: int = 0
    confidence: str = "unknown"

    def __post_init__(self) -> None:
        if self.kind not in USAGE_KINDS:
            raise ValueError(f"invalid usage kind: {self.kind}")
        if self.covered_days < 0:
            raise ValueError("covered_days must be non-negative")
        if self.confidence not in CONFIDENCE_LEVELS:
            raise ValueError(f"invalid usage confidence: {self.confidence}")


@dataclass
class Protection:
    protected: bool = False
    reasons: list[str] = field(default_factory=list)


@dataclass
class CommandSuggestion:
    value: str
    origin: str = "manager_verified"

    def __post_init__(self) -> None:
        if self.origin not in COMMAND_ORIGINS:
            raise ValueError(f"invalid command origin: {self.origin}")
        if not self.value.strip():
            raise ValueError("command value must not be empty")


@dataclass
class Candidate:
    app_id: str
    source: str
    name: str
    size: SizeEvidence = field(default_factory=SizeEvidence)
    executable_hints: list[str] = field(default_factory=list)
    usage: UsageEvidence = field(default_factory=UsageEvidence)
    protection: Protection = field(default_factory=Protection)
    command: CommandSuggestion | None = None
    score: int = 0
    confidence: str = "unknown"
    reasons: list[str] = field(default_factory=list)
    metadata: dict[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        if not self.app_id.startswith(f"{self.source}:"):
            raise ValueError("app_id must be prefixed by its source")
        if not self.name.strip():
            raise ValueError("candidate name must not be empty")
        if self.protection.protected:
            self.command = None
            self.score = 0
        if self.confidence not in CONFIDENCE_LEVELS:
            raise ValueError(f"invalid candidate confidence: {self.confidence}")
        self.executable_hints = sorted(set(self.executable_hints))

    def to_dict(self) -> dict[str, Any]:
        data = asdict(self)
        if self.command is None:
            data["command"] = None
        return data


@dataclass
class SourceError:
    source: str
    code: str
    message: str


@dataclass
class RecommendationReport:
    generated_at: str
    platform: str
    candidates: list[Candidate] = field(default_factory=list)
    source_errors: list[SourceError] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    schema_version: int = SCHEMA_VERSION

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "generated_at": self.generated_at,
            "platform": self.platform,
            "candidates": [candidate.to_dict() for candidate in self.candidates],
            "source_errors": [asdict(error) for error in self.source_errors],
            "warnings": list(self.warnings),
        }
