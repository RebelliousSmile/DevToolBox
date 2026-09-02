//! Windows platform implementation.
//!
//! Path resolution relocates the `%APPDATA%`/`%LOCALAPPDATA%` logic
//! currently duplicated in `storage::json::user_config_path()` (path
//! resolution for the config file), `icons::resolve::icons_dirs()` (the
//! `%APPDATA%\DevToolBox\icons` candidate), and `main::init_logging()` (the
//! log file location). Startup registration wraps the existing
//! `crate::windows::registry` Run-key logic — no reimplementation.
//!
//! # Verification note (Part 1, Phase 2)
//! This development environment is native Linux with no Windows toolchain,
//! so this module cannot be compiled or executed here. Its path-resolution
//! logic was verified by manual side-by-side comparison against the
//! originals it relocates (see the Phase 2 implementer report). The
//! `StartupProvider` impl only forwards to `crate::windows::registry`
//! functions and adds no new logic of its own.
//!
//! # Wiring status
//! `storage::json`, `icons::resolve`, and `main::init_logging` are not yet
//! rewired to call into this module (Phase 3). Their original logic still
//! lives in place, unchanged, alongside this relocated copy.

// Nothing calls into this module yet (Phase 3 wires the callers). Keep the
// dead-code lint quiet until then.
#![allow(dead_code)]

use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use crate::platform::{StartupError, StartupProvider};

const APP_DIR_NAME: &str = "DevToolBox";
const PUBLISHER_DIR_NAME: &str = "RebelliousSmile";
const CONFIG_FILE_NAME: &str = "config.json";
const LOG_FILE_NAME: &str = "devtoolbox.log";
const MACHINE_COMMANDS_FILE_NAME: &str = "machine-commands.json";
const APPLICATION_USAGE_FILE_NAME: &str = "application-usage.json";
const LEGACY_STATE_FILES: [&str; 4] = [
    LOG_FILE_NAME,
    "devtoolbox.log.old",
    MACHINE_COMMANDS_FILE_NAME,
    APPLICATION_USAGE_FILE_NAME,
];
const MACHINE_ID_ENV_VAR: &str = "DEVTOOLBOX_MACHINE_ID";
const UNKNOWN_MACHINE_ID: &str = "unknown";

/// `%APPDATA%\DevToolBox\config.json`.
///
/// Byte-identical to `storage::json::user_config_path()` when `APPDATA` is
/// set. That function returns `None` when `APPDATA` is unset; this one
/// falls back to `<temp_dir>\DevToolBox\config.json` instead, mirroring the
/// `LOCALAPPDATA`-unset fallback pattern already used by
/// `main::init_logging()`. The fallback exists only so this function can
/// satisfy the `platform::config_path() -> PathBuf` signature (non-`Option`)
/// — in practice `APPDATA` is always set on Windows, so it is unreachable
/// in normal operation.
pub fn config_path() -> PathBuf {
    appdata_dir().join(CONFIG_FILE_NAME)
}

/// Base directory for application data: `%APPDATA%\DevToolBox`.
///
/// This is the parent directory of the first candidate built by
/// `icons::resolve::icons_dirs()` (`%APPDATA%\DevToolBox\icons`).
pub fn data_dir() -> PathBuf {
    appdata_dir()
}

/// `%LOCALAPPDATA%\RebelliousSmile\DevToolBox\devtoolbox.log`.
///
/// Byte-identical to the path built by `main::init_logging()`, including
/// its fallback to `std::env::temp_dir()` when `LOCALAPPDATA` is unset.
pub fn state_log_path() -> PathBuf {
    local_state_dir().join(LOG_FILE_NAME)
}

/// `DEVTOOLBOX_MACHINE_ID` when set to a non-empty value, else
/// `%COMPUTERNAME%` when set to a non-empty value, else the `"unknown"`
/// sentinel. Never panics.
pub fn machine_id() -> String {
    std::env::var(MACHINE_ID_ENV_VAR)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("COMPUTERNAME")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| UNKNOWN_MACHINE_ID.to_string())
}

/// `%LOCALAPPDATA%\RebelliousSmile\DevToolBox\machine-commands.json`. Mirrors
/// [`state_log_path`]'s non-roaming directory, deliberately not
/// [`config_path`]'s roaming `%APPDATA%`.
pub fn machine_commands_path() -> PathBuf {
    local_state_dir().join(MACHINE_COMMANDS_FILE_NAME)
}

