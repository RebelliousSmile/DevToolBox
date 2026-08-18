//! Client for `scripts/winclean/clean.py`'s `--json` contract.
//!
//! Everything the future « Nettoyage » view consumes and nothing visual: a
//! serde model of the plan and run payloads, a stdout parser tolerant to
//! multiple concatenated JSON documents, a per-module aggregation, and
//! background spawn helpers mirroring `crate::applications::spawn_report`.
//! No scan or deletion logic lives in Rust — the Python script is the
//! single source of truth for both.

// Nothing consumes the facade until Part 3 wires the view; keep the
// re-exports ready without tripping `-D warnings` in CI.
#![allow(unused_imports)]

pub mod model;
pub mod parse;
pub mod rows;
pub mod spawn;

pub use model::{Candidate, CleanupPlan, ModuleResult, RunPayload};
pub use parse::{parse_output, Payload};
pub use rows::{module_rows, ModuleRow};
pub use spawn::{spawn_analyze, spawn_clean, CleanupEvent};
