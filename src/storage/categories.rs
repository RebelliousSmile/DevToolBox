//! Pure, GDI-free category core for WinFXStart.
//!
//! ## Public surface
//!
//! - [`CategoryGroup`]: a category (or the synthetic Uncategorized bucket) and
//!   the commands that belong to it, borrowing from [`Config`].
//! - [`group_commands_by_category`]: groups commands by config-declared category
//!   order, with a trailing synthetic Uncategorized bucket for orphan commands.
//! - [`add_category`]: append a new category (rejects duplicate ids).
//! - [`rename_category`]: rename an existing category by id.
//! - [`remove_category`]: remove a category and clear the `category` id of
//!   every command that referenced it (orphan-safe, Decision D3).
//!
//! ## Deferred CRUD UI seam (issue #9)
//!
//! The functions above are the callable API seam documented in Decision D2.
//! Interactive create/rename/delete widgets are deferred to the settings /
//! alias-editor issue (#9). Callers in future settings code should import:
//! ```rust,ignore
//! use crate::storage::{add_category, rename_category, remove_category};
//! ```
//! then call `crate::storage::save(&config)` to persist the mutation.

// Staged category CRUD API (issue #9): fully tested but not yet wired to the UI.
#![allow(dead_code)]

use crate::storage::models::{Category, Command, Config};

// ---------------------------------------------------------------------------
// CategoryGroup
// ---------------------------------------------------------------------------

/// One bucket in the grouped display.
///
/// `category` is `Some(&Category)` for a declared category and `None` for the
/// synthetic Uncategorized bucket (commands whose `category` id is empty or
/// does not match any declared category).
///
/// The Uncategorized bucket is NEVER persisted (Decision D4) and NEVER added
/// to `config.categories`.
#[derive(Debug)]
pub struct CategoryGroup<'a> {
    /// The declared category, or `None` for the Uncategorized bucket.
    pub category: Option<&'a Category>,
    /// Commands belonging to this group, in their original config order.
    pub commands: Vec<&'a Command>,
}

// ---------------------------------------------------------------------------
// Grouping
// ---------------------------------------------------------------------------

/// Group every command under its declared category, in config category order.
///
/// Commands whose `category` id is empty or does not match any declared
/// category are collected into a trailing synthetic Uncategorized group
/// (`category: None`).  If there are no orphan commands, no Uncategorized
/// group is emitted.
///
/// Every command appears in exactly one group.
pub fn group_commands_by_category(config: &Config) -> Vec<CategoryGroup<'_>> {
    // Build an index: category_id -> position in output vec for fast lookup.
    let mut groups: Vec<CategoryGroup<'_>> = config
        .categories
        .iter()
        .map(|cat| CategoryGroup {
            category: Some(cat),
            commands: Vec::new(),
        })
        .collect();

    // Map each category id to its index in `groups`.
    let id_to_idx: std::collections::HashMap<&str, usize> = config
        .categories
        .iter()
        .enumerate()
        .map(|(i, cat)| (cat.id.as_str(), i))
        .collect();

    // Bucket every command; orphans go into the uncategorized vec.
    let mut uncategorized: Vec<&Command> = Vec::new();
    for cmd in &config.commands {
        if let Some(&idx) = id_to_idx.get(cmd.category.as_str()) {
            groups[idx].commands.push(cmd);
        } else {
            uncategorized.push(cmd);
        }
    }

    // Append synthetic Uncategorized bucket only when there are orphans.
    if !uncategorized.is_empty() {
        groups.push(CategoryGroup {
            category: None,
            commands: uncategorized,
        });
    }

    groups
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

/// Error type for category CRUD operations.
#[derive(Debug, PartialEq, Eq)]
pub enum CategoryError {
    /// A category with this id already exists (returned by `add_category`).
    DuplicateId(String),
    /// No category with this id was found (returned by `rename_category` /
    /// `remove_category`).
    NotFound(String),
}

impl std::fmt::Display for CategoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CategoryError::DuplicateId(id) => {
                write!(f, "category id '{id}' already exists")
            }
            CategoryError::NotFound(id) => {
                write!(f, "category id '{id}' not found")
            }
        }
    }
}

