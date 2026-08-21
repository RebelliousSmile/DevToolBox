//! Pure, GDI-free favorite command operations for DevToolBox.
//!
//! ## Public surface
//!
//! - [`FavoriteError`]: typed error returned when `toggle_favorite` cannot find
//!   the requested command id.
//! - [`toggle_favorite`]: flip `is_favorite` for a single command and return
//!   the new state; persist via [`crate::storage::json::save`].
//! - [`CommandError`]: typed error returned by `add_command`/`update_command`/
//!   `remove_command` (mirrors [`crate::storage::categories::CategoryError`]).
//! - [`add_command`]: append a new command (rejects a duplicate id).
//! - [`update_command`]: replace an existing command in place by id.
//! - [`remove_command`]: remove a command by id.
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

// Staged favorite-toggle API (issue #9) and command CRUD (preferences config
// editor part 1): fully tested but not yet wired to the UI.
#![allow(dead_code)]

use crate::storage::categories::MoveDirection;
use crate::storage::models::{Command, Config};

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
// Command CRUD
// ---------------------------------------------------------------------------

/// Error type for command CRUD operations, mirroring
/// [`crate::storage::categories::CategoryError`]'s shape.
#[derive(Debug, PartialEq, Eq)]
pub enum CommandError {
    /// A command with this id already exists (returned by `add_command`).
    DuplicateId(String),
    /// No command with this id was found (returned by `update_command` /
    /// `remove_command`), or no group with this key was found (returned by
    /// `move_command_group` / `remove_command_group`).
    NotFound(String),
    /// The command targeted by `move_variant` has no `variant_group`, so it
    /// cannot be reordered relative to sibling variants.
    NotGrouped(String),
    /// A variant group's members are not stored contiguously in
    /// `config.commands`, so `move_command_group` cannot safely swap it as a
    /// block without risking corrupting an interleaved command/group. Should
    /// never happen in practice — groups are always inserted contiguously —
    /// but is checked defensively before any mutation.
    NotContiguous(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::DuplicateId(id) => {
                write!(f, "command id '{id}' already exists")
            }
            CommandError::NotFound(id) => {
                write!(f, "command id '{id}' not found")
            }
            CommandError::NotGrouped(id) => {
                write!(f, "command id '{id}' has no variant_group")
            }
            CommandError::NotContiguous(key) => {
                write!(f, "variant group '{key}' is not stored contiguously")
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// Append a new command to `config.commands`.
///
/// Returns `Err(CommandError::DuplicateId)` if a command with the same id
/// already exists, mirroring [`crate::storage::categories::add_category`].
pub fn add_command(config: &mut Config, command: Command) -> Result<(), CommandError> {
    if config.commands.iter().any(|c| c.id == command.id) {
        return Err(CommandError::DuplicateId(command.id));
    }
    config.commands.push(command);
    Ok(())
}

/// Replace the command matching `id` in place with `updated`.
///
/// Returns `Ok(())` on success or `Err(CommandError::NotFound)` when no
/// command with `id` exists. The config is left unchanged in that case.
pub fn update_command(config: &mut Config, id: &str, updated: Command) -> Result<(), CommandError> {
    match config.commands.iter_mut().find(|c| c.id == id) {
        Some(cmd) => {
            *cmd = updated;
            Ok(())
        }
        None => Err(CommandError::NotFound(id.to_string())),
    }
}

/// The bucket-neighbor search key for a "move" operation: a command's own
/// `variant_group` if it has one (every variant of a group shares that
/// group's single slot in the bucket), otherwise its own `id` (an ungrouped
/// command is its own atom). Shared by [`move_command`]/
/// [`move_command_group`] via [`move_atom`] so a lone command's move never
/// splits a neighboring group apart — it swaps with the whole block.
fn effective_key(c: &Command) -> String {
    c.variant_group.clone().unwrap_or_else(|| c.id.clone())
}

/// The contiguous `[start, end)` span in `config.commands` of every command
/// whose [`effective_key`] equals `key`. `Ok(None)` if nothing matches.
/// `Err(CommandError::NotContiguous)` if matches exist but are split apart
/// by other commands — see [`move_command_group`]'s doc comment for why
/// that should never happen in practice.
fn atom_span(config: &Config, key: &str) -> Result<Option<(usize, usize)>, CommandError> {
    let indices: Vec<usize> = config
        .commands
        .iter()
        .enumerate()
        .filter(|(_, c)| effective_key(c) == key)
        .map(|(i, _)| i)
        .collect();
    if indices.is_empty() {
        return Ok(None);
    }
    let start = indices[0];
    let end = indices[indices.len() - 1] + 1;
    if end - start != indices.len() {
        return Err(CommandError::NotContiguous(key.to_string()));
    }
    Ok(Some((start, end)))
}

/// Moves the bucket atom identified by `key` (a plain command id for an
/// ungrouped command, or a `variant_group` value for a whole group) one
/// slot up or down within its category bucket, swapping it as a block with
/// its nearest same-bucket neighbor atom — which may itself be a lone
/// command or another group. Shared implementation behind [`move_command`]
/// and [`move_command_group`]; see the latter's doc comment for the bucket
/// and contiguity semantics.
fn move_atom(config: &mut Config, key: &str, direction: MoveDirection) -> Result<(), CommandError> {
    let Some((start, end)) = atom_span(config, key)? else {
        return Err(CommandError::NotFound(key.to_string()));
    };

    let known_ids: std::collections::HashSet<&str> =
        config.categories.iter().map(|c| c.id.as_str()).collect();
    let bucket_of = |cmd: &Command| -> Option<String> {
        known_ids
            .contains(cmd.category.as_str())
            .then(|| cmd.category.clone())
    };
    let target_bucket = bucket_of(&config.commands[start]);

    // Both branches swap the moving atom's block with its nearest
    // same-bucket neighbor's block as a whole, leaving any "junk" (other-
    // bucket items) sandwiched between them in the same relative order but
    // *not* the same absolute index — the junk block moves along with
    // whichever side of it shifts, exactly like exchanging two books on a
    // shelf and letting the books between them slide over. A plain
    // `rotate_left` over the enclosing span does NOT implement this: for two
    // length-1 atoms it produces `[junk, moving, neighbor]` instead of the
    // desired `[moving, junk, neighbor]`. Splicing the three reordered
    // blocks back in is correct regardless of block-length differences
    // (single command vs. multi-variant group).
    match direction {
        MoveDirection::Up => {
            let mut i = start;
            let mut neighbor_last = None;
            while i > 0 {
                i -= 1;
                if bucket_of(&config.commands[i]) == target_bucket {
                    neighbor_last = Some(i);
                    break;
                }
            }
            let Some(n_last) = neighbor_last else {
                return Ok(());
            };
            let n_key = effective_key(&config.commands[n_last]);
            let (n_start, n_end) = atom_span(config, &n_key)?.unwrap_or((n_last, n_last + 1));
            let moving: Vec<Command> = config.commands[start..end].to_vec();
            let junk: Vec<Command> = config.commands[n_end..start].to_vec();
            let neighbor: Vec<Command> = config.commands[n_start..n_end].to_vec();
            let mut new_span = moving;
            new_span.extend(junk);
            new_span.extend(neighbor);
            config.commands.splice(n_start..end, new_span);
        }
        MoveDirection::Down => {
            let mut i = end;
            let mut neighbor_start = None;
            while i < config.commands.len() {
                if bucket_of(&config.commands[i]) == target_bucket {
                    neighbor_start = Some(i);
                    break;
                }
                i += 1;
            }
            let Some(n_start) = neighbor_start else {
                return Ok(());
            };
            let n_key = effective_key(&config.commands[n_start]);
            let (n_start, n_end) = atom_span(config, &n_key)?.unwrap_or((n_start, n_start + 1));
            let moving: Vec<Command> = config.commands[start..end].to_vec();
            let junk: Vec<Command> = config.commands[end..n_start].to_vec();
            let neighbor: Vec<Command> = config.commands[n_start..n_end].to_vec();
            let mut new_span = neighbor;
            new_span.extend(junk);
            new_span.extend(moving);
            config.commands.splice(start..n_end, new_span);
        }
    }

    Ok(())
}

/// Move a command one slot up or down *within its category bucket*,
/// swapping it with its nearest same-bucket neighbor in `config.commands`.
/// Mirrors [`crate::storage::categories::move_category`]'s semantics —
/// display order (both in Préférences and Actions) is exactly
/// `config.commands`'s order, there is no dedicated ordering field.
///
/// The bucket a command belongs to is the same one
/// [`crate::storage::categories::group_commands_by_category`] would put it
/// in: its own category id if that id exists in `config.categories`,
/// otherwise the synthetic "Sans catégorie" bucket. Only same-bucket
/// commands are considered as neighbors, so moving an action never crosses
/// into another category's list — reassigning category is a separate
/// operation (`update_command`).
///
/// If the nearest same-bucket neighbor belongs to a variant group, the
/// whole group moves as one block rather than just that one variant — this
/// keeps a lone command's move from ever splitting a group's variants
/// apart in storage (see [`move_atom`]).
///
/// A no-op (returns `Ok(())` without mutating anything) if the command is
/// already first/last within its bucket in the requested direction.
///
/// Returns `Err(CommandError::NotFound)` when no command with `id` exists.
pub fn move_command(
    config: &mut Config,
    id: &str,
    direction: MoveDirection,
) -> Result<(), CommandError> {
    let pos = config
        .commands
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;
    let key = effective_key(&config.commands[pos]);
    move_atom(config, &key, direction)
}

/// Move a variant one slot up or down *within its own variant group*,
/// swapping it with its nearest same-group neighbor in `config.commands`.
/// Mirrors [`move_command`]'s neighbor-search/swap approach, but the bucket
/// is the variant group itself rather than the category — this is what lets
/// a Préférences group row reorder its variants without letting the move
/// escape into a sibling command/group of the same category.
///
/// Returns `Err(CommandError::NotFound)` when no command with `id` exists,
/// or `Err(CommandError::NotGrouped)` when it exists but has no
/// `variant_group`. A no-op (returns `Ok(())`) if already first/last within
/// the group in the requested direction.
pub fn move_variant(
    config: &mut Config,
    id: &str,
    direction: MoveDirection,
) -> Result<(), CommandError> {
    let pos = config
        .commands
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;

    let Some(group_key) = config.commands[pos].variant_group.clone() else {
        return Err(CommandError::NotGrouped(id.to_string()));
    };
    let same_group = |c: &Command| c.variant_group.as_deref() == Some(group_key.as_str());

    let neighbor = match direction {
        MoveDirection::Up => (0..pos).rev().find(|&i| same_group(&config.commands[i])),
        MoveDirection::Down => {
            ((pos + 1)..config.commands.len()).find(|&i| same_group(&config.commands[i]))
        }
    };

    if let Some(neighbor) = neighbor {
        config.commands.swap(pos, neighbor);
    }

    Ok(())
}

/// Move an entire variant group one slot up or down *within its category
/// bucket*, as a single block, relative to its nearest same-bucket sibling
/// (a lone command or another group). This is the group-row equivalent of
/// [`move_command`] — same bucket semantics (skips other-bucket neighbors),
/// but swaps a variable-length contiguous block instead of a single index.
///
/// Requires the group's members to be physically contiguous in
/// `config.commands` (always true in practice — variants are only ever
/// inserted as a contiguous run); returns
/// `Err(CommandError::NotContiguous)` rather than risk corrupting an
/// interleaved neighbor if that invariant is ever violated.
///
/// Returns `Err(CommandError::NotFound)` when no command has this
/// `variant_group`. A no-op if the group is already first/last within its
/// bucket in the requested direction.
pub fn move_command_group(
    config: &mut Config,
    group_key: &str,
    direction: MoveDirection,
) -> Result<(), CommandError> {
    move_atom(config, group_key, direction)
}

/// Remove every command belonging to `group_key` (i.e. sharing that
/// `variant_group`) from `config.commands` — the group-row equivalent of
/// [`remove_command`], deleting a whole app's variants in one call.
///
/// Returns `Ok(count)` with the number of variants removed, or
/// `Err(CommandError::NotFound)` when no command has this `variant_group`
/// (the config is left unchanged in that case).
pub fn remove_command_group(config: &mut Config, group_key: &str) -> Result<usize, CommandError> {
    let before = config.commands.len();
    config
        .commands
        .retain(|c| c.variant_group.as_deref() != Some(group_key));
    let removed = before - config.commands.len();
    if removed == 0 {
        return Err(CommandError::NotFound(group_key.to_string()));
    }
    Ok(removed)
}

/// Remove the command matching `id` from `config.commands`.
///
/// Returns `Ok(())` on success or `Err(CommandError::NotFound)` when no
/// command with `id` exists.
pub fn remove_command(config: &mut Config, id: &str) -> Result<(), CommandError> {
    let pos = config
        .commands
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| CommandError::NotFound(id.to_string()))?;

    config.commands.remove(pos);
    Ok(())
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
            dormant_after_days: 60,
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
            info: None,
        }
    }

