//! JSON persistence for WinFXStart configuration.
//!
//! - `load()`: reads `%APPDATA%\WinFXStart\config.json`; falls back to the
//!   bundled `config/default.json` (cwd then exe-dir) when the user file is
//!   absent.
//! - `save()`: writes the typed [`Config`] to `%APPDATA%\WinFXStart\config.json`,
//!   creating the directory if needed.  The `version` field is preserved as-is.
//!
//! Neither function panics.  Path-injectable variants (`load_from` / `save_to`)
//! are provided so unit tests can operate in a temp directory without touching
//! the real `%APPDATA%`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::storage::models::Config;

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
                write!(f, "no config found (user file and bundled default both missing)")
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

/// Returns `%APPDATA%\WinFXStart\config.json`, or `None` if `APPDATA` is unset.
pub fn user_config_path() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|appdata| PathBuf::from(appdata).join("WinFXStart").join("config.json"))
}

/// Returns the path to the bundled `config/default.json` using the same
/// candidates as the former `load_config()` in `app.rs`:
/// 1. Relative to the current working directory.
/// 2. Relative to the executable directory.
pub fn default_config_path() -> Option<PathBuf> {
    let relative = PathBuf::from("config/default.json");
    if relative.exists() {
        return Some(relative);
    }

    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("config/default.json")))
        .filter(|p| p.exists())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load configuration.
///
/// Priority:
/// 1. `%APPDATA%\WinFXStart\config.json` (user file).
/// 2. Bundled `config/default.json` (cwd or exe-dir).
///
/// Returns [`StorageError::NoConfigFound`] if neither is present.
pub fn load() -> Result<Config, StorageError> {
    // 1. Try the user file.
    if let Some(user_path) = user_config_path() {
        if user_path.exists() {
            log::info!("Loading config from user file: {}", user_path.display());
            return load_from(&user_path);
        }
    }

    // 2. Fall back to the bundled default.
    if let Some(default_path) = default_config_path() {
        log::info!(
            "User config absent — loading bundled default: {}",
            default_path.display()
        );
        return load_from(&default_path);
    }

    log::warn!("No config file found (user file and bundled default both missing)");
    Err(StorageError::NoConfigFound)
}

/// Load configuration from an explicit path.
///
/// Used by unit tests to avoid touching `%APPDATA%`.
pub fn load_from(path: &Path) -> Result<Config, StorageError> {
    let text = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&text)?;
    Ok(config)
}

/// Save configuration to `%APPDATA%\WinFXStart\config.json`.
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
            version: "0.1.0".to_string(),
            default_settings: Settings {
                show_categories: true,
                icon_size: 80,
                theme: "light".to_string(),
                launch_at_startup: true,
                show_descriptions: true,
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
                },
                Command {
                    id: "cmd".to_string(),
                    name: "Invite de commandes".to_string(),
                    command: "cmd.exe /c".to_string(),
                    category: "system".to_string(),
                    icon: "💻".to_string(),
                    is_favorite: true,
                    shortcut: None,
                },
            ],
        }
    }

    #[test]
    fn round_trip_lossless_with_version_preserved() {
        let original = sample_config();
        let temp_dir = std::env::temp_dir().join(format!(
            "winfxstart_test_{}",
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
            "winfxstart_shortcut_test_{}",
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
        let nonexistent = std::env::temp_dir().join("winfxstart_absent_99999999/config.json");
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
            "winfxstart_malformed_{}",
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
            "winfxstart_mkdirs_{}",
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
    fn user_config_path_contains_winfxstart() {
        // Only verify shape when APPDATA is set; skip if absent.
        if let Some(path) = user_config_path() {
            let display = path.display().to_string();
            assert!(
                display.contains("WinFXStart"),
                "user config path must contain WinFXStart, got: {display}"
            );
            assert!(
                display.ends_with("config.json"),
                "user config path must end with config.json, got: {display}"
            );
        }
    }
}
