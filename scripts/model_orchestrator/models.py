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
VALIDATION_LEVELS = frozenset({"strong", "structural", "opaque", "failed"})
JOURNAL_STATES = frozenset(
    {"staging", "committing", "completed", "resumable", "discardable", "manual-attention"}
)
PROVIDER_STATES = frozenset({"available", "unavailable", "unauthenticated", "error"})
EVENT_KINDS = frozenset({"schema", "progress", "completed", "failed", "cancelled"})


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
class RootEvidence:
    path: str
    source: str
    confidence: str

    def __post_init__(self) -> None:
        if not self.path or not self.source:
            raise ValueError("root evidence requires path and source")
        if self.confidence not in CONFIDENCE_LEVELS:
            raise ValueError(f"invalid confidence: {self.confidence}")


@dataclass(frozen=True)
class ToolInstallation:
    tool: str
    detected: bool
    version: str | None = None
    executable: str | None = None
    roots: tuple[str, ...] = ()
    root_evidence: tuple[RootEvidence, ...] = ()
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


@dataclass(frozen=True)
class ValidationEvidence:
    valid: bool
    level: str
    format: str
    message: str

    def __post_init__(self) -> None:
        if self.level not in VALIDATION_LEVELS:
            raise ValueError(f"invalid validation level: {self.level}")
        if self.valid == (self.level == "failed"):
            raise ValueError("failed evidence must be invalid and other evidence valid")


@dataclass
class LibraryJournal:
    operation_id: str
    state: str
    filename: str
    staging_path: str
    target_path: str | None = None
    artifact_id: str | None = None
    expected_digest: str | None = None
    bytes_written: int = 0
    created_at: str = ""
    updated_at: str = ""
    error: str | None = None

    def __post_init__(self) -> None:
        if self.state not in JOURNAL_STATES:
            raise ValueError(f"invalid journal state: {self.state}")
        _non_negative(self.bytes_written, "bytes_written")


