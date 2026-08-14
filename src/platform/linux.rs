//! Linux platform implementation — XDG Base Directory path resolution plus
//! [`StartupProvider`].
//!
//! Implements XDG Base Directory path resolution (`config_path`,
//! `data_dir`, `state_log_path`) and, since Part 3 Phase 1 of the multi-OS
//! transformation plan, [`crate::platform::StartupProvider`] via
//! [`LinuxStartupProvider`] — a thin forward to
//! `crate::linux::autostart`'s XDG `.desktop`-file logic, mirroring how
//! [`crate::platform::windows::RegistryStartupProvider`] forwards to
//! `crate::windows::registry`.
//!
//! The public functions (`config_path`, `data_dir`, `state_log_path`) read
//! real process environment variables via `std::env::var`. Their logic is
//! factored into `*_with_env` variants that take an injectable
//! `name -> Option<String>` lookup closure, so the XDG fallback rules can be
//! unit-tested deterministically without mutating real (process-global,
//! test-order-dependent) environment variables.

use std::path::PathBuf;

use crate::platform::{StartupError, StartupProvider};

const APP_DIR_NAME: &str = "devtoolbox";
const CONFIG_FILE_NAME: &str = "config.json";
const LOG_FILE_NAME: &str = "devtoolbox.log";
const MACHINE_COMMANDS_FILE_NAME: &str = "machine-commands.json";
const APPLICATION_USAGE_FILE_NAME: &str = "application-usage.json";
const MACHINE_ID_ENV_VAR: &str = "DEVTOOLBOX_MACHINE_ID";
const UNKNOWN_MACHINE_ID: &str = "unknown";

/// `$XDG_CONFIG_HOME/devtoolbox/config.json`, falling back to
/// `~/.config/devtoolbox/config.json` when `XDG_CONFIG_HOME` is unset or
/// empty.
pub fn config_path() -> PathBuf {
    config_path_with_env(&std_env_lookup)
}

/// `DEVTOOLBOX_MACHINE_ID` when set to a non-empty value, else the trimmed
/// contents of `/etc/hostname`, else the `"unknown"` sentinel. Never panics.
pub fn machine_id() -> String {
    machine_id_with_sources(&std_env_lookup, &std_hostname_file_reader)
}

/// `$XDG_STATE_HOME/devtoolbox/machine-commands.json`, falling back to
/// `~/.local/state/devtoolbox/machine-commands.json`. Mirrors
/// [`state_log_path`]'s directory, deliberately not [`config_path`]'s.
pub fn machine_commands_path() -> PathBuf {
    machine_commands_path_with_env(&std_env_lookup)
}

pub fn application_usage_path() -> PathBuf {
    application_usage_path_with_env(&std_env_lookup)
}

/// Base directory for application data: `$XDG_DATA_HOME/devtoolbox`,
/// falling back to `~/.local/share/devtoolbox`.
pub fn data_dir() -> PathBuf {
    data_dir_with_env(&std_env_lookup)
}

/// `$XDG_STATE_HOME/devtoolbox/devtoolbox.log`, falling back to
/// `~/.local/state/devtoolbox/devtoolbox.log`. The `devtoolbox.log`
/// filename mirrors the convention used by
/// `platform::windows::state_log_path()`.
pub fn state_log_path() -> PathBuf {
    state_log_path_with_env(&std_env_lookup)
}

// ---------------------------------------------------------------------------
// Env-injectable core
// ---------------------------------------------------------------------------

/// `name -> Option<value>` environment lookup, injectable for testing.
type EnvLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

