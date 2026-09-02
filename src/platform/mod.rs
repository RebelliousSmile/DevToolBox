//! Platform abstraction layer.
//!
//! Exposes OS-neutral config/data/state path resolution and a
//! `StartupProvider` trait for "launch at OS startup/login" registration.
//! Dispatches to [`windows`] (Win32 `%APPDATA%`/`%LOCALAPPDATA%` plus the
//! HKCU Run-key registry logic) or [`linux`] (XDG Base Directory spec) at
//! compile time via `#[cfg(windows)]` / `#[cfg(target_os = "linux")]`.
//!
//! `platform::linux` implements [`StartupProvider`] via
//! [`linux::LinuxStartupProvider`], wrapping the XDG `.desktop` autostart
//! logic in `crate::linux::autostart` (Part 3, Phase 1 of the multi-OS
//! transformation plan).
//!
//! # Wiring status (Part 1, Phase 2)
//! This module is not yet called by `main.rs`, `storage::json`, or
//! `icons::resolve` — those callers still hold their own copies of the path
//! logic this module relocates/wraps. Rewiring them is Phase 3's job.

// Nothing in this crate calls into `platform::` yet (Phase 3 wires the
// callers). Keep the dead-code lint quiet until then, mirroring the same
// pattern already used in `storage::json` for its not-yet-wired write side.
#![allow(dead_code)]

use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(any(target_os = "macos", test))]
pub mod macos;
#[cfg(windows)]
pub mod windows;

/// Error type returned by [`StartupProvider`] operations.
pub type StartupError = Box<dyn std::error::Error>;

/// OS-specific "launch at startup/login" registration.
///
/// Implemented by [`windows::RegistryStartupProvider`] (wraps the existing
/// `crate::windows::registry` Run-key logic) and by
/// [`linux::LinuxStartupProvider`] (wraps `crate::linux::autostart`'s XDG
/// `.desktop`-file logic).
pub trait StartupProvider {
    /// Register the application to launch at OS startup/login.
    fn register(&self) -> Result<(), StartupError>;
    /// Remove the application from OS startup/login.
    fn unregister(&self) -> Result<(), StartupError>;
    /// Return `true` iff the application is currently registered for startup.
    fn is_registered(&self) -> bool;
}

/// Path to the user configuration file (`config.json`).
///
/// - Windows: `%APPDATA%\DevToolBox\config.json` — relocated from
///   `storage::json::user_config_path()`, byte-identical when `APPDATA` is
///   set.
/// - Linux: `$XDG_CONFIG_HOME/devtoolbox/config.json`, falling back to
///   `~/.config/devtoolbox/config.json`.
#[cfg(windows)]
pub fn config_path() -> PathBuf {
    windows::config_path()
}

/// See [`config_path`] (Windows variant); this is the Linux variant.
#[cfg(target_os = "linux")]
pub fn config_path() -> PathBuf {
    linux::config_path()
}

#[cfg(target_os = "macos")]
pub fn config_path() -> PathBuf {
    macos::config_path()
}

/// Base directory for application data (icons, etc.).
///
/// - Windows: `%APPDATA%\DevToolBox` — relocated from the first candidate
///   built by `icons::resolve::icons_dirs()`.
/// - Linux: `$XDG_DATA_HOME/devtoolbox`, falling back to
///   `~/.local/share/devtoolbox`.
#[cfg(windows)]
pub fn data_dir() -> PathBuf {
    windows::data_dir()
}

/// See [`data_dir`] (Windows variant); this is the Linux variant.
#[cfg(target_os = "linux")]
pub fn data_dir() -> PathBuf {
    linux::data_dir()
}

#[cfg(target_os = "macos")]
pub fn data_dir() -> PathBuf {
    macos::data_dir()
}

/// Path to the application log file.
///
/// - Windows: `%LOCALAPPDATA%\DevToolBox\devtoolbox.log` — relocated from
///   `main::init_logging()`, byte-identical fallback-to-temp-dir behavior.
/// - Linux: `$XDG_STATE_HOME/devtoolbox/devtoolbox.log`, falling back to
///   `~/.local/state/devtoolbox/devtoolbox.log` (mirrors the
///   `devtoolbox.log` filename convention used on Windows).
#[cfg(windows)]
pub fn state_log_path() -> PathBuf {
    windows::state_log_path()
}

/// See [`state_log_path`] (Windows variant); this is the Linux variant.
#[cfg(target_os = "linux")]
pub fn state_log_path() -> PathBuf {
    linux::state_log_path()
}

#[cfg(target_os = "macos")]
pub fn state_log_path() -> PathBuf {
    macos::state_log_path()
}

