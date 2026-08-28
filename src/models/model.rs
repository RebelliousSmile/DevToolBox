//! Tolerant serde mirrors of the model-orchestrator schema-v1 protocol.

#![allow(dead_code)]

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ArtifactIdentity {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub algorithm: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolReference {
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub reference_id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub owner: bool,
    #[serde(default)]
    pub loaded: bool,
    #[serde(default)]
    pub workflow: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Protection {
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Artifact {
    #[serde(default)]
    pub artifact_id: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub identity: ArtifactIdentity,
    #[serde(default)]
    pub logical_size: Option<u64>,
    #[serde(default)]
    pub allocated_size: Option<u64>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub quantization: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub relationship: String,
    #[serde(default)]
    pub allocation_id: Option<String>,
    #[serde(default)]
    pub references: Vec<ToolReference>,
    #[serde(default)]
    pub protection: Protection,
    #[serde(default)]
    pub duplicate_group: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdapterCapabilities {
    #[serde(default)]
    pub inventory: bool,
    #[serde(default)]
    pub reference: bool,
    #[serde(default)]
    pub hard_link: bool,
    #[serde(default)]
    pub symbolic_link: bool,
    #[serde(default)]
    pub copy: bool,
    #[serde(default)]
    pub native_import: bool,
    #[serde(default)]
    pub load_validation: bool,
    #[serde(default)]
    pub inference_validation: bool,
    #[serde(default)]
    pub native_delete: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolInstallation {
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub detected: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub root_evidence: Vec<Value>,
    #[serde(default)]
    pub discovery_source: String,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub capabilities: AdapterCapabilities,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SourceError {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub confidence: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogSnapshot {
    pub schema_version: u64,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub installations: Vec<ToolInstallation>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub source_errors: Vec<SourceError>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AcquisitionOffer {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub locator: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub immutable_revision: Option<String>,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub trusted_digest: Option<String>,
    #[serde(default)]
    pub executable: bool,
    #[serde(default)]
    pub conversion_required: bool,
    #[serde(default)]
    pub network_bytes: Option<u64>,
    #[serde(default)]
    pub local_copy_bytes: Option<u64>,
    #[serde(default)]
    pub temporary_bytes: Option<u64>,
    #[serde(default)]
    pub resume_supported: bool,
    #[serde(default)]
    pub identity_evidence: String,
    #[serde(default)]
    pub owner_tool: Option<String>,
    #[serde(default)]
    pub export_method: Option<String>,
    #[serde(default)]
    pub duplicate_allocation_avoided: bool,
    #[serde(default)]
    pub retirement_supported: bool,
    #[serde(default)]
    pub quantization: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub cached_bytes: u64,
    #[serde(default)]
    pub cache_verified: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AcquisitionPlan {
    #[serde(default)]
    pub operation_id: String,
    #[serde(default)]
    pub offer: AcquisitionOffer,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProgressEvent {
    pub sequence: u64,
    pub kind: String,
    pub operation_id: String,
    #[serde(default)]
    pub transferred_bytes: Option<u64>,
    #[serde(default)]
    pub total_bytes: Option<u64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub artifact_id: Option<String>,
    pub schema_version: u64,
}

impl ProgressEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self.kind.as_str(), "completed" | "failed" | "cancelled")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MigrationValidation {
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub catalog: String,
    #[serde(default)]
    pub load: String,
    #[serde(default)]
    pub inference: String,
    #[serde(default)]
    pub workflow: String,
    #[serde(default)]
    pub destination_digest: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RecoveryAction {
    #[serde(default)]
    pub operation_id: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RetirementPlan {
    #[serde(default)]
    pub plan_id: String,
    #[serde(default)]
    pub owner_tool: String,
    #[serde(default)]
    pub source_artifact_id: String,
    #[serde(default)]
    pub source_path: String,
    #[serde(default)]
    pub source_native_id: String,
    #[serde(default)]
    pub source_sha256: String,
    #[serde(default)]
    pub references_digest: String,
    #[serde(default)]
    pub migration_plan_digest: String,
    #[serde(default)]
    pub logical_bytes: u64,
    #[serde(default)]
    pub avoided_bytes: u64,
    #[serde(default)]
    pub estimated_reclaimable_bytes: u64,
    #[serde(default)]
    pub allocation_id: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelSettings {
    #[serde(default)]
    pub library_root: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AcquisitionResult {
    #[serde(default)]
    pub operation_id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub record: Option<Value>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MigrationResult {
    #[serde(default)]
    pub plan_id: String,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub steps: Vec<Value>,
    #[serde(default)]
    pub validation: MigrationValidation,
    #[serde(default)]
    pub retirement_eligible: bool,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RetirementResult {
    #[serde(default)]
    pub plan_id: String,
    #[serde(default)]
    pub deleted_native_id: String,
    #[serde(default)]
    pub logical_bytes: u64,
    #[serde(default)]
    pub avoided_bytes: u64,
    #[serde(default)]
    pub estimated_reclaimable_bytes: u64,
    #[serde(default)]
    pub measured_freed_bytes: u64,
}
