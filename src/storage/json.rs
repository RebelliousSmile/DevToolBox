//! JSON persistence for DevToolBox configuration.
//!
//! - `load()`: reads the user config file (`platform::config_path()` —
//!   `%APPDATA%\DevToolBox\config.json` on Windows,
//!   `$XDG_CONFIG_HOME/devtoolbox/config.json` on Linux); falls back to the
//!   bundled default (cwd then exe-dir) when the user file is absent —
//!   `config/default.json` on Windows/other, `config/default.linux.json` on
//!   Linux (the Windows default's `notepad.exe`/`cmd.exe`/`ipconfig` commands
//!   don't exist there).
//! - `save()`: writes the typed [`Config`] to `%APPDATA%\DevToolBox\config.json`,
//!   creating the directory if needed.  The `version` field is preserved as-is.
//!
//! Neither function panics.  Path-injectable variants (`load_from` / `save_to`)
//! are provided so unit tests can operate in a temp directory without touching
//! the real `%APPDATA%`.

// The write side (`save` / `save_to`) is tested but not yet wired to the UI.
#![allow(dead_code)]

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::storage::models::{Category, Command, Config};

#[derive(Deserialize)]
struct BuiltinActions {
    #[serde(default)]
    categories: Vec<Category>,
    #[serde(default)]
    commands: Vec<Command>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// All failure modes from the storage layer.
#[derive(Debug)]
pub enum StorageError {
    /// An I/O error occurred while reading or writing a file.
    Io(std::io::Error),
    /// JSON (de)serialization failed.
    Parse(serde_json::Error),
    /// Neither the user config nor the bundled default could be found.
    NoConfigFound,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::Io(e) => write!(f, "storage I/O error: {e}"),
            StorageError::Parse(e) => write!(f, "storage parse error: {e}"),
            StorageError::NoConfigFound => {
                write!(
                    f,
                    "no config found (user file and bundled default both missing)"
                )
            }
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StorageError::Io(e) => Some(e),
            StorageError::Parse(e) => Some(e),
            StorageError::NoConfigFound => None,
        }
    }
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        StorageError::Io(e)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(e: serde_json::Error) -> Self {
        StorageError::Parse(e)
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Returns the user configuration file path, delegated to
/// [`crate::platform::config_path`] (Windows: `%APPDATA%\DevToolBox\config.json`;
/// Linux: `$XDG_CONFIG_HOME/devtoolbox/config.json`, falling back to
/// `~/.config/devtoolbox/config.json`).
///
/// Always returns `Some` — `platform::config_path()` returns a bare
/// `PathBuf` with its own OS-specific fallback for the unset-env-var case
/// (falls back to a temp dir on Windows rather than returning `None`), so
/// this wrapper never observes an absent path. Kept `Option`-returning for
/// source compatibility with existing callers in this file.
pub fn user_config_path() -> Option<PathBuf> {
    Some(crate::platform::config_path())
}

/// Returns the path to the bundled default config using the same candidates
/// as the former `load_config()` in `app.rs`:
/// 1. Relative to the current working directory.
/// 2. Relative to the executable directory.
///
/// Picks `config/default.linux.json` on Linux (real, Ubuntu-LTS-validated
/// commands — `notepad.exe`/`cmd.exe`/`ipconfig` in `default.json` don't
/// exist there) and `config/default.json` everywhere else.
pub fn default_config_path() -> Option<PathBuf> {
    bundled_config_path(default_config_filename())
}

#[cfg(target_os = "linux")]
fn default_config_filename() -> &'static str {
    "default.linux.json"
}

#[cfg(target_os = "macos")]
fn default_config_filename() -> &'static str {
    "default.macos.json"
}

#[cfg(windows)]
fn default_config_filename() -> &'static str {
    "default.json"
}