/// Sync the OS "launch at startup/login" registration to match `enabled`.
///
/// Routes through the OS-specific [`StartupProvider`]
/// ([`windows::RegistryStartupProvider`] on Windows,
/// [`linux::LinuxStartupProvider`] on Linux) instead of callers reaching
/// into `crate::windows::registry` / `crate::linux::autostart` directly.
///
/// - Windows: registers/unregisters via the HKCU Run-key registry logic.
/// - Linux: registers/unregisters via the XDG autostart `.desktop`-file
///   logic in `crate::linux::autostart`.
#[cfg(windows)]
pub fn sync_startup(enabled: bool) -> Result<(), StartupError> {
    let provider = windows::RegistryStartupProvider;
    if enabled {
        provider.register()
    } else {
        provider.unregister()
    }
}

/// See [`sync_startup`] (Windows variant); this is the Linux variant.
#[cfg(target_os = "linux")]
pub fn sync_startup(enabled: bool) -> Result<(), StartupError> {
    let provider = linux::LinuxStartupProvider;
    if enabled {
        provider.register()
    } else {
        provider.unregister()
    }
}

#[cfg(target_os = "macos")]
pub fn sync_startup(enabled: bool) -> Result<(), StartupError> {
    let provider = macos::MacStartupProvider;
    if enabled {
        provider.register()
    } else {
        provider.unregister()
    }
}

/// Identifier for "which machine is this", used by the (future) per-machine
/// command mapping resolution layer.
///
/// Resolution order:
/// 1. The `DEVTOOLBOX_MACHINE_ID` environment variable, when set to a
///    non-empty value (lets a user pin an explicit id regardless of OS
///    hostname quirks).
/// 2. The OS-specific hostname source: `%COMPUTERNAME%` on Windows, the
///    trimmed contents of `/etc/hostname` on Linux.
/// 3. The `"unknown"` sentinel, when even the hostname source is
///    unavailable (e.g. `/etc/hostname` absent on a minimal container).
///
/// Never panics.
#[cfg(windows)]
pub fn machine_id() -> String {
    windows::machine_id()
}

/// See [`machine_id`] (Windows variant); this is the Linux variant.
#[cfg(target_os = "linux")]
pub fn machine_id() -> String {
    linux::machine_id()
}

#[cfg(target_os = "macos")]
pub fn machine_id() -> String {
    macos::machine_id()
}

/// Path to the per-machine command mapping file (`machine-commands.json`).
///
/// Deliberately mirrors [`state_log_path`]'s directory-resolution
/// convention, NOT [`config_path`]'s: the mapping is machine-local and must
/// stay outside the directory a config-sync tool would target.
///
/// - Windows: `%LOCALAPPDATA%\DevToolBox\machine-commands.json`.
/// - Linux: `$XDG_STATE_HOME/devtoolbox/machine-commands.json`, falling back
///   to `~/.local/state/devtoolbox/machine-commands.json`.
#[cfg(windows)]
pub fn machine_commands_path() -> PathBuf {
    windows::machine_commands_path()
}

/// See [`machine_commands_path`] (Windows variant); this is the Linux variant.
#[cfg(target_os = "linux")]
pub fn machine_commands_path() -> PathBuf {
    linux::machine_commands_path()
}

#[cfg(target_os = "macos")]
pub fn machine_commands_path() -> PathBuf {
    macos::machine_commands_path()
}

/// Machine-local, non-roaming prospective application usage history.
#[cfg(windows)]
pub fn application_usage_path() -> PathBuf {
    windows::application_usage_path()
}

/// See [`application_usage_path`] (Windows variant); this is the Linux variant.
#[cfg(target_os = "linux")]
pub fn application_usage_path() -> PathBuf {
    linux::application_usage_path()
}

#[cfg(target_os = "macos")]
pub fn application_usage_path() -> PathBuf {
    macos::application_usage_path()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub actions: bool,
    pub terminal: bool,
    pub docker_read: bool,
    pub automations: bool,
    pub recommendations: bool,
    pub cleanup: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformKind {
    Windows,
    Linux,
    Macos,
}

pub const fn capabilities_for(platform: PlatformKind) -> Capabilities {
    match platform {
        PlatformKind::Windows | PlatformKind::Linux => Capabilities {
            actions: true,
            terminal: true,
            docker_read: true,
            automations: true,
            recommendations: true,
            cleanup: true,
        },
        PlatformKind::Macos => Capabilities {
            actions: true,
            terminal: true,
            docker_read: true,
            automations: false,
            recommendations: false,
            cleanup: false,
        },
    }
}

pub const fn capabilities() -> Capabilities {
    #[cfg(windows)]
    {
        capabilities_for(PlatformKind::Windows)
    }
    #[cfg(target_os = "linux")]
    {
        capabilities_for(PlatformKind::Linux)
    }
    #[cfg(target_os = "macos")]
    {
        capabilities_for(PlatformKind::Macos)
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn macos_core_and_unavailable_capabilities_are_explicit() {
        let capabilities = capabilities_for(PlatformKind::Macos);
        assert!(capabilities.actions);
        assert!(capabilities.terminal);
        assert!(capabilities.docker_read);
        assert!(!capabilities.automations);
        assert!(!capabilities.recommendations);
        assert!(!capabilities.cleanup);
    }
}
