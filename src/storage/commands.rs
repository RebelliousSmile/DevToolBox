//! Pure, GDI-free favorite command operations for DevToolBox.
//!
//! ## Public surface
//!
//! - [`FavoriteError`]: typed error returned when `toggle_favorite` cannot find
//!   the requested command id.
//! - [`toggle_favorite`]: flip `is_favorite` for a single command and return
//!   the new state; persist via [`crate::storage::json::save`].
//!
//! ## Deferred interactive toggle seam (issue #9)
//!
//! The function below is the callable API seam for the future settings /
//! alias-editor UI (issue #9).  The interactive toggle widget is deferred.
//! Callers should:
//! ```rust,ignore
//! use crate::storage::{toggle_favorite, save};
//! let new_state = toggle_favorite(&mut config, "notepad")?;
//! save(&config)?;
//! // Then call UiHost::reload() to refresh the visible grid.
//! ```

// Staged favorite-toggle API (issue #9): fully tested but not yet wired to the UI.
#![allow(dead_code)]

use crate::storage::models::Config;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error returned by [`toggle_favorite`] when the requested command id is not
/// found in the config.
#[derive(Debug, PartialEq, Eq)]
pub enum FavoriteError {
    /// No command with this id exists in `config.commands`.
    NotFound(String),
}

impl std::fmt::Display for FavoriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FavoriteError::NotFound(id) => {
                write!(f, "command id '{id}' not found")
            }
        }
    }
}

impl std::error::Error for FavoriteError {}

// ---------------------------------------------------------------------------
// toggle_favorite
// ---------------------------------------------------------------------------

