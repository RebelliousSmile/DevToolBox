//! Typed models for DevToolBox configuration.
//!
//! Each struct maps field-for-field to `config/default.json`.  The JSON key
//! `default_settings` is used verbatim so (de)serialization does not silently
//! lose the settings block.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Display / behaviour settings stored under `default_settings` in JSON.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Settings {
    pub show_categories: bool,
    pub icon_size: u32,
    pub theme: String,
    pub launch_at_startup: bool,
    pub show_descriptions: bool,
    /// Age in days past which a stopped container / unused image / orphan
    /// volume is badged « dormant » in the Docker tab.
    ///
    /// The field-level default is **required**, not decorative: `Settings`
    /// carries no struct-level `#[serde(default)]`, so every `config.json`
    /// written before this field existed would otherwise fail to deserialize
    /// and drop the user into `fallback_config()`.
    #[serde(default = "default_dormant_after_days")]
    pub dormant_after_days: u32,
    /// Base directory for relative user-authored `@python` actions. Bundled
    /// Python tools keep their own distribution-root resolution.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_scripts_directory: String,
    /// Enables system window materials when platform and accessibility allow.
    #[serde(default = "default_native_effects")]
    pub native_effects: bool,
}

/// Two months, the threshold the user asked for.
fn default_dormant_after_days() -> u32 {
    60
}

fn default_native_effects() -> bool {
    true
}

/// A named group that commands can belong to.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub icon: String,
}

/// A single launchable command entry.
///
/// `shortcut` and `info` are optional — commands without them omit the key
/// in JSON (via `skip_serializing_if`) so the round-trip stays lossless.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub command: String,
    pub category: String,
    pub icon: String,
    pub is_favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_label: Option<String>,
    /// Whether this command's launch string should be resolved per-machine
    /// via the Part 1 `MachineCommands` mapping instead of using `command`
    /// as-is. Absent from JSON (existing configs) deserializes to `false`,
    /// preserving current behaviour for every pre-existing entry.
    #[serde(default)]
    pub machine_specific: bool,
    /// Optional free-text note shown as an "i" badge with a tooltip on the
    /// command's card. Absent from JSON (existing configs) deserializes to
    /// `None`, so no badge is drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
}