fn std_env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn home_dir(env: &EnvLookup) -> PathBuf {
    // No sensible fallback exists if HOME is unset; `/` keeps path-joining
    // well-defined rather than panicking. HOME is always set in practice on
    // Linux user sessions.
    env("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn config_path_with_env(env: &EnvLookup) -> PathBuf {
    xdg_base_dir(env, "XDG_CONFIG_HOME", ".config")
        .join(APP_DIR_NAME)
        .join(CONFIG_FILE_NAME)
}

fn data_dir_with_env(env: &EnvLookup) -> PathBuf {
    xdg_base_dir(env, "XDG_DATA_HOME", ".local/share").join(APP_DIR_NAME)
}

fn state_log_path_with_env(env: &EnvLookup) -> PathBuf {
    xdg_base_dir(env, "XDG_STATE_HOME", ".local/state")
        .join(APP_DIR_NAME)
        .join(LOG_FILE_NAME)
}

fn machine_commands_path_with_env(env: &EnvLookup) -> PathBuf {
    xdg_base_dir(env, "XDG_STATE_HOME", ".local/state")
        .join(APP_DIR_NAME)
        .join(MACHINE_COMMANDS_FILE_NAME)
}

fn application_usage_path_with_env(env: &EnvLookup) -> PathBuf {
    xdg_base_dir(env, "XDG_STATE_HOME", ".local/state")
        .join(APP_DIR_NAME)
        .join(APPLICATION_USAGE_FILE_NAME)
}

/// `/etc/hostname` reader, injectable for testing. `EnvLookup`'s
/// `Fn(&str) -> Option<String>` shape models an env-var lookup, not a
/// filesystem read, so this is a second, dedicated injection point.
/// Returns `None` when the file cannot be read (e.g. absent on a minimal
/// container/distro) rather than propagating an I/O error — `machine_id`
/// degrades to the `"unknown"` sentinel in that case, never panics.
type HostnameFileReader<'a> = dyn Fn() -> Option<String> + 'a;

fn std_hostname_file_reader() -> Option<String> {
    std::fs::read_to_string("/etc/hostname").ok()
}

fn machine_id_with_sources(env: &EnvLookup, hostname_reader: &HostnameFileReader) -> String {
    if let Some(value) = env(MACHINE_ID_ENV_VAR) {
        if !value.is_empty() {
            return value;
        }
    }
    hostname_reader()
        .map(|contents| contents.trim().to_string())
        .filter(|trimmed| !trimmed.is_empty())
        .unwrap_or_else(|| UNKNOWN_MACHINE_ID.to_string())
}

/// Resolve an XDG base directory: `$<var>` when set to a non-empty value,
/// else `$HOME/<fallback_relative>` (per the XDG Base Directory spec, an
/// unset or empty variable means "use the default").
fn xdg_base_dir(env: &EnvLookup, var: &str, fallback_relative: &str) -> PathBuf {
    match env(var) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home_dir(env).join(fallback_relative),
    }
}

// ---------------------------------------------------------------------------
// StartupProvider
// ---------------------------------------------------------------------------

/// [`StartupProvider`] backed by the XDG autostart `.desktop`-file logic in
/// `crate::linux::autostart`. Adds no logic of its own — every method is a
/// direct forward, matching the same "thin wrapper" shape as
/// [`crate::platform::windows::RegistryStartupProvider`].
pub struct LinuxStartupProvider;

impl StartupProvider for LinuxStartupProvider {
    fn register(&self) -> Result<(), StartupError> {
        crate::linux::autostart::register().map_err(|e| Box::new(e) as StartupError)
    }

    fn unregister(&self) -> Result<(), StartupError> {
        crate::linux::autostart::unregister().map_err(|e| Box::new(e) as StartupError)
    }

