//! Storage module — typed JSON persistence for WinFXStart configuration.
//!
//! Public surface:
//! - [`models`]: serde structs aligned with `config/default.json`.
//! - [`json`]: `load()` / `save()` + [`json::StorageError`].
//! - [`categories`]: pure category grouping + CRUD API (issue #6).
//!
//! Re-exports for ergonomic use from other modules:
//! ```rust,ignore
//! use crate::storage::{Config, Command, load, save, StorageError};
//! use crate::storage::{group_commands_by_category, CategoryGroup};
//! use crate::storage::{add_category, rename_category, remove_category};
//! ```

pub mod categories;
pub mod json;
pub mod models;

pub use categories::{
    add_category, group_commands_by_category, remove_category, rename_category, CategoryError,
    CategoryGroup,
};
pub use json::{load, save, StorageError};
pub use models::{Category, Command, Config, Settings};
