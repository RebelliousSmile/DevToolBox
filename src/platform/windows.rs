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

use std::path::PathBuf;

use crate::platform::{StartupError, StartupProvider};

const APP_DIR_NAME: &str = "DevToolBox";
const CONFIG_FILE_NAME: &str = "config.json";
const LOG_FILE_NAME: &str = "devtoolbox.log";
const MACHINE_COMMANDS_FILE_NAME: &str = "machine-commands.json";
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

/// `%LOCALAPPDATA%\DevToolBox\devtoolbox.log`.
///
/// Byte-identical to the path built by `main::init_logging()`, including
/// its fallback to `std::env::temp_dir()` when `LOCALAPPDATA` is unset.
pub fn state_log_path() -> PathBuf {
    localappdata_dir().join(LOG_FILE_NAME)
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

/// `%LOCALAPPDATA%\DevToolBox\machine-commands.json`. Mirrors
/// [`state_log_path`]'s non-roaming directory, deliberately not
/// [`config_path`]'s roaming `%APPDATA%`.
pub fn machine_commands_path() -> PathBuf {
    localappdata_dir().join(MACHINE_COMMANDS_FILE_NAME)
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

fn localappdata_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR_NAME)
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