/// `%LOCALAPPDATA%\RebelliousSmile\DevToolBox\application-usage.json`.
pub fn application_usage_path() -> PathBuf {
    local_state_dir().join(APPLICATION_USAGE_FILE_NAME)
}

fn appdata_dir() -> PathBuf {
    // `var()` (not `var_os()`) on purpose: `storage::json::user_config_path()`
    // and `icons::resolve::icons_dirs()` both use `std::env::var("APPDATA")`,
    // which collapses "unset" and "not valid Unicode" to the same `Err` — and
    // so does this, for byte-identical parity with both originals.
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(APP_DIR_NAME)
}

fn localappdata_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn local_state_dir() -> PathBuf {
    localappdata_root()
        .join(PUBLISHER_DIR_NAME)
        .join(APP_DIR_NAME)
}

fn legacy_local_state_dir() -> PathBuf {
    localappdata_root().join(APP_DIR_NAME)
}

/// Moves only DevToolBox's known machine-local files out of the legacy
/// installation directory. The operation never traverses either root and
/// refuses links or conflicting destination content.
pub fn migrate_legacy_local_state() -> Result<Vec<PathBuf>, String> {
    migrate_local_state(&legacy_local_state_dir(), &local_state_dir())
}

fn migrate_local_state(legacy_root: &Path, state_root: &Path) -> Result<Vec<PathBuf>, String> {
    reject_root_link(legacy_root)?;
    reject_root_link(state_root)?;

    let mut migrated = Vec::new();
    for file_name in LEGACY_STATE_FILES {
        let source = legacy_root.join(file_name);
        let destination = state_root.join(file_name);
        if migrate_known_file(&source, &destination)? {
            migrated.push(destination);
        }
    }
    Ok(migrated)
}

fn reject_root_link(root: &Path) -> Result<(), String> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "migration refusée pour la racine liée {}",
            root.display()
        )),
        Ok(metadata) if !metadata.is_dir() => Err(format!(
            "la racine de migration n'est pas un dossier: {}",
            root.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "inspection de {} impossible: {error}",
            root.display()
        )),
    }
}

