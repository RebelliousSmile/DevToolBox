//! Typed models for WinFXStart configuration.
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
/// `shortcut` is optional — commands without a shortcut omit the key in JSON
/// (via `skip_serializing_if`) so the round-trip stays lossless.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub command: String,
    pub category: String,
    pub icon: String,
    pub is_favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The literal content of `config/default.json` — kept here so tests are
    /// self-contained and do not depend on the working directory at test time.
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

    #[test]
    fn shortcut_absent_stays_absent_on_roundtrip() {
        let config: Config = serde_json::from_str(DEFAULT_JSON).expect("parse failed");
        let serialized = serde_json::to_string(&config).expect("serialize failed");

        // The `cmd` command must not emit a `shortcut` key
        let value: serde_json::Value =
            serde_json::from_str(&serialized).expect("re-parse failed");
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