fn bundled_config_path(filename: &str) -> Option<PathBuf> {
    let relative = PathBuf::from("config").join(filename);
    if relative.exists() {
        return Some(relative);
    }

    let mut roots = Vec::new();
    if let Ok(current) = std::env::current_dir() {
        roots.push(current);
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    roots.into_iter().find_map(|root| {
        root.ancestors()
            .map(|ancestor| ancestor.join("config").join(filename))
            .find(|candidate| candidate.is_file())
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load configuration.
///
/// Priority:
/// 1. `%APPDATA%\DevToolBox\config.json` (user file).
/// 2. Bundled `config/default.json` (cwd or exe-dir).
///
/// Returns [`StorageError::NoConfigFound`] if neither is present.
pub fn load() -> Result<Config, StorageError> {
    // 1. Try the user file.
    if let Some(user_path) = user_config_path() {
        if user_path.exists() {
            log::info!("Loading config from user file: {}", user_path.display());
            return merge_builtin_actions(load_from(&user_path)?);
        }
    }

    // 2. Fall back to the bundled default.
    if let Some(default_path) = default_config_path() {
        log::info!(
            "User config absent — loading bundled default: {}",
            default_path.display()
        );
        return merge_builtin_actions(load_from(&default_path)?);
    }

    log::warn!("No config file found (user file and bundled default both missing)");
    Err(StorageError::NoConfigFound)
}

fn merge_builtin_actions(mut config: Config) -> Result<Config, StorageError> {
    let Some(path) = bundled_config_path("builtin-actions.json") else {
        return Ok(config);
    };
    let text = std::fs::read_to_string(&path)?;
    let builtins: BuiltinActions = serde_json::from_str(&text)?;

    for category in builtins.categories {
        if !config
            .categories
            .iter()
            .any(|existing| existing.id == category.id)
        {
            config.categories.push(category);
        }
    }
    for command in builtins.commands {
        if !config
            .commands
            .iter()
            .any(|existing| existing.id == command.id)
        {
            config.commands.push(command);
        }
    }
    log::info!("Merged built-in actions from {}", path.display());
    Ok(config)
}

/// Load configuration from an explicit path.
///
/// Used by unit tests to avoid touching `%APPDATA%`.
pub fn load_from(path: &Path) -> Result<Config, StorageError> {
    let text = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&text)?;
    Ok(config)
}

/// Save configuration to `%APPDATA%\DevToolBox\config.json`.
///
/// Creates the directory if it does not exist.
/// Returns [`StorageError`] (never panics) when `APPDATA` is unset or an I/O
/// / serialization error occurs.
pub fn save(config: &Config) -> Result<(), StorageError> {
    let path = user_config_path().ok_or_else(|| {
        StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "APPDATA environment variable is not set",
        ))
    })?;
    save_to(config, &path)
}

/// Save configuration to an explicit path.
///
/// Creates parent directories as needed.
/// Used by unit tests to avoid touching `%APPDATA%`.
pub fn save_to(config: &Config, path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(path, json)?;
    log::info!("Config saved to {}", path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{Category, Command, Config, Settings};

    fn sample_config() -> Config {
        Config {
            docker_stacks: Vec::new(),
            version: "0.1.0".to_string(),
            default_settings: Settings {
                show_categories: true,
                icon_size: 80,
                theme: "light".to_string(),
                launch_at_startup: true,
                show_descriptions: true,
                dormant_after_days: 60,
                user_scripts_directory: String::new(),
            },
            categories: vec![Category {
                id: "system".to_string(),
                name: "Système".to_string(),
                icon: "🖥️".to_string(),
            }],
            commands: vec![
                Command {
                    id: "notepad".to_string(),
                    name: "Bloc-notes".to_string(),
                    command: "notepad.exe".to_string(),
                    category: "system".to_string(),
                    icon: "📝".to_string(),
                    is_favorite: true,
                    shortcut: Some("Ctrl+N".to_string()),
                    variant_group: None,
                    group_name: None,
                    variant_label: None,
                    machine_specific: false,
                    info: None,
                },
                Command {
                    id: "cmd".to_string(),
                    name: "Invite de commandes".to_string(),
                    command: "cmd.exe /c".to_string(),
                    category: "system".to_string(),
                    icon: "💻".to_string(),
                    is_favorite: true,
                    shortcut: None,
                    variant_group: None,
                    group_name: None,
                    variant_label: None,
                    machine_specific: false,
                    info: None,
                },
            ],
        }
    }

    #[test]
    fn round_trip_lossless_with_version_preserved() {
        let original = sample_config();
        let temp_dir = std::env::temp_dir().join(format!(
            "devtoolbox_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let path = temp_dir.join("config.json");

        save_to(&original, &path).expect("save_to failed");
        let loaded = load_from(&path).expect("load_from failed");

        assert_eq!(loaded, original, "round-trip must be lossless");
        assert_eq!(loaded.version, "0.1.0", "version must be preserved");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn shortcut_none_absent_after_roundtrip() {
        let original = sample_config();
        let temp_dir = std::env::temp_dir().join(format!(
            "devtoolbox_shortcut_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let path = temp_dir.join("config.json");

        save_to(&original, &path).expect("save_to failed");

        // Read raw JSON to confirm `shortcut` key is absent for `cmd`
        let raw = std::fs::read_to_string(&path).expect("read failed");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("re-parse failed");
        let commands = value["commands"].as_array().expect("commands array");
        let cmd_entry = commands
            .iter()
            .find(|c| c["id"] == "cmd")
            .expect("cmd entry");
        assert!(
            cmd_entry.get("shortcut").is_none(),
            "shortcut must be absent in serialized JSON for commands without a shortcut"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn fallback_when_user_file_absent() {
        // A path that definitely does not exist — no user file present.
        let nonexistent = std::env::temp_dir().join("devtoolbox_absent_99999999/config.json");
        assert!(!nonexistent.exists(), "precondition: file must not exist");

        // load_from on a missing path returns an Io error, not a panic.
        let result = load_from(&nonexistent);
        assert!(
            matches!(result, Err(StorageError::Io(_))),
            "expected StorageError::Io for missing file, got: {:?}",
            result
        );
    }

    #[test]
    fn parse_error_on_malformed_json() {
        let temp_dir = std::env::temp_dir().join(format!(
            "devtoolbox_malformed_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).expect("create temp dir");
        let path = temp_dir.join("bad.json");
        std::fs::write(&path, b"{ not valid json !!!").expect("write failed");

        let result = load_from(&path);
        assert!(
            matches!(result, Err(StorageError::Parse(_))),
            "expected StorageError::Parse for malformed JSON, got: {:?}",
            result
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn save_creates_parent_directory() {
        let temp_base = std::env::temp_dir().join(format!(
            "devtoolbox_mkdirs_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        // Use a two-level nested path that doesn't exist yet.
        let path = temp_base.join("nested").join("config.json");
        assert!(!path.exists(), "precondition: path must not exist");

        let config = sample_config();
        save_to(&config, &path).expect("save_to must create directories and write");
        assert!(path.exists(), "file must be created");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_base);
    }

    #[test]
    fn user_config_path_contains_devtoolbox() {
        // Delegated to `platform::config_path()`, which always returns
        // `Some` now (see `user_config_path`'s doc comment) — no env-var
        // skip branch needed anymore.
        let path = user_config_path().expect("user_config_path must always return Some");
        let display = path.display().to_string();
        // Directory-name casing is OS-specific: `DevToolBox` on Windows
        // (`platform::windows::config_path`), lowercase `devtoolbox` on
        // Linux (`platform::linux::config_path`, XDG convention).
        #[cfg(windows)]
        assert!(
            display.contains("DevToolBox"),
            "user config path must contain DevToolBox, got: {display}"
        );
        #[cfg(target_os = "linux")]
        assert!(
            display.contains("devtoolbox"),
            "user config path must contain devtoolbox, got: {display}"
        );
        assert!(
            display.ends_with("config.json"),
            "user config path must end with config.json, got: {display}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn default_config_path_resolves_to_the_real_linux_bundled_file() {
        let path = default_config_path().expect("default_config_path must find a bundled file");
        assert!(
            path.ends_with("config/default.linux.json"),
            "expected the Linux-specific default, got: {}",
            path.display()
        );
        assert!(path.is_file(), "resolved path must exist for real");
        let config = load_from(&path).expect("default.linux.json must parse as a valid Config");
        assert!(
            !config.commands.is_empty(),
            "default.linux.json must ship at least one real command"
        );
    }

    #[test]
    fn all_shipped_defaults_parse_and_keep_schema_version() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("config");
        for filename in ["default.json", "default.linux.json", "default.macos.json"] {
            let config = load_from(&root.join(filename)).expect("shipped default must parse");
            assert_eq!(config.version, "0.1.0", "{filename} schema changed");
            let serialized = serde_json::to_string(&config).expect("serialize default");
            let round_trip: Config = serde_json::from_str(&serialized).expect("round trip");
            assert_eq!(round_trip.version, "0.1.0");
        }
    }

    #[test]
    fn load_merges_builtin_actions_without_duplicates() {
        let merged = merge_builtin_actions(sample_config()).expect("merge failed");
        assert!(merged.categories.iter().any(|item| item.id == "sftp-sync"));
        assert!(merged.commands.iter().any(|item| item.id == "sftp-pro"));

        let merged_again = merge_builtin_actions(merged).expect("second merge failed");
        assert_eq!(
            merged_again
                .commands
                .iter()
                .filter(|item| item.id == "sftp-pro")
                .count(),
            1
        );
    }
}