impl std::error::Error for CategoryError {}

/// Append a new category to `config.categories`.
///
/// Returns `Err(CategoryError::DuplicateId)` if a category with the same id
/// already exists (Decision D2: reject on duplicate).
pub fn add_category(
    config: &mut Config,
    id: impl Into<String>,
    name: impl Into<String>,
    icon: impl Into<String>,
) -> Result<(), CategoryError> {
    let id = id.into();
    if config.categories.iter().any(|c| c.id == id) {
        return Err(CategoryError::DuplicateId(id));
    }
    config.categories.push(Category {
        id,
        name: name.into(),
        icon: icon.into(),
    });
    Ok(())
}

/// Rename an existing category in place.
///
/// Returns `Ok(())` on success or `Err(CategoryError::NotFound)` when no
/// category with `id` exists.
pub fn rename_category(
    config: &mut Config,
    id: &str,
    new_name: impl Into<String>,
) -> Result<(), CategoryError> {
    let new_name = new_name.into();
    match config.categories.iter_mut().find(|c| c.id == id) {
        Some(cat) => {
            cat.name = new_name;
            Ok(())
        }
        None => Err(CategoryError::NotFound(id.to_string())),
    }
}

/// Remove a category and clear the `category` id of every affected command.
///
/// Commands are NOT deleted (Decision D3): their `category` id is set to an
/// empty string so they re-bucket as Uncategorized on the next
/// [`group_commands_by_category`] call.
///
/// Returns `Ok(())` on success or `Err(CategoryError::NotFound)` when no
/// category with `id` exists.
pub fn remove_category(config: &mut Config, id: &str) -> Result<(), CategoryError> {
    let pos = config
        .categories
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| CategoryError::NotFound(id.to_string()))?;

    config.categories.remove(pos);

    // Clear the category id of every command that referenced the removed
    // category so they fall into the synthetic Uncategorized bucket.
    for cmd in config.commands.iter_mut() {
        if cmd.category == id {
            cmd.category = String::new();
        }
    }

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
            show_categories: true,
            icon_size: 80,
            theme: "light".to_string(),
            launch_at_startup: false,
            show_descriptions: true,
        }
    }

    fn make_category(id: &str, name: &str) -> Category {
        Category {
            id: id.to_string(),
            name: name.to_string(),
            icon: String::new(),
        }
    }

    fn make_command(id: &str, category: &str, is_favorite: bool) -> Command {
        Command {
            id: id.to_string(),
            name: id.to_string(),
            command: format!("{id}.exe"),
            category: category.to_string(),
            icon: String::new(),
            is_favorite,
            shortcut: None,
                variant_group: None,
                group_name: None,
                variant_label: None,
        }
    }

    fn three_category_config() -> Config {
        Config {
            version: "0.1.0".to_string(),
            default_settings: make_settings(),
            categories: vec![
                make_category("system", "Système"),
                make_category("network", "Réseau"),
                make_category("maintenance", "Maintenance"),
            ],
            commands: vec![
                make_command("notepad", "system", true),
                make_command("cmd", "system", true),
                make_command("ipconfig", "network", true),
                make_command("ping", "network", false),
                make_command("defrag", "maintenance", false),
            ],
        }
    }

    // -----------------------------------------------------------------------
    // group_commands_by_category
    // -----------------------------------------------------------------------

    #[test]
    fn groups_follow_config_category_order() {
        let config = three_category_config();
        let groups = group_commands_by_category(&config);

        // Three declared categories, no orphans → three groups, no Uncategorized.
        assert_eq!(groups.len(), 3, "expected exactly 3 groups");
        assert_eq!(groups[0].category.unwrap().id, "system");
        assert_eq!(groups[1].category.unwrap().id, "network");
        assert_eq!(groups[2].category.unwrap().id, "maintenance");
    }

    #[test]
    fn commands_appear_in_their_category() {
        let config = three_category_config();
        let groups = group_commands_by_category(&config);

        let system_ids: Vec<&str> = groups[0].commands.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(system_ids, vec!["notepad", "cmd"]);

        let network_ids: Vec<&str> = groups[1].commands.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(network_ids, vec!["ipconfig", "ping"]);

        let maint_ids: Vec<&str> = groups[2].commands.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(maint_ids, vec!["defrag"]);
    }

    #[test]
    fn orphan_command_lands_in_trailing_uncategorized_bucket() {
        let mut config = three_category_config();
        // Add a command with an unknown category id.
        config
            .commands
            .push(make_command("orphan", "unknown_cat", false));

        let groups = group_commands_by_category(&config);

        // 3 declared categories + 1 Uncategorized bucket.
        assert_eq!(groups.len(), 4);
        let unc = &groups[3];
        assert!(unc.category.is_none(), "last group must be Uncategorized");
        assert_eq!(unc.commands.len(), 1);
        assert_eq!(unc.commands[0].id, "orphan");
    }

    #[test]
    fn empty_category_id_is_uncategorized() {
        let mut config = three_category_config();
        // Command with empty category string.
        config.commands.push(make_command("blank_cat", "", false));

        let groups = group_commands_by_category(&config);
        let unc = groups.last().unwrap();
        assert!(unc.category.is_none());
        assert!(unc.commands.iter().any(|c| c.id == "blank_cat"));
    }

    #[test]
    fn no_uncategorized_bucket_when_all_commands_are_assigned() {
        let config = three_category_config();
        let groups = group_commands_by_category(&config);

        // No command is unassigned → no Uncategorized bucket.
        for g in &groups {
            assert!(
                g.category.is_some(),
                "unexpected Uncategorized bucket when all commands assigned"
            );
        }
    }

    #[test]
    fn all_commands_appear_exactly_once() {
        let mut config = three_category_config();
        config.commands.push(make_command("orphan", "ghost", false));

        let groups = group_commands_by_category(&config);
        let mut seen: Vec<&str> = groups
            .iter()
            .flat_map(|g| g.commands.iter().map(|c| c.id.as_str()))
            .collect();
        seen.sort_unstable();

        let mut expected: Vec<&str> = config.commands.iter().map(|c| c.id.as_str()).collect();
        expected.sort_unstable();

        assert_eq!(seen, expected, "every command must appear exactly once");
    }

    #[test]
    fn empty_config_produces_no_groups() {
        let config = Config {
            version: "0.1.0".to_string(),
            default_settings: make_settings(),
            categories: Vec::new(),
            commands: Vec::new(),
        };
        let groups = group_commands_by_category(&config);
        assert!(groups.is_empty());
    }

    // -----------------------------------------------------------------------
    // add_category
    // -----------------------------------------------------------------------

    #[test]
    fn add_category_appends() {
        let mut config = three_category_config();
        add_category(&mut config, "dev", "Dev", "").expect("add failed");

        assert_eq!(config.categories.len(), 4);
        assert_eq!(config.categories[3].id, "dev");
        assert_eq!(config.categories[3].name, "Dev");
    }

    #[test]
    fn add_category_rejects_duplicate_id() {
        let mut config = three_category_config();
        let result = add_category(&mut config, "system", "Doublon", "");
        assert_eq!(
            result,
            Err(CategoryError::DuplicateId("system".to_string()))
        );
        // Original category count unchanged.
        assert_eq!(config.categories.len(), 3);
    }

    // -----------------------------------------------------------------------
    // rename_category
    // -----------------------------------------------------------------------

    #[test]
    fn rename_category_renames_in_place() {
        let mut config = three_category_config();
        rename_category(&mut config, "network", "Réseau Étendu").expect("rename failed");

        let cat = config
            .categories
            .iter()
            .find(|c| c.id == "network")
            .unwrap();
        assert_eq!(cat.name, "Réseau Étendu");
    }

    #[test]
    fn rename_category_returns_error_for_unknown_id() {
        let mut config = three_category_config();
        let result = rename_category(&mut config, "ghost", "Fantôme");
        assert_eq!(result, Err(CategoryError::NotFound("ghost".to_string())));
    }

    // -----------------------------------------------------------------------
    // remove_category
    // -----------------------------------------------------------------------

    #[test]
    fn remove_category_removes_and_clears_commands() {
        let mut config = three_category_config();
        // "system" has notepad + cmd assigned to it.
        remove_category(&mut config, "system").expect("remove failed");

        // Category is gone.
        assert!(!config.categories.iter().any(|c| c.id == "system"));

        // Commands still present, category id cleared.
        let notepad = config.commands.iter().find(|c| c.id == "notepad").unwrap();
        let cmd = config.commands.iter().find(|c| c.id == "cmd").unwrap();
        assert_eq!(notepad.category, "", "category id must be cleared");
        assert_eq!(cmd.category, "", "category id must be cleared");
    }

    #[test]
    fn remove_category_commands_rebucket_as_uncategorized() {
        let mut config = three_category_config();
        remove_category(&mut config, "system").expect("remove failed");

        let groups = group_commands_by_category(&config);

        // The Uncategorized bucket must exist and contain notepad + cmd.
        let unc = groups
            .iter()
            .find(|g| g.category.is_none())
            .expect("Uncategorized bucket must exist after remove");
        let unc_ids: Vec<&str> = unc.commands.iter().map(|c| c.id.as_str()).collect();
        assert!(
            unc_ids.contains(&"notepad"),
            "notepad must be Uncategorized"
        );
        assert!(unc_ids.contains(&"cmd"), "cmd must be Uncategorized");
    }

    #[test]
    fn remove_category_returns_error_for_unknown_id() {
        let mut config = three_category_config();
        let result = remove_category(&mut config, "ghost");
        assert_eq!(result, Err(CategoryError::NotFound("ghost".to_string())));
    }

    #[test]
    fn remove_category_does_not_delete_commands() {
        let mut config = three_category_config();
        let original_command_count = config.commands.len();
        remove_category(&mut config, "maintenance").expect("remove failed");

        assert_eq!(
            config.commands.len(),
            original_command_count,
            "commands must never be deleted by remove_category"
        );
    }

    // -----------------------------------------------------------------------
    // Persistence round-trip (AC3 — mirrors issue #3 test style)
    // -----------------------------------------------------------------------

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "winfxstart_cat_{}_{tag}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
            .join("config.json")
    }

    #[test]
    fn add_category_round_trip() {
        let mut config = three_category_config();
        add_category(&mut config, "dev", "Dev Tools", "💻").expect("add failed");

        let path = temp_path("add");
        save_to(&config, &path).expect("save_to failed");
        let loaded = load_from(&path).expect("load_from failed");

        assert_eq!(loaded, config, "round-trip must be lossless");
        assert_eq!(loaded.version, "0.1.0", "version must be preserved");
        assert!(loaded.categories.iter().any(|c| c.id == "dev"));

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn rename_category_round_trip() {
        let mut config = three_category_config();
        rename_category(&mut config, "network", "LAN").expect("rename failed");

        let path = temp_path("rename");
        save_to(&config, &path).expect("save_to failed");
        let loaded = load_from(&path).expect("load_from failed");

        assert_eq!(loaded, config, "round-trip must be lossless");
        let cat = loaded
            .categories
            .iter()
            .find(|c| c.id == "network")
            .unwrap();
        assert_eq!(cat.name, "LAN");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn remove_category_round_trip() {
        let mut config = three_category_config();
        remove_category(&mut config, "system").expect("remove failed");

        let path = temp_path("remove");
        save_to(&config, &path).expect("save_to failed");
        let loaded = load_from(&path).expect("load_from failed");

        assert_eq!(loaded, config, "round-trip must be lossless");
        assert!(!loaded.categories.iter().any(|c| c.id == "system"));
        // Commands cleared of system id must persist with empty category.
        let notepad = loaded.commands.iter().find(|c| c.id == "notepad").unwrap();
        assert_eq!(notepad.category, "");

        // Confirm no synthetic Uncategorized category was persisted.
        let raw = std::fs::read_to_string(&path).expect("read failed");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("re-parse");
        let cats = value["categories"].as_array().unwrap();
        assert!(
            !cats.iter().any(|c| c["id"] == "uncategorized"),
            "synthetic Uncategorized must never be persisted"
        );

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