    fn two_command_config() -> Config {
        Config {
            docker_stacks: Vec::new(),
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
            docker_stacks: Vec::new(),
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
                info: None,
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

    // -----------------------------------------------------------------------
    // add_command
    // -----------------------------------------------------------------------

    #[test]
    fn add_command_appends() {
        let mut config = two_command_config();
        let new_cmd = make_command("calc", false);
        add_command(&mut config, new_cmd).expect("add failed");

        assert_eq!(config.commands.len(), 3);
        assert_eq!(config.commands[2].id, "calc");
    }

    #[test]
    fn add_command_rejects_duplicate_id() {
        let mut config = two_command_config();
        let dup = make_command("notepad", false);
        let result = add_command(&mut config, dup);
        assert_eq!(
            result,
            Err(CommandError::DuplicateId("notepad".to_string()))
        );
        // Original command count unchanged.
        assert_eq!(config.commands.len(), 2);
    }

    // -----------------------------------------------------------------------
    // update_command
    // -----------------------------------------------------------------------

    #[test]
    fn update_command_replaces_fields_in_place() {
        let mut config = two_command_config();
        let mut updated = make_command("notepad", false);
        updated.name = "Notepad Renamed".to_string();
        updated.command = "notepad2.exe".to_string();

        update_command(&mut config, "notepad", updated).expect("update failed");

        let cmd = config.commands.iter().find(|c| c.id == "notepad").unwrap();
        assert_eq!(cmd.name, "Notepad Renamed");
        assert_eq!(cmd.command, "notepad2.exe");
        // Command count unchanged.
        assert_eq!(config.commands.len(), 2);
    }

    #[test]
    fn update_command_returns_error_for_unknown_id() {
        let mut config = two_command_config();
        let updated = make_command("ghost", false);
        let result = update_command(&mut config, "ghost", updated);
        assert_eq!(result, Err(CommandError::NotFound("ghost".to_string())));
    }

    // -----------------------------------------------------------------------
    // move_command
    // -----------------------------------------------------------------------

    /// Two categories ("system", "network") plus two orphan commands whose
    /// `category` strings differ ("ghost-a"/"ghost-b") but both fall
    /// outside `categories` — they must still be treated as one bucket
    /// ("Sans catégorie"), same as `group_commands_by_category` would.
    ///
    /// Layout: [a(system), x(network), b(system), o1(ghost-a), o2(ghost-b)]
    fn bucket_test_config() -> Config {
        let mut a = make_command("a", false);
        a.category = "system".to_string();
        let mut x = make_command("x", false);
        x.category = "network".to_string();
        let mut b = make_command("b", false);
        b.category = "system".to_string();
        let mut o1 = make_command("o1", false);
        o1.category = "ghost-a".to_string();
        let mut o2 = make_command("o2", false);
        o2.category = "ghost-b".to_string();

        Config {
            docker_stacks: Vec::new(),
            version: "0.1.0".to_string(),
            default_settings: make_settings(),
            categories: vec![
                Category {
                    id: "system".to_string(),
                    name: "Système".to_string(),
                    icon: String::new(),
                },
                Category {
                    id: "network".to_string(),
                    name: "Réseau".to_string(),
                    icon: String::new(),
                },
            ],
            commands: vec![a, x, b, o1, o2],
        }
    }

    fn ids(config: &Config) -> Vec<&str> {
        config.commands.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn move_command_up_skips_over_a_different_bucket_neighbor() {
        let mut config = bucket_test_config();
        // "b" (system) must swap with "a" (system), skipping "x" (network).
        move_command(&mut config, "b", MoveDirection::Up).expect("move failed");
        assert_eq!(ids(&config), vec!["b", "x", "a", "o1", "o2"]);
    }

    #[test]
    fn move_command_down_skips_over_a_different_bucket_neighbor() {
        let mut config = bucket_test_config();
        // "a" (system) must swap with "b" (system), skipping "x" (network).
        move_command(&mut config, "a", MoveDirection::Down).expect("move failed");
        assert_eq!(ids(&config), vec!["b", "x", "a", "o1", "o2"]);
    }

    #[test]
    fn move_command_up_on_first_in_bucket_is_a_no_op() {
        let mut config = bucket_test_config();
        move_command(&mut config, "a", MoveDirection::Up).expect("move failed");
        assert_eq!(ids(&config), vec!["a", "x", "b", "o1", "o2"]);
    }

    #[test]
    fn move_command_down_on_last_in_bucket_is_a_no_op() {
        let mut config = bucket_test_config();
        move_command(&mut config, "b", MoveDirection::Down).expect("move failed");
        assert_eq!(ids(&config), vec!["a", "x", "b", "o1", "o2"]);
    }

    #[test]
    fn move_command_treats_orphans_with_different_category_strings_as_one_bucket() {
        let mut config = bucket_test_config();
        // o1 (category "ghost-a") and o2 (category "ghost-b") both fall
        // outside `categories` — they must be swappable with each other.
        move_command(&mut config, "o2", MoveDirection::Up).expect("move failed");
        assert_eq!(ids(&config), vec!["a", "x", "b", "o2", "o1"]);
    }

    #[test]
    fn move_command_returns_error_for_unknown_id() {
        let mut config = bucket_test_config();
        let result = move_command(&mut config, "ghost", MoveDirection::Up);
        assert_eq!(result, Err(CommandError::NotFound("ghost".to_string())));
    }

    // -----------------------------------------------------------------------
    // remove_command
    // -----------------------------------------------------------------------

    #[test]
    fn remove_command_deletes_the_command() {
        let mut config = two_command_config();
        remove_command(&mut config, "notepad").expect("remove failed");

        assert_eq!(config.commands.len(), 1);
        assert!(!config.commands.iter().any(|c| c.id == "notepad"));
    }

    #[test]
    fn remove_command_returns_error_for_unknown_id() {
        let mut config = two_command_config();
        let result = remove_command(&mut config, "ghost");
        assert_eq!(result, Err(CommandError::NotFound("ghost".to_string())));
        // Command count unchanged.
        assert_eq!(config.commands.len(), 2);
    }

    // -----------------------------------------------------------------------
    // move_variant / move_command_group / remove_command_group
    // -----------------------------------------------------------------------

    fn make_variant(id: &str, group: &str) -> Command {
        let mut c = make_command(id, false);
        c.variant_group = Some(group.to_string());
        c.group_name = Some(group.to_string());
        c.variant_label = Some(id.to_string());
        c
    }

    /// Layout: [g1a, g1b, g1c (group "g1"), single "s" (system),
    /// g2a, g2b (group "g2"), o1 (orphan, "Sans catégorie")] — all system
    /// except the trailing orphan, so bucket order is [g1, s, g2, o1].
    fn grouped_test_config() -> Config {
        let g1a = make_variant("g1a", "g1");
        let g1b = make_variant("g1b", "g1");
        let g1c = make_variant("g1c", "g1");
        let s = make_command("s", false);
        let g2a = make_variant("g2a", "g2");
        let g2b = make_variant("g2b", "g2");
        let mut o1 = make_command("o1", false);
        o1.category = "ghost".to_string();

        Config {
            docker_stacks: Vec::new(),
            version: "0.1.0".to_string(),
            default_settings: make_settings(),
            categories: vec![Category {
                id: "system".to_string(),
                name: "Système".to_string(),
                icon: String::new(),
            }],
            commands: vec![g1a, g1b, g1c, s, g2a, g2b, o1],
        }
    }

    #[test]
    fn move_variant_down_swaps_with_next_sibling_only() {
        let mut config = grouped_test_config();
        move_variant(&mut config, "g1a", MoveDirection::Down).expect("move failed");
        assert_eq!(
            ids(&config),
            vec!["g1b", "g1a", "g1c", "s", "g2a", "g2b", "o1"]
        );
    }

    #[test]
    fn move_variant_up_on_first_in_group_is_a_no_op() {
        let mut config = grouped_test_config();
        move_variant(&mut config, "g1a", MoveDirection::Up).expect("move failed");
        assert_eq!(
            ids(&config),
            vec!["g1a", "g1b", "g1c", "s", "g2a", "g2b", "o1"]
        );
    }

    #[test]
    fn move_variant_down_on_last_in_group_is_a_no_op() {
        let mut config = grouped_test_config();
        move_variant(&mut config, "g1c", MoveDirection::Down).expect("move failed");
        assert_eq!(
            ids(&config),
            vec!["g1a", "g1b", "g1c", "s", "g2a", "g2b", "o1"]
        );
    }

    #[test]
    fn move_variant_returns_not_grouped_for_an_ungrouped_command() {
        let mut config = grouped_test_config();
        let result = move_variant(&mut config, "s", MoveDirection::Up);
        assert_eq!(result, Err(CommandError::NotGrouped("s".to_string())));
    }

    #[test]
    fn move_variant_returns_not_found_for_unknown_id() {
        let mut config = grouped_test_config();
        let result = move_variant(&mut config, "ghost", MoveDirection::Up);
        assert_eq!(result, Err(CommandError::NotFound("ghost".to_string())));
    }

    #[test]
    fn move_command_group_down_swaps_the_whole_block_with_the_next_bucket_neighbor() {
        let mut config = grouped_test_config();
        // "g1" must move past "s" as one block, preserving internal order.
        move_command_group(&mut config, "g1", MoveDirection::Down).expect("move failed");
        assert_eq!(
            ids(&config),
            vec!["s", "g1a", "g1b", "g1c", "g2a", "g2b", "o1"]
        );
    }

    #[test]
    fn move_command_group_up_swaps_the_whole_block_with_the_previous_bucket_neighbor() {
        let mut config = grouped_test_config();
        // "g2" must move past "s" as one block, preserving internal order.
        move_command_group(&mut config, "g2", MoveDirection::Up).expect("move failed");
        assert_eq!(
            ids(&config),
            vec!["g1a", "g1b", "g1c", "g2a", "g2b", "s", "o1"]
        );
    }

    #[test]
    fn move_command_group_down_can_swap_past_another_group_as_a_block() {
        let mut config = grouped_test_config();
        move_command_group(&mut config, "g1", MoveDirection::Down).expect("first move failed");
        // g1 is now after "s"; move it again to swap past "g2".
        move_command_group(&mut config, "g1", MoveDirection::Down).expect("second move failed");
        assert_eq!(
            ids(&config),
            vec!["s", "g2a", "g2b", "g1a", "g1b", "g1c", "o1"]
        );
    }

    #[test]
    fn move_command_group_up_on_first_group_in_bucket_is_a_no_op() {
        let mut config = grouped_test_config();
        move_command_group(&mut config, "g1", MoveDirection::Up).expect("move failed");
        assert_eq!(
            ids(&config),
            vec!["g1a", "g1b", "g1c", "s", "g2a", "g2b", "o1"]
        );
    }

    #[test]
    fn move_command_group_returns_error_for_unknown_group() {
        let mut config = grouped_test_config();
        let result = move_command_group(&mut config, "ghost-group", MoveDirection::Up);
        assert_eq!(
            result,
            Err(CommandError::NotFound("ghost-group".to_string()))
        );
    }

    #[test]
    fn move_command_group_returns_not_contiguous_when_members_are_split() {
        let mut config = grouped_test_config();
        // Break "g1"'s contiguity by swapping one member past "s".
        config.commands.swap(1, 3); // g1b <-> s
        let result = move_command_group(&mut config, "g1", MoveDirection::Down);
        assert_eq!(result, Err(CommandError::NotContiguous("g1".to_string())));
    }

    #[test]
    fn remove_command_group_removes_every_variant_and_returns_the_count() {
        let mut config = grouped_test_config();
        let removed = remove_command_group(&mut config, "g1").expect("remove failed");
        assert_eq!(removed, 3);
        assert_eq!(ids(&config), vec!["s", "g2a", "g2b", "o1"]);
    }

    #[test]
    fn remove_command_group_returns_error_for_unknown_group() {
        let mut config = grouped_test_config();
        let result = remove_command_group(&mut config, "ghost-group");
        assert_eq!(
            result,
            Err(CommandError::NotFound("ghost-group".to_string()))
        );
        assert_eq!(config.commands.len(), 7);
    }
}