    fn is_registered(&self) -> bool {
        crate::linux::autostart::is_registered()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an env-lookup closure from a fixed set of pairs — variables not
    /// listed behave as unset (`None`), independent of the real process env.
    fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn config_path_uses_xdg_config_home_when_set() {
        let env = env_map(&[
            ("XDG_CONFIG_HOME", "/custom/config"),
            ("HOME", "/home/someone"),
        ]);
        assert_eq!(
            config_path_with_env(&env),
            PathBuf::from("/custom/config/devtoolbox/config.json")
        );
    }

    #[test]
    fn config_path_falls_back_to_home_config_when_xdg_unset() {
        let env = env_map(&[("HOME", "/home/someone")]);
        assert_eq!(
            config_path_with_env(&env),
            PathBuf::from("/home/someone/.config/devtoolbox/config.json")
        );
    }

    #[test]
    fn config_path_falls_back_when_xdg_config_home_is_empty_string() {
        let env = env_map(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/someone")]);
        assert_eq!(
            config_path_with_env(&env),
            PathBuf::from("/home/someone/.config/devtoolbox/config.json"),
            "an empty XDG_CONFIG_HOME must be treated as unset per the XDG spec"
        );
    }

    #[test]
    fn data_dir_uses_xdg_data_home_when_set() {
        let env = env_map(&[("XDG_DATA_HOME", "/custom/data"), ("HOME", "/home/someone")]);
        assert_eq!(
            data_dir_with_env(&env),
            PathBuf::from("/custom/data/devtoolbox")
        );
    }

    #[test]
    fn data_dir_falls_back_to_home_local_share_when_xdg_unset() {
        let env = env_map(&[("HOME", "/home/someone")]);
        assert_eq!(
            data_dir_with_env(&env),
            PathBuf::from("/home/someone/.local/share/devtoolbox")
        );
    }

    #[test]
    fn data_dir_falls_back_when_xdg_data_home_is_empty_string() {
        let env = env_map(&[("XDG_DATA_HOME", ""), ("HOME", "/home/someone")]);
        assert_eq!(
            data_dir_with_env(&env),
            PathBuf::from("/home/someone/.local/share/devtoolbox"),
            "an empty XDG_DATA_HOME must be treated as unset per the XDG spec"
        );
    }

    #[test]
    fn state_log_path_uses_xdg_state_home_when_set() {
        let env = env_map(&[
            ("XDG_STATE_HOME", "/custom/state"),
            ("HOME", "/home/someone"),
        ]);
        assert_eq!(
            state_log_path_with_env(&env),
            PathBuf::from("/custom/state/devtoolbox/devtoolbox.log")
        );
    }

    #[test]
    fn state_log_path_falls_back_to_home_local_state_when_xdg_unset() {
        let env = env_map(&[("HOME", "/home/someone")]);
        assert_eq!(
            state_log_path_with_env(&env),
            PathBuf::from("/home/someone/.local/state/devtoolbox/devtoolbox.log")
        );
    }

    #[test]
    fn state_log_path_falls_back_when_xdg_state_home_is_empty_string() {
        let env = env_map(&[("XDG_STATE_HOME", ""), ("HOME", "/home/someone")]);
        assert_eq!(
            state_log_path_with_env(&env),
            PathBuf::from("/home/someone/.local/state/devtoolbox/devtoolbox.log"),
            "an empty XDG_STATE_HOME must be treated as unset per the XDG spec"
        );
    }

    #[test]
    fn machine_commands_path_uses_xdg_state_home_when_set() {
        let env = env_map(&[
            ("XDG_STATE_HOME", "/custom/state"),
            ("HOME", "/home/someone"),
        ]);
        assert_eq!(
            machine_commands_path_with_env(&env),
            PathBuf::from("/custom/state/devtoolbox/machine-commands.json")
        );
    }

    #[test]
    fn machine_commands_path_falls_back_to_home_local_state_when_xdg_unset() {
        let env = env_map(&[("HOME", "/home/someone")]);
        assert_eq!(
            machine_commands_path_with_env(&env),
            PathBuf::from("/home/someone/.local/state/devtoolbox/machine-commands.json")
        );
    }

    #[test]
    fn application_usage_path_is_machine_local_xdg_state() {
        let env = env_map(&[
            ("XDG_STATE_HOME", "/custom/state"),
            ("HOME", "/home/someone"),
        ]);
        assert_eq!(
            application_usage_path_with_env(&env),
            PathBuf::from("/custom/state/devtoolbox/application-usage.json")
        );
    }

    #[test]
    fn machine_id_returns_env_override_when_set() {
        let env = env_map(&[("DEVTOOLBOX_MACHINE_ID", "test-machine")]);
        let hostname_reader = || -> Option<String> {
            panic!("hostname file must not be read when the env override is set")
        };
        assert_eq!(
            machine_id_with_sources(&env, &hostname_reader),
            "test-machine"
        );
    }

    #[test]
    fn machine_id_falls_back_to_trimmed_hostname_when_env_unset() {
        let env = env_map(&[]);
        let hostname_reader = || Some("my-hostname\n".to_string());
        assert_eq!(
            machine_id_with_sources(&env, &hostname_reader),
            "my-hostname",
            "trailing newline from /etc/hostname must be trimmed"
        );
    }

    #[test]
    fn machine_id_falls_back_to_trimmed_hostname_when_env_is_empty_string() {
        let env = env_map(&[("DEVTOOLBOX_MACHINE_ID", "")]);
        let hostname_reader = || Some("my-hostname\n".to_string());
        assert_eq!(
            machine_id_with_sources(&env, &hostname_reader),
            "my-hostname",
            "an empty DEVTOOLBOX_MACHINE_ID must be treated as unset"
        );
    }

    #[test]
    fn machine_id_falls_back_to_unknown_when_hostname_file_unavailable() {
        let env = env_map(&[]);
        let hostname_reader = || None;
        assert_eq!(
            machine_id_with_sources(&env, &hostname_reader),
            "unknown",
            "must degrade to the unknown sentinel, never panic, when /etc/hostname is unreadable"
        );
    }

    #[test]
    fn machine_id_falls_back_to_unknown_when_hostname_file_is_blank() {
        let env = env_map(&[]);
        let hostname_reader = || Some("   \n".to_string());
        assert_eq!(
            machine_id_with_sources(&env, &hostname_reader),
            "unknown",
            "a blank /etc/hostname must degrade to the unknown sentinel"
        );
    }

    /// Exercise the real `std::env`-backed public functions at least once so
    /// the wiring from `std_env_lookup` through to each `*_with_env`
    /// function actually compiles and runs end-to-end. Values are not
    /// asserted since the real environment is outside test control.
    #[test]
    fn public_functions_do_not_panic() {
        let _ = config_path();
        let _ = data_dir();
        let _ = state_log_path();
        let _ = machine_id();
        let _ = machine_commands_path();
        let _ = application_usage_path();
    }
}