/// Flip the `is_favorite` flag of the command identified by `command_id`.
///
/// Returns `Ok(new_state)` where `new_state` is the value of `is_favorite`
/// AFTER the flip (i.e. the opposite of what it was before).
///
/// Returns `Err(FavoriteError::NotFound)` when no command in
/// `config.commands` has the given id; the config is left unchanged in that
/// case.
///
/// This is a pure model operation — the caller is responsible for calling
/// [`crate::storage::save`] (or [`crate::storage::json::save_to`] in tests)
/// to persist the change, and for calling `UiHost::reload()` to refresh the
/// visible grid.
pub fn toggle_favorite(config: &mut Config, command_id: &str) -> Result<bool, FavoriteError> {
    let cmd = config
        .commands
        .iter_mut()
        .find(|c| c.id == command_id)
        .ok_or_else(|| FavoriteError::NotFound(command_id.to_string()))?;

    cmd.is_favorite = !cmd.is_favorite;
    Ok(cmd.is_favorite)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::json::{load_from, save_to};
    use crate::storage::models::{Category, Command, Config, Settings};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_settings() -> Settings {
        Settings {
            show_categories: false,
            icon_size: 80,
            theme: "light".to_string(),
            launch_at_startup: false,
            show_descriptions: true,
        }
    }

    fn make_command(id: &str, is_favorite: bool) -> Command {
        Command {
            id: id.to_string(),
            name: id.to_string(),
            command: format!("{id}.exe"),
            category: "system".to_string(),
            icon: String::new(),
            is_favorite,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: false,
        }
    }

    fn two_command_config() -> Config {
        Config {
            version: "0.1.0".to_string(),
            default_settings: make_settings(),
            categories: vec![Category {
                id: "system".to_string(),
                name: "Système".to_string(),
                icon: String::new(),
            }],
            commands: vec![make_command("notepad", true), make_command("paint", false)],
        }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "devtoolbox_fav_{}_{tag}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
            .join("config.json")
    }

    // -----------------------------------------------------------------------
    // toggle_favorite — flip semantics
    // -----------------------------------------------------------------------

    #[test]
    fn toggle_true_to_false_returns_false() {
        let mut config = two_command_config();
        // notepad starts as is_favorite = true
        let new_state = toggle_favorite(&mut config, "notepad").expect("toggle failed");
        assert!(!new_state, "returned state must be the new (flipped) value");
        assert!(
            !config
                .commands
                .iter()
                .find(|c| c.id == "notepad")
                .unwrap()
                .is_favorite,
            "is_favorite must be false after toggle"
        );
    }

    #[test]
    fn toggle_false_to_true_returns_true() {
        let mut config = two_command_config();
        // paint starts as is_favorite = false
        let new_state = toggle_favorite(&mut config, "paint").expect("toggle failed");
        assert!(new_state, "returned state must be the new (flipped) value");
        assert!(
            config
                .commands
                .iter()
                .find(|c| c.id == "paint")
                .unwrap()
                .is_favorite,
            "is_favorite must be true after toggle"
        );
    }

    #[test]
    fn toggle_only_affects_target_command() {
        let mut config = two_command_config();
        let paint_before = config
            .commands
            .iter()
            .find(|c| c.id == "paint")
            .unwrap()
            .is_favorite;
        toggle_favorite(&mut config, "notepad").expect("toggle failed");
        let paint_after = config
            .commands
            .iter()
            .find(|c| c.id == "paint")
            .unwrap()
            .is_favorite;
        assert_eq!(
            paint_before, paint_after,
            "other commands must not be affected"
        );
    }

    // -----------------------------------------------------------------------
    // toggle_favorite — unknown id returns NotFound; config unchanged
    // -----------------------------------------------------------------------

    #[test]
    fn unknown_id_returns_not_found() {
        let mut config = two_command_config();
        let result = toggle_favorite(&mut config, "ghost");
        assert_eq!(
            result,
            Err(FavoriteError::NotFound("ghost".to_string())),
            "missing id must return NotFound"
        );
    }

    #[test]
    fn config_unchanged_on_not_found() {
        let mut config = two_command_config();
        let original = config.clone();
        let _ = toggle_favorite(&mut config, "ghost");
        assert_eq!(
            config, original,
            "config must be unchanged when id is not found"
        );
    }

    // -----------------------------------------------------------------------
    // toggle_favorite — double-toggle idempotence
    // -----------------------------------------------------------------------

    #[test]
    fn double_toggle_restores_original_state() {
        let mut config = two_command_config();
        let original_fav = config
            .commands
            .iter()
            .find(|c| c.id == "notepad")
            .unwrap()
            .is_favorite;

        toggle_favorite(&mut config, "notepad").expect("first toggle failed");
        toggle_favorite(&mut config, "notepad").expect("second toggle failed");

        let restored_fav = config
            .commands
            .iter()
            .find(|c| c.id == "notepad")
            .unwrap()
            .is_favorite;
        assert_eq!(
            restored_fav, original_fav,
            "two consecutive toggles must restore the original is_favorite value"
        );
    }

    #[test]
    fn double_toggle_returns_correct_states() {
        let mut config = two_command_config();
        // notepad starts true; first toggle → false, second toggle → true
        let first = toggle_favorite(&mut config, "notepad").expect("first toggle failed");
        let second = toggle_favorite(&mut config, "notepad").expect("second toggle failed");
        assert!(!first, "first toggle from true must return false");
        assert!(second, "second toggle back to true must return true");
    }

    // -----------------------------------------------------------------------
    // toggle_favorite — save_to → load_from round-trip (AC2 JSON side)
    // -----------------------------------------------------------------------

    #[test]
    fn toggle_round_trip_persists_new_is_favorite() {
        let mut config = two_command_config();
        // notepad starts true; toggle → false
        toggle_favorite(&mut config, "notepad").expect("toggle failed");

        let path = temp_path("toggle_rt");
        save_to(&config, &path).expect("save_to failed");
        let loaded = load_from(&path).expect("load_from failed");

        // Round-trip must be lossless.
        assert_eq!(loaded, config, "round-trip must be lossless");

        // The toggled value must be persisted.
        let notepad = loaded.commands.iter().find(|c| c.id == "notepad").unwrap();
        assert!(
            !notepad.is_favorite,
            "toggled is_favorite (false) must be persisted after save_to→load_from"
        );

        // version must be preserved.
        assert_eq!(loaded.version, "0.1.0", "version must be preserved");

        // Untouched command must be unchanged.
        let paint = loaded.commands.iter().find(|c| c.id == "paint").unwrap();
        assert!(
            !paint.is_favorite,
            "paint's is_favorite must remain unchanged (false)"
        );

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn toggle_round_trip_lossless_with_all_fields() {
        let mut config = Config {
            version: "0.1.0".to_string(),
            default_settings: make_settings(),
            categories: vec![Category {
                id: "sys".to_string(),
                name: "System".to_string(),
                icon: "🖥️".to_string(),
            }],
            commands: vec![Command {
                id: "notepad".to_string(),
                name: "Bloc-notes".to_string(),
                command: "notepad.exe".to_string(),
                category: "sys".to_string(),
                icon: "📝".to_string(),
                is_favorite: true,
                shortcut: Some("Ctrl+N".to_string()),
                variant_group: None,
                group_name: None,
                variant_label: None,
                machine_specific: false,
            }],
        };

        toggle_favorite(&mut config, "notepad").expect("toggle failed");

        let path = temp_path("toggle_full_rt");
        save_to(&config, &path).expect("save_to failed");
        let loaded = load_from(&path).expect("load_from failed");

        assert_eq!(loaded, config, "full round-trip must be lossless");
        assert!(
            !loaded.commands[0].is_favorite,
            "toggled is_favorite must survive save→load"
        );
        assert_eq!(
            loaded.commands[0].shortcut,
            Some("Ctrl+N".to_string()),
            "shortcut must be preserved"
        );

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
