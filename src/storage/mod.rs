//! Storage module — typed JSON persistence for DevToolBox configuration.
//!
//! Public surface:
//! - [`models`]: serde structs aligned with `config/default.json`.
//! - [`json`]: `load()` / `save()` + [`json::StorageError`].
//! - [`categories`]: pure category grouping + CRUD API (issue #6).
//! - [`commands`]: pure favorite-toggle op (issue #7).
//!
//! Re-exports for ergonomic use from other modules:
//! ```rust,ignore
//! use crate::storage::{Config, Command, load, save, StorageError};
//! use crate::storage::{group_commands_by_category, CategoryGroup};
//! use crate::storage::{add_category, rename_category, remove_category};
//! use crate::storage::{toggle_favorite, FavoriteError};
//! ```

// Thin re-export facade for the staged storage API (issues #6/#7/#9); several
// re-exports below are not yet consumed by the UI.
#![allow(unused_imports)]

pub mod categories;
pub mod commands;
pub mod json;
pub mod models;

pub use categories::{
    add_category, group_commands_by_category, remove_category, rename_category, CategoryError,
    CategoryGroup,
};
pub use commands::{toggle_favorite, FavoriteError};
pub use json::{load, save, StorageError};
pub use models::{Category, Command, Config, Settings};
