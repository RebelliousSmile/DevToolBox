//! Storage module — typed JSON persistence for WinFXStart configuration.
//!
//! Public surface:
//! - [`models`]: serde structs aligned with `config/default.json`.
//! - [`json`]: `load()` / `save()` + [`json::StorageError`].
//!
//! Re-exports for ergonomic use from other modules:
//! ```rust,ignore
//! use crate::storage::{Config, Command, load, save, StorageError};
//! ```

pub mod json;
pub mod models;

pub use json::{load, save, StorageError};
pub use models::{Category, Command, Config, Settings};