@dataclass(frozen=True)
class LibraryRecord:
    artifact_id: str
    path: str
    filename: str
    family: str
    format: str
    identity: ArtifactIdentity
    validation: ValidationEvidence
    logical_size: int
    allocated_size: int | None
    relationship: str
    allocation_id: str | None
    origin: str
    revision: str | None
    created_at: str
    hash_pending: bool = False
    destination_usability: dict[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        if self.family not in FAMILIES:
            raise ValueError(f"invalid artifact family: {self.family}")
        _non_negative(self.logical_size, "logical_size")
        _non_negative(self.allocated_size, "allocated_size")


@dataclass(frozen=True)
class AcquisitionRequest:
    primary_locator: str
    family: str
    alternatives: tuple[str, ...] = ()
    user_sha256: str | None = None

    def __post_init__(self) -> None:
        if not self.primary_locator:
            raise ValueError("primary locator is required")
        if self.family not in FAMILIES:
            raise ValueError(f"invalid artifact family: {self.family}")


@dataclass(frozen=True)
class ProviderStatus:
    provider: str
    state: str
    version: str | None = None
    authenticated: bool | None = None
    guidance: str | None = None

    def __post_init__(self) -> None:
        if self.state not in PROVIDER_STATES:
            raise ValueError(f"invalid provider state: {self.state}")


@dataclass(frozen=True)
class AcquisitionOffer:
    provider: str
    locator: str
    family: str
    immutable_revision: str | None
    filename: str
    format: str
    trusted_digest: str | None = None
    executable: bool = True
    conversion_required: bool = False
    network_bytes: int | None = None
    local_copy_bytes: int | None = 0
    temporary_bytes: int | None = None
    resume_supported: bool = False
    identity_evidence: str = "unknown"
    owner_tool: str | None = None
    export_method: str | None = None
    duplicate_allocation_avoided: bool = False
    retirement_supported: bool = False
    quantization: str | None = None
    category: str | None = None
    cached_bytes: int = 0
    cache_verified: bool = False

    def __post_init__(self) -> None:
        if self.family not in FAMILIES:
            raise ValueError(f"invalid artifact family: {self.family}")
        _non_negative(self.network_bytes, "network_bytes")
        _non_negative(self.local_copy_bytes, "local_copy_bytes")
        _non_negative(self.temporary_bytes, "temporary_bytes")
        _non_negative(self.cached_bytes, "cached_bytes")
        if self.conversion_required and self.executable:
            raise ValueError("conversion-required offers are not executable in v1")

    @property
    def exact_group_key(self) -> tuple[str, str, str, str] | None:
        if not self.immutable_revision or not self.trusted_digest:
            return None
        return (
            self.immutable_revision,
            self.filename,
            self.format,
            self.trusted_digest,
        )


@dataclass(frozen=True)
class AcquisitionPlan:
    operation_id: str
    offer: AcquisitionOffer
    created_at: str


@dataclass(frozen=True)
class ProgressEvent:
    sequence: int
    kind: str
    operation_id: str
    transferred_bytes: int | None = None
    total_bytes: int | None = None
    message: str | None = None
    artifact_id: str | None = None
    schema_version: int = SCHEMA_VERSION

    def __post_init__(self) -> None:
        if self.kind not in EVENT_KINDS:
            raise ValueError(f"invalid event kind: {self.kind}")
        _non_negative(self.sequence, "sequence")
        _non_negative(self.transferred_bytes, "transferred_bytes")
        _non_negative(self.total_bytes, "total_bytes")


@dataclass(frozen=True)
class AcquisitionResult:
    operation_id: str
    provider: str
    record: LibraryRecord | None
    error_code: str | None = None
    message: str | None = None


@dataclass(frozen=True)
class MigrationValidation:
    identity: str = "not-run"
    catalog: str = "not-run"
    load: str = "not-run"
    inference: str = "not-run"
    workflow: str = "not-run"
    destination_digest: str | None = None
    message: str | None = None

    def __post_init__(self) -> None:
        allowed = {"not-run", "passed", "failed", "unavailable", "none", "weak"}
        for name in ("identity", "catalog", "load", "inference", "workflow"):
            if getattr(self, name) not in allowed:
                raise ValueError(f"invalid validation state for {name}")


@dataclass
class MigrationStep:
    step_id: str
    kind: str
    target: str
    rollback_kind: str | None
    state: str = "planned"
    created_by_operation: bool = False
    created_allocation_id: str | None = None
    created_size: int | None = None
    created_mtime_ns: int | None = None


@dataclass(frozen=True)
class MigrationPlan:
    plan_id: str
    source_artifact_id: str
    source_path: str
    source_sha256: str
    source_size: int
    source_mtime_ns: int
    destination_tool: str
    destination_version: str | None
    destination_root: str
    destination_native_id: str
    target_path: str | None
    method: str
    free_bytes: int
    temporary_bytes: int
    allocated_bytes: int
    validation_level: str
    capabilities: tuple[str, ...]
    created_at: str


@dataclass(frozen=True)
class MigrationResult:
    plan_id: str
    success: bool
    steps: tuple[MigrationStep, ...]
    validation: MigrationValidation
    retirement_eligible: bool = False
    confirmation_token: str | None = None
    error_code: str | None = None
    message: str | None = None


@dataclass
class ManualStep:
    step_id: str
    source_path: str
    destination_tool: str
    documented_action: str
    expected_reference: str
    resume_condition: str
    state: str = "pending"


@dataclass
class GuidedMigration:
    migration_id: str
    source_artifact_id: str
    source_path: str
    source_sha256: str
    destination_tool: str
    category: str | None
    state: str = "prepared"
    manual_step: ManualStep | None = None
    owned_config_path: str | None = None
    registration_created: bool = False
    config_allocation_id: str | None = None
    validation: MigrationValidation = field(default_factory=MigrationValidation)
    retirement_eligible: bool = False


@dataclass(frozen=True)
class PerformanceObservation:
    provider: str
    kind: str
    timestamp: str
    success: bool
    elapsed_seconds: float
    startup_seconds: float
    network_bytes: int = 0
    local_copy_bytes: int = 0
    failure_code: str | None = None
    network_seconds: float | None = None
    local_copy_seconds: float | None = None

    def __post_init__(self) -> None:
        if self.elapsed_seconds < 0 or self.startup_seconds < 0:
            raise ValueError("performance durations must be non-negative")
        _non_negative(self.network_bytes, "network_bytes")
        _non_negative(self.local_copy_bytes, "local_copy_bytes")
        if self.network_seconds is not None and self.network_seconds <= 0:
            raise ValueError("network_seconds must be positive")
        if self.local_copy_seconds is not None and self.local_copy_seconds <= 0:
            raise ValueError("local_copy_seconds must be positive")


@dataclass(frozen=True)
class RankedOffer:
    offer: AcquisitionOffer
    predicted_seconds: float | None
    adjusted_seconds: float | None
    sample_count: int
    observed_range: tuple[float, float] | None
    confidence: str
    reasons: tuple[str, ...]


@dataclass(frozen=True)
class RecoveryAction:
    operation_id: str
    action: str
    available: bool
    reason: str


@dataclass(frozen=True)
class RetirementPlan:
    plan_id: str
    owner_tool: str
    source_artifact_id: str
    source_path: str
    source_native_id: str
    source_sha256: str
    references_digest: str
    migration_plan_digest: str
    logical_bytes: int
    avoided_bytes: int
    estimated_reclaimable_bytes: int
    allocation_id: str | None
    created_at: str


@dataclass(frozen=True)
class RetirementToken:
    token: str
    plan_digest: str
    state_digest: str
    expires_at: float


@dataclass(frozen=True)
class RetirementResult:
    plan_id: str
    deleted_native_id: str
    logical_bytes: int
    avoided_bytes: int
    estimated_reclaimable_bytes: int
    measured_freed_bytes: int


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
