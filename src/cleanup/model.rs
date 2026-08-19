//! Serde mirror of `scripts/winclean/common.py`'s JSON payloads.
//!
//! Only the fields the view consumes are modeled; everything is
//! `#[serde(default)]`-tolerant and nothing is `deny_unknown_fields`, so
//! the script may grow fields without breaking this side.
// Consumed by the « Nettoyage » view in a later part; keep the compiler
// quiet until then (same convention as `storage::commands` pre-wiring).
#![allow(dead_code)]

use serde::Deserialize;

/// One thing a module proposes to delete (`CleanCandidate.to_json_payload`).
///
/// `estimated_bytes: None` means *non-measurable*, never zero — that is the
/// script's single encoding of the state (see `common.py`). `path: None`
/// marks a pathless candidate reclaimed through an external command.
#[derive(Debug, Clone, Deserialize)]
pub struct Candidate {
    pub module: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub estimated_bytes: Option<u64>,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub needs_network: bool,
}

/// The plan document (`Plan.to_json_payload`), also present as the top
/// level of an apply payload.
#[derive(Debug, Clone, Deserialize)]
pub struct CleanupPlan {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub apply: bool,
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub total_estimated_bytes: Option<u64>,
    #[serde(default)]
    pub unpriced_modules: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<serde_json::Value>,
}

/// Per-module outcome of an `--apply` run (`CleanResult.to_json_payload`).
///
/// Contract caveat (verified against `common.py`): `failed` is a **byte
/// total** (`failed_total_bytes` at the report level), possibly `None` when
/// unmeasurable — never a failure count. Success is therefore judged on the
/// failure *lists* being empty, and a UI failure count comes from their
/// lengths, not from `failed`.
#[derive(Debug, Clone, Deserialize)]
pub struct ModuleResult {
    pub module: String,
    #[serde(default)]
    pub estimated: Option<u64>,
    #[serde(default)]
    pub freed: Option<u64>,
    #[serde(default)]
    pub failed: Option<u64>,
    #[serde(default)]
    pub measured: Option<u64>,
    #[serde(default)]
    pub locked_paths: Vec<String>,
    #[serde(default)]
    pub operation_failures: Vec<serde_json::Value>,
}

impl ModuleResult {
    /// `true` when nothing failed: no locked path, no external-operation
    /// failure. `failed` bytes deliberately play no part (see type docs).
    pub fn is_success(&self) -> bool {
        self.locked_paths.is_empty() && self.operation_failures.is_empty()
    }

    /// How many failures to show in the UI.
    pub fn failure_count(&self) -> usize {
        self.locked_paths.len() + self.operation_failures.len()
    }
}

/// The `"run"` object of an apply payload (`RunReport.to_json_payload`).
#[derive(Debug, Clone, Deserialize)]
pub struct RunPayload {
    /// `"completed"` or `"interrupted"` — an interrupted run must never be
    /// reported as a success, whatever its per-module results say.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub results: Vec<ModuleResult>,
}

impl RunPayload {
    pub fn is_interrupted(&self) -> bool {
        self.status == "interrupted"
    }
}
