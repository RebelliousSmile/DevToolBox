//! macOS paths and startup integration.

use std::path::PathBuf;
use std::process::Command;

use crate::platform::{StartupError, StartupProvider};

const APP_DIR_NAME: &str = "DevToolBox";
const MACHINE_ID_ENV_VAR: &str = "DEVTOOLBOX_MACHINE_ID";

type EnvLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;
type ComputerNameLookup<'a> = dyn Fn() -> Option<String> + 'a;

fn std_env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn home_dir(env: &EnvLookup) -> PathBuf {
    env("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn config_path() -> PathBuf {
    config_path_with_env(&std_env_lookup)
}

pub fn data_dir() -> PathBuf {
    data_dir_with_env(&std_env_lookup)
}

pub fn state_log_path() -> PathBuf {
    state_log_path_with_env(&std_env_lookup)
}

pub fn machine_commands_path() -> PathBuf {
    data_dir().join("machine-commands.json")
}

pub fn application_usage_path() -> PathBuf {
    data_dir().join("application-usage.json")
}

pub fn machine_id() -> String {
    machine_id_with_sources(&std_env_lookup, &computer_name)
}

fn application_support(env: &EnvLookup) -> PathBuf {
    home_dir(env)
        .join("Library")
        .join("Application Support")
        .join(APP_DIR_NAME)
}

fn config_path_with_env(env: &EnvLookup) -> PathBuf {
    application_support(env).join("config.json")
}

fn data_dir_with_env(env: &EnvLookup) -> PathBuf {
    application_support(env)
}

fn state_log_path_with_env(env: &EnvLookup) -> PathBuf {
    home_dir(env)
        .join("Library")
        .join("Logs")
        .join(APP_DIR_NAME)
        .join("devtoolbox.log")
}

fn machine_id_with_sources(env: &EnvLookup, name: &ComputerNameLookup) -> String {
    env(MACHINE_ID_ENV_VAR)
        .filter(|value| !value.trim().is_empty())
        .or_else(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn computer_name() -> Option<String> {
    let output = Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub struct MacStartupProvider;

impl StartupProvider for MacStartupProvider {
    fn register(&self) -> Result<(), StartupError> {
        crate::macos::autostart::register().map_err(|error| Box::new(error) as StartupError)
    }

    fn unregister(&self) -> Result<(), StartupError> {
        crate::macos::autostart::unregister().map_err(|error| Box::new(error) as StartupError)
    }

    fn is_registered(&self) -> bool {
        crate::macos::autostart::is_registered()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let values: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |name| values.get(name).cloned()
    }

    #[test]
    fn paths_follow_macos_library_conventions() {
        let env = env_map(&[("HOME", "/Users/alice")]);
        assert_eq!(
            config_path_with_env(&env),
            PathBuf::from("/Users/alice/Library/Application Support/DevToolBox/config.json")
        );
        assert_eq!(
            data_dir_with_env(&env),
            PathBuf::from("/Users/alice/Library/Application Support/DevToolBox")
        );
        assert_eq!(
            state_log_path_with_env(&env),
            PathBuf::from("/Users/alice/Library/Logs/DevToolBox/devtoolbox.log")
        );
    }

    #[test]
    fn machine_id_prefers_override_then_computer_name_then_unknown() {
        let env = env_map(&[("DEVTOOLBOX_MACHINE_ID", "pinned")]);
        assert_eq!(
            machine_id_with_sources(&env, &|| Some("Mac".to_string())),
            "pinned"
        );
        let empty = env_map(&[]);
        assert_eq!(
            machine_id_with_sources(&empty, &|| Some(" Studio \n".to_string())),
            "Studio"
        );
        assert_eq!(machine_id_with_sources(&empty, &|| None), "unknown");
    }
}