/// Top-level configuration wrapper.
///
/// The `version` field is kept as a raw `String` and preserved verbatim on
/// every save (no migration yet — see Decision D4 in the plan).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    pub version: String,
    pub default_settings: Settings,
    pub categories: Vec<Category>,
    pub commands: Vec<Command>,
    /// Absolute paths of the compose files the Docker tab remembers between
    /// runs (Part 2). Written by the `$HOME` scan and by « Oublier ».
    ///
    /// `skip_serializing_if` keeps the key out of `config/default.json` and
    /// out of every config belonging to a user who never opened the Docker
    /// tab: the shipped default must not carry machine-specific paths, and an
    /// empty array in everyone's config file would be noise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub docker_stacks: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A `config/default.json` **as it shipped before** `dormant_after_days`
    /// existed — kept here so tests are self-contained and do not depend on
    /// the working directory at test time, and doubling as the legacy-config
    /// fixture: every field added since must be `#[serde(default)]` for this
    /// to keep parsing. The current on-disk file is checked separately by
    /// `the_shipped_default_json_carries_the_dormancy_threshold`.
    const DEFAULT_JSON: &str = r#"{
  "version": "0.1.0",
  "default_settings": {
    "show_categories": true,
    "icon_size": 80,
    "theme": "light",
    "launch_at_startup": true,
    "show_descriptions": true
  },
  "categories": [
    {
      "id": "system",
      "name": "Système",
      "icon": "🖥️"
    },
    {
      "id": "network",
      "name": "Réseau",
      "icon": "🌐"
    },
    {
      "id": "maintenance",
      "name": "Maintenance",
      "icon": "⚙️"
    }
  ],
  "commands": [
    {
      "id": "notepad",
      "name": "Bloc-notes",
      "command": "notepad.exe",
      "category": "system",
      "icon": "📝",
      "is_favorite": true,
      "shortcut": "Ctrl+N"
    },
    {
      "id": "cmd",
      "name": "Invite de commandes",
      "command": "cmd.exe /c",
      "category": "system",
      "icon": "💻",
      "is_favorite": true
    },
    {
      "id": "ipconfig",
      "name": "Afficher l'adresse IP",
      "command": "ipconfig /all",
      "category": "network",
      "icon": "🌐",
      "is_favorite": true
    }
  ]
}"#;

    #[test]
    fn deserializes_default_json_exact_fields() {
        let config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");

        // Version
        assert_eq!(config.version, "0.1.0");

        // Settings exact fields
        let s = &config.default_settings;
        assert!(s.show_categories);
        assert_eq!(s.icon_size, 80u32);
        assert_eq!(s.theme, "light");
        assert!(s.launch_at_startup);
        assert!(s.show_descriptions);
        assert!(s.user_scripts_directory.is_empty());
        assert!(
            s.native_effects,
            "historical JSON enables native effects by default"
        );

        // Categories
        assert_eq!(config.categories.len(), 3);
        assert_eq!(config.categories[0].id, "system");
        assert_eq!(config.categories[1].id, "network");
        assert_eq!(config.categories[2].id, "maintenance");

        // Commands
        assert_eq!(config.commands.len(), 3);
        let notepad = &config.commands[0];
        assert_eq!(notepad.id, "notepad");
        assert_eq!(notepad.name, "Bloc-notes");
        assert_eq!(notepad.command, "notepad.exe");
        assert_eq!(notepad.category, "system");
        assert!(notepad.is_favorite);
        assert_eq!(notepad.shortcut, Some("Ctrl+N".to_string()));

        let cmd = &config.commands[1];
        assert_eq!(cmd.id, "cmd");
        assert!(cmd.is_favorite);
        // `cmd` has no shortcut key in JSON — must deserialize as None
        assert_eq!(cmd.shortcut, None);
    }

    // --- dormant_after_days (Docker dormancy threshold) ---------------------

    #[test]
    fn dormant_after_days_defaults_to_sixty_when_the_key_is_absent() {
        // A `config.json` written by a build predating the Docker dormancy
        // work must still load — the field carries its own serde default
        // because `Settings` has no struct-level `#[serde(default)]`.
        let config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");
        assert_eq!(config.default_settings.dormant_after_days, 60);
    }

    #[test]
    fn dormant_after_days_is_read_back_when_the_key_is_present() {
        let json = DEFAULT_JSON.replace(
            r#""show_descriptions": true"#,
            r#""show_descriptions": true,
    "dormant_after_days": 90"#,
        );
        let config: Config = serde_json::from_str(&json).expect("parse failed");
        assert_eq!(config.default_settings.dormant_after_days, 90);
    }

    #[test]
    fn dormant_after_days_survives_a_serde_round_trip_in_both_shapes() {
        for threshold in [1u32, 60, 3650] {
            let mut config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");
            config.default_settings.dormant_after_days = threshold;
            let serialized = serde_json::to_string(&config).expect("serialize failed");
            assert!(
                serialized.contains("dormant_after_days"),
                "the key must always be written back, never dropped"
            );
            let reloaded: Config = serde_json::from_str(&serialized).expect("re-parse failed");
            assert_eq!(reloaded, config, "round-trip must be lossless");
            assert_eq!(reloaded.default_settings.dormant_after_days, threshold);
        }
    }

    #[test]
    fn user_scripts_directory_defaults_empty_and_round_trips_when_configured() {
        let mut config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");
        assert!(config.default_settings.user_scripts_directory.is_empty());

        config.default_settings.user_scripts_directory = "/home/user/scripts".to_string();
        let serialized = serde_json::to_string(&config).expect("serialize failed");
        let reloaded: Config = serde_json::from_str(&serialized).expect("re-parse failed");
        assert_eq!(
            reloaded.default_settings.user_scripts_directory,
            "/home/user/scripts"
        );
    }

    #[test]
    fn the_shipped_default_json_carries_the_dormancy_threshold() {
        // The one test that reads the real file: `DEFAULT_JSON` above is
        // deliberately frozen at the pre-dormancy shape, so it cannot catch a
        // `config/default.json` that forgot the new key.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config")
            .join("default.json");
        let raw = std::fs::read_to_string(&path).expect("config/default.json must be readable");
        let config: Config = serde_json::from_str(&raw).expect("config/default.json must parse");
        assert_eq!(
            config.default_settings.dormant_after_days, 60,
            "the shipped default must match `default_dormant_after_days()`"
        );
    }

    // --- docker_stacks (Part 2 compose-file memory) -------------------------

    #[test]
    fn docker_stacks_defaults_to_empty_and_stays_out_of_the_json_when_it_is() {
        let config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");
        assert!(config.default_settings.dormant_after_days == 60);
        assert!(config.docker_stacks.is_empty());
        let serialized = serde_json::to_string(&config).expect("serialize failed");
        assert!(
            !serialized.contains("docker_stacks"),
            "an empty list must not appear in a user's config file"
        );
    }

    #[test]
    fn docker_stacks_round_trips_when_populated() {
        let mut config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");
        config.docker_stacks = vec![
            "/home/tnn/a/docker-compose.yml".to_string(),
            "/home/tnn/b/compose.yaml".to_string(),
        ];
        let serialized = serde_json::to_string(&config).expect("serialize failed");
        let reloaded: Config = serde_json::from_str(&serialized).expect("re-parse failed");
        assert_eq!(reloaded, config, "round-trip must be lossless");
    }

    #[test]
    fn shortcut_absent_stays_absent_on_roundtrip() {
        let config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");
        let serialized = serde_json::to_string(&config).expect("serialize failed");

        // The `cmd` command must not emit a `shortcut` key
        let value: serde_json::Value = serde_json::from_str(&serialized).expect("re-parse failed");
        let commands = value["commands"].as_array().expect("commands array");
        let cmd_entry = commands
            .iter()
            .find(|c| c["id"] == "cmd")
            .expect("cmd entry");
        assert!(
            cmd_entry.get("shortcut").is_none(),
            "shortcut key must be absent for commands without a shortcut"
        );

        // The `notepad` command MUST keep its shortcut
        let notepad_entry = commands
            .iter()
            .find(|c| c["id"] == "notepad")
            .expect("notepad entry");
        assert_eq!(notepad_entry["shortcut"], "Ctrl+N");
    }

    #[test]
    fn info_absent_stays_absent_and_present_roundtrips() {
        let mut config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");
        assert!(
            config.commands.iter().all(|c| c.info.is_none()),
            "a JSON config with no 'info' keys must deserialize every command with info: None"
        );

        config.commands[0].info = Some("Nécessite le VPN".to_string());
        let serialized = serde_json::to_string(&config).expect("serialize failed");
        let value: serde_json::Value = serde_json::from_str(&serialized).expect("re-parse failed");
        let commands = value["commands"].as_array().expect("commands array");

        let cmd_entry = commands
            .iter()
            .find(|c| c["id"] == "cmd")
            .expect("cmd entry");
        assert!(
            cmd_entry.get("info").is_none(),
            "info key must be absent for commands without an info note"
        );
        let notepad_entry = commands
            .iter()
            .find(|c| c["id"] == "notepad")
            .expect("notepad entry");
        assert_eq!(notepad_entry["info"], "Nécessite le VPN");
    }

    #[test]
    fn settings_deserializes_correctly() {
        let json = r#"{
            "version": "1.0.0",
            "default_settings": {
                "show_categories": false,
                "icon_size": 48,
                "theme": "dark",
                "launch_at_startup": false,
                "show_descriptions": false
            },
            "categories": [],
            "commands": []
        }"#;
        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert_eq!(config.default_settings.icon_size, 48u32);
        assert_eq!(config.default_settings.theme, "dark");
        assert!(!config.default_settings.show_categories);
        assert!(!config.default_settings.launch_at_startup);
        assert!(!config.default_settings.show_descriptions);
    }
}
