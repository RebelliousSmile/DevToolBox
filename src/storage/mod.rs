//! Storage module — typed JSON persistence for DevToolBox configuration.
//!
//! Public surface:
//! - [`models`]: serde structs aligned with `config/default.json`.
//! - [`json`]: `load()` / `save()` + [`json::StorageError`].
//! - [`categories`]: pure category grouping + CRUD API (issue #6).
//! - [`commands`]: pure favorite-toggle op (issue #7) + command CRUD API
//!   (preferences config editor, part 1).
//! - [`slug`]: collision-free id generation from a display name (preferences
//!   config editor, part 1).
//! - [`machine_commands`]: per-machine command mapping model + load/save,
//!   mirroring the `config.json` pattern but persisted to a separate,
//!   non-synced file (`platform::machine_commands_path()`).
//!
//! Re-exports for ergonomic use from other modules:
//! ```rust,ignore
//! use crate::storage::{Config, Command, load, save, StorageError};
//! use crate::storage::{group_commands_by_category, CategoryGroup};
//! use crate::storage::{add_category, rename_category, remove_category};
//! use crate::storage::{toggle_favorite, FavoriteError};
//! use crate::storage::{add_command, update_command, remove_command, CommandError};
//! use crate::storage::generate_slug;
//! use crate::storage::{MachineCommands, MachineCommandsError};
//! use crate::storage::{load_machine_commands_from, save_machine_commands_to};
//! ```

// Thin re-export facade for the staged storage API (issues #6/#7/#9); several
// re-exports below are not yet consumed by the UI.
#![allow(unused_imports)]

pub mod categories;
pub mod commands;
pub mod json;
pub mod machine_commands;
pub mod models;
pub mod slug;

pub use categories::{
    add_category, group_commands_by_category, move_category, remove_category, rename_category,
    CategoryError, CategoryGroup, MoveDirection,
};
pub use commands::{
    add_command, move_command, move_command_group, move_variant, remove_command,
    remove_command_group, toggle_favorite, update_command, CommandError, FavoriteError,
};
pub use json::{load, save, StorageError};
pub use machine_commands::{
    load_machine_commands_from, resolve_command, save_machine_commands_to, CommandResolution,
    MachineCommands, MachineCommandsError,
};
pub use models::{Category, Command, Config, Settings};
pub use slug::generate_slug;
