"""Stable schema-v1 records shared by model-orchestrator adapters and Rust."""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any

SCHEMA_VERSION = 1
FAMILIES = frozenset({"llm", "image"})
IDENTITY_STATES = frozenset({"verified", "provisional", "unknown"})
RELATIONSHIPS = frozenset(
    {"copy", "hard_link", "symbolic_link", "owner_blob", "canonical", "unknown"}
)
CONFIDENCE_LEVELS = frozenset({"unknown", "low", "medium", "high"})


def _non_negative(value: int | None, field_name: str) -> None:
    if value is not None and (isinstance(value, bool) or value < 0):
        raise ValueError(f"{field_name} must be non-negative")


@dataclass(frozen=True)
class ArtifactIdentity:
    state: str = "unknown"
    algorithm: str | None = None
    value: str | None = None
    source: str = "unknown"

    def __post_init__(self) -> None:
        if self.state not in IDENTITY_STATES:
            raise ValueError(f"invalid identity state: {self.state}")
        if self.state == "verified":
            if self.algorithm != "sha256" or not self.value:
                raise ValueError("verified identity requires a sha256 value")
            normalized = self.value.lower()
            if len(normalized) != 64 or any(ch not in "0123456789abcdef" for ch in normalized):
                raise ValueError("verified sha256 must contain 64 hexadecimal characters")
            object.__setattr__(self, "value", normalized)

    @property
    def exact_key(self) -> str | None:
        if self.state != "verified":
            return None
        return f"{self.algorithm}:{self.value}"


@dataclass(frozen=True)
class ToolReference:
    tool: str
    reference_id: str
    kind: str = "catalog"
    owner: bool = False
    loaded: bool = False
    workflow: bool = False

    def __post_init__(self) -> None:
        if not self.tool.strip() or not self.reference_id.strip():
            raise ValueError("tool references require non-empty tool and reference_id")


@dataclass
class Protection:
    protected: bool = False
    reasons: list[str] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.reasons = sorted(set(reason for reason in self.reasons if reason))
        if self.reasons:
            self.protected = True


@dataclass(frozen=True)
class AdapterCapabilities:
    inventory: bool = True
    reference: bool = False
    hard_link: bool = False
    symbolic_link: bool = False
    copy: bool = False
    native_import: bool = False
    load_validation: bool = False
    inference_validation: bool = False
    native_delete: bool = False


@dataclass(frozen=True)
class ToolInstallation:
    tool: str
    detected: bool
    version: str | None = None
    executable: str | None = None
    roots: tuple[str, ...] = ()
    discovery_source: str = "unknown"
    confidence: str = "unknown"
    capabilities: AdapterCapabilities = field(default_factory=AdapterCapabilities)

    def __post_init__(self) -> None:
        if not self.tool.strip():
            raise ValueError("tool must not be empty")
        if self.confidence not in CONFIDENCE_LEVELS:
            raise ValueError(f"invalid confidence: {self.confidence}")


@dataclass
class Artifact:
    artifact_id: str
    path: str
    family: str
    format: str
    identity: ArtifactIdentity = field(default_factory=ArtifactIdentity)
    logical_size: int | None = None
    allocated_size: int | None = None
    revision: str | None = None
    quantization: str | None = None
    category: str | None = None
    relationship: str = "unknown"
    allocation_id: str | None = None
    references: list[ToolReference] = field(default_factory=list)
    protection: Protection = field(default_factory=Protection)
    duplicate_group: str | None = None
    metadata: dict[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        if not self.artifact_id.strip() or not self.path.strip() or not self.format.strip():
            raise ValueError("artifact id, path, and format must not be empty")
        if self.family not in FAMILIES:
            raise ValueError(f"invalid artifact family: {self.family}")
        if self.relationship not in RELATIONSHIPS:
            raise ValueError(f"invalid relationship: {self.relationship}")
        _non_negative(self.logical_size, "logical_size")
        _non_negative(self.allocated_size, "allocated_size")

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True)
class SourceError:
    source: str
    code: str
    message: str
    confidence: str = "high"

    def __post_init__(self) -> None:
        if not self.source or not self.code or not self.message:
            raise ValueError("source errors require source, code, and message")
        if self.confidence not in CONFIDENCE_LEVELS:
            raise ValueError(f"invalid confidence: {self.confidence}")


@dataclass
class CatalogSnapshot:
    generated_at: str
    platform: str
    installations: list[ToolInstallation] = field(default_factory=list)
    artifacts: list[Artifact] = field(default_factory=list)
    source_errors: list[SourceError] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    schema_version: int = SCHEMA_VERSION

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "generated_at": self.generated_at,
            "platform": self.platform,
            "installations": [asdict(item) for item in self.installations],
            "artifacts": [item.to_dict() for item in self.artifacts],
            "source_errors": [asdict(item) for item in self.source_errors],
            "warnings": list(self.warnings),
        }