fn migrate_known_file(source: &Path, destination: &Path) -> Result<bool, String> {
    let source_metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "inspection de {} impossible: {error}",
                source.display()
            ))
        }
    };
    if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
        return Err(format!(
            "migration refusée pour le fichier non régulier {}",
            source.display()
        ));
    }

    if let Ok(destination_metadata) = fs::symlink_metadata(destination) {
        if destination_metadata.file_type().is_symlink() || !destination_metadata.is_file() {
            return Err(format!(
                "destination de migration non régulière: {}",
                destination.display()
            ));
        }
        if !files_equal(source, destination)? {
            return Err(format!(
                "conflit de migration, les deux copies sont conservées: {}",
                destination.display()
            ));
        }
        fs::remove_file(source)
            .map_err(|error| format!("suppression de {} impossible: {error}", source.display()))?;
        return Ok(true);
    }

    let parent = destination
        .parent()
        .ok_or_else(|| format!("destination sans parent: {}", destination.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("création de {} impossible: {error}", parent.display()))?;
    reject_root_link(parent)?;

    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("nom de destination invalide: {}", destination.display()))?;
    let temporary = parent.join(format!(".{file_name}.migration-{}.tmp", std::process::id()));
    let result = (|| {
        let mut input = File::open(source)
            .map_err(|error| format!("lecture de {} impossible: {error}", source.display()))?;
        let mut output = File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("création de {} impossible: {error}", temporary.display()))?;
        std::io::copy(&mut input, &mut output)
            .map_err(|error| format!("copie de {} impossible: {error}", source.display()))?;
        output.sync_all().map_err(|error| {
            format!(
                "synchronisation de {} impossible: {error}",
                temporary.display()
            )
        })?;
        drop(output);
        if !files_equal(source, &temporary)? {
            return Err(format!(
                "validation de la copie échouée pour {}",
                source.display()
            ));
        }
        fs::rename(&temporary, destination).map_err(|error| {
            format!(
                "validation atomique vers {} impossible: {error}",
                destination.display()
            )
        })?;
        fs::remove_file(source)
            .map_err(|error| format!("suppression de {} impossible: {error}", source.display()))?;
        Ok(true)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let left_len = fs::metadata(left).map_err(|error| error.to_string())?.len();
    let right_len = fs::metadata(right)
        .map_err(|error| error.to_string())?
        .len();
    if left_len != right_len {
        return Ok(false);
    }
    let mut left = BufReader::new(File::open(left).map_err(|error| error.to_string())?);
    let mut right = BufReader::new(File::open(right).map_err(|error| error.to_string())?);
    let mut left_buffer = [0_u8; 8192];
    let mut right_buffer = [0_u8; 8192];
    loop {
        let left_read = left
            .read(&mut left_buffer)
            .map_err(|error| error.to_string())?;
        let right_read = right
            .read(&mut right_buffer)
            .map_err(|error| error.to_string())?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

/// [`StartupProvider`] backed by the HKCU Run-key logic in
/// `crate::windows::registry`. Adds no logic of its own — every method is a
/// direct forward to the existing, already-tested registry functions.
pub struct RegistryStartupProvider;

impl StartupProvider for RegistryStartupProvider {
    fn register(&self) -> Result<(), StartupError> {
        crate::windows::registry::enable_startup().map_err(|e| Box::new(e) as StartupError)
    }

    fn unregister(&self) -> Result<(), StartupError> {
        crate::windows::registry::disable_startup().map_err(|e| Box::new(e) as StartupError)
    }

    fn is_registered(&self) -> bool {
        crate::windows::registry::is_startup_enabled()
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    fn roots(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "devtoolbox-state-migration-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let legacy = base.join("legacy");
        let state = base.join("state");
        fs::create_dir_all(&legacy).unwrap();
        (base, legacy, state)
    }

    #[test]
    fn known_files_are_moved_and_validated() {
        let (base, legacy, state) = roots("success");
        for (index, file_name) in LEGACY_STATE_FILES.iter().enumerate() {
            fs::write(legacy.join(file_name), format!("payload-{index}")).unwrap();
        }

        let migrated = migrate_local_state(&legacy, &state).unwrap();

        assert_eq!(migrated.len(), LEGACY_STATE_FILES.len());
        for (index, file_name) in LEGACY_STATE_FILES.iter().enumerate() {
            assert!(!legacy.join(file_name).exists());
            assert_eq!(
                fs::read_to_string(state.join(file_name)).unwrap(),
                format!("payload-{index}")
            );
        }
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn an_absent_source_is_a_no_op() {
        let (base, legacy, state) = roots("absent");
        assert!(migrate_local_state(&legacy, &state).unwrap().is_empty());
        assert!(!state.exists());
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn different_existing_content_blocks_without_overwriting_either_copy() {
        let (base, legacy, state) = roots("conflict");
        fs::create_dir_all(&state).unwrap();
        fs::write(legacy.join(LOG_FILE_NAME), b"legacy").unwrap();
        fs::write(state.join(LOG_FILE_NAME), b"current").unwrap();

        let error = migrate_local_state(&legacy, &state).unwrap_err();

        assert!(error.contains("conflit"));
        assert_eq!(fs::read(legacy.join(LOG_FILE_NAME)).unwrap(), b"legacy");
        assert_eq!(fs::read(state.join(LOG_FILE_NAME)).unwrap(), b"current");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn identical_existing_content_proves_preservation_and_removes_the_legacy_copy() {
        let (base, legacy, state) = roots("identical");
        fs::create_dir_all(&state).unwrap();
        fs::write(legacy.join(LOG_FILE_NAME), b"same").unwrap();
        fs::write(state.join(LOG_FILE_NAME), b"same").unwrap();

        migrate_local_state(&legacy, &state).unwrap();

        assert!(!legacy.join(LOG_FILE_NAME).exists());
        assert_eq!(fs::read(state.join(LOG_FILE_NAME)).unwrap(), b"same");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn an_unwritable_destination_shape_fails_without_touching_the_source() {
        let (base, legacy, state) = roots("write-error");
        fs::write(legacy.join(LOG_FILE_NAME), b"preserve me").unwrap();
        fs::write(&state, b"not a directory").unwrap();

        assert!(migrate_local_state(&legacy, &state).is_err());
        assert_eq!(
            fs::read(legacy.join(LOG_FILE_NAME)).unwrap(),
            b"preserve me"
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn a_source_symlink_is_refused_and_never_followed() {
        let (base, legacy, state) = roots("symlink");
        let outside = base.join("outside.log");
        fs::write(&outside, b"outside").unwrap();
        std::os::windows::fs::symlink_file(&outside, legacy.join(LOG_FILE_NAME)).unwrap();

        assert!(migrate_local_state(&legacy, &state).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
        assert!(!state.join(LOG_FILE_NAME).exists());
        let _ = fs::remove_dir_all(base);
    }
}
