//! Per-machine command mapping storage.
//!
//! `MachineCommands` lives in a separate, non-synced on-disk file (see
//! `platform::machine_commands_path()`, deliberately outside the
//! `config.json` directory so a directory-level sync tool doesn't sweep it
//! up) mapping machine id -> command id -> override value. This module only
//! provides the model and load/save primitives, mirroring the existing
//! `storage::json` `config.json` pattern; a future resolution layer (Part 2)
//! reads from it.

// Not yet wired to any caller (Part 2 wires the resolution layer). Keep the
// dead-code lint quiet until then, mirroring the same pattern already used
// in `platform::` and `storage::categories` for their not-yet-wired APIs.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::models::Command;

/// Machine id -> command id -> override value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineCommands {
    #[serde(default)]
    pub machines: BTreeMap<String, BTreeMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Failure modes for [`load_machine_commands_from`].
///
/// A missing file is deliberately NOT represented here — it is not an error
/// (see that function's docs). This type only covers the cases where a file
/// exists but cannot be read or parsed, so a hand-edited JSON syntax error
/// is distinguishable from "no mapping configured yet".
#[derive(Debug)]
pub enum MachineCommandsError {
    /// An I/O error occurred while reading the file (other than "not found").
    Io(io::Error),
    /// JSON deserialization failed.
    Parse(serde_json::Error),
}

impl fmt::Display for MachineCommandsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MachineCommandsError::Io(e) => write!(f, "machine-commands I/O error: {e}"),
            MachineCommandsError::Parse(e) => write!(f, "machine-commands parse error: {e}"),
        }
    }
}

impl std::error::Error for MachineCommandsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MachineCommandsError::Io(e) => Some(e),
            MachineCommandsError::Parse(e) => Some(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load the per-machine command mapping from `path`.
///
/// A missing file is not an error: it returns `Ok(MachineCommands::default())`
/// (an empty map) since a machine that has never saved an override has no
/// file yet. Malformed JSON propagates a distinct
/// [`MachineCommandsError::Parse`] rather than being silently treated as
/// empty, so a hand-edited syntax error surfaces to the caller.
pub fn load_machine_commands_from(path: &Path) -> Result<MachineCommands, MachineCommandsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(MachineCommands::default()),
        Err(e) => return Err(MachineCommandsError::Io(e)),
    };
    let parsed: MachineCommands =
        serde_json::from_str(&text).map_err(MachineCommandsError::Parse)?;
    Ok(parsed)
}

/// Save `commands` to `path`, creating parent directories as needed.
pub fn save_machine_commands_to(path: &Path, commands: &MachineCommands) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(commands)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Outcome of resolving a [`Command`] against the per-machine mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResolution {
    /// The launch string to use — either `command.command` unchanged (for
    /// non-machine-specific commands) or the matching override.
    Resolved(String),
    /// `command.machine_specific` is `true` but no override is configured
    /// for this machine/command pair. Carries the ids so a caller (Part 3's
    /// UI) can render a helpful disabled state.
    Unconfigured {
        command_id: String,
        machine_id: String,
    },
}

/// Resolve `command`'s launch string for `machine_id` against `overrides`.
///
/// Non-machine-specific commands (`machine_specific == false`) always
/// resolve to `command.command` as-is, ignoring `overrides` entirely.
///
/// Machine-specific commands look up `overrides.machines[machine_id][command.id]`.
/// Both the stored machine-id keys and the lookup `machine_id` are lowercased
/// before comparing, so a mapping saved from a machine with an uppercase
/// hostname still matches a lookup using a lowercase one (see the plan's risk
/// register on machine id case-sensitivity). A miss on either the machine id
/// or the command id yields [`CommandResolution::Unconfigured`].
pub fn resolve_command(
    command: &Command,
    overrides: &MachineCommands,
    machine_id: &str,
) -> CommandResolution {
    if !command.machine_specific {
        return CommandResolution::Resolved(command.command.clone());
    }

    let lookup_id = machine_id.to_lowercase();
    let matching_machine = overrides
        .machines
        .iter()
        .find(|(stored_id, _)| stored_id.to_lowercase() == lookup_id)
        .map(|(_, per_command)| per_command);

    match matching_machine.and_then(|per_command| per_command.get(&command.id)) {
        Some(override_string) => CommandResolution::Resolved(override_string.clone()),
        None => CommandResolution::Unconfigured {
            command_id: command.id.clone(),
            machine_id: machine_id.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "devtoolbox_machine_commands_{label}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    fn sample() -> MachineCommands {
        let mut machines = BTreeMap::new();
        let mut overrides = BTreeMap::new();
        overrides.insert("notepad".to_string(), "notepad.exe -multiInst".to_string());
        overrides.insert("term".to_string(), "alacritty".to_string());
        machines.insert("desktop-1".to_string(), overrides);
        MachineCommands { machines }
    }

    #[test]
    fn missing_file_returns_empty_map_without_error() {
        let dir = scratch_path("missing");
        let path = dir.join("machine-commands.json");
        assert!(!path.exists(), "precondition: file must not exist");

        let loaded = load_machine_commands_from(&path).expect("missing file must not error");
        assert_eq!(
            loaded,
            MachineCommands::default(),
            "missing file must yield an empty map"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trip_save_then_load_returns_identical_content() {
        let dir = scratch_path("roundtrip");
        let path = dir.join("machine-commands.json");
        let original = sample();

        save_machine_commands_to(&path, &original).expect("save must succeed");
        let loaded = load_machine_commands_from(&path).expect("load must succeed");

        assert_eq!(loaded, original, "round-trip must be lossless");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_json_returns_distinct_parse_error_not_empty_map() {
        let dir = scratch_path("malformed");
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("machine-commands.json");
        std::fs::write(&path, b"{ not valid json !!!").expect("write malformed file");

        let result = load_machine_commands_from(&path);
        assert!(
            matches!(result, Err(MachineCommandsError::Parse(_))),
            "expected MachineCommandsError::Parse for malformed JSON, got: {:?}",
            result
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = scratch_path("mkdirs");
        let path = dir.join("nested").join("machine-commands.json");
        assert!(!path.exists(), "precondition: path must not exist");

        save_machine_commands_to(&path, &sample()).expect("save must create directories");
        assert!(path.exists(), "file must be created");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- resolve_command ------------------------------------------------

    fn make_command(id: &str, base_command: &str, machine_specific: bool) -> Command {
        Command {
            id: id.to_string(),
            command: base_command.to_string(),
            machine_specific,
            ..Default::default()
        }
    }

    #[test]
    fn non_machine_specific_always_resolves_to_base_command_regardless_of_overrides() {
        let command = make_command("notepad", "notepad.exe", false);
        let overrides = sample(); // has a "desktop-1" entry, but must be ignored

        let resolution = resolve_command(&command, &overrides, "desktop-1");

        assert_eq!(
            resolution,
            CommandResolution::Resolved("notepad.exe".to_string())
        );
    }

    #[test]
    fn machine_specific_with_matching_override_resolves_to_override_string() {
        let command = make_command("notepad", "notepad.exe", true);
        let overrides = sample(); // desktop-1 -> notepad -> "notepad.exe -multiInst"

        let resolution = resolve_command(&command, &overrides, "desktop-1");

        assert_eq!(
            resolution,
            CommandResolution::Resolved("notepad.exe -multiInst".to_string())
        );
    }

    #[test]
    fn machine_specific_with_no_matching_machine_returns_unconfigured() {
        let command = make_command("notepad", "notepad.exe", true);
        let overrides = sample(); // no "laptop-2" entry

        let resolution = resolve_command(&command, &overrides, "laptop-2");

        assert_eq!(
            resolution,
            CommandResolution::Unconfigured {
                command_id: "notepad".to_string(),
                machine_id: "laptop-2".to_string(),
            }
        );
    }

    #[test]
    fn machine_specific_with_no_matching_command_id_returns_unconfigured() {
        let command = make_command("does-not-exist", "whatever", true);
        let overrides = sample(); // desktop-1 exists, but has no "does-not-exist" entry

        let resolution = resolve_command(&command, &overrides, "desktop-1");

        assert_eq!(
            resolution,
            CommandResolution::Unconfigured {
                command_id: "does-not-exist".to_string(),
                machine_id: "desktop-1".to_string(),
            }
        );
    }

    #[test]
    fn machine_id_lookup_is_case_insensitive() {
        let command = make_command("notepad", "notepad.exe", true);
        let overrides = sample(); // stored key is lowercase "desktop-1"

        let resolution = resolve_command(&command, &overrides, "DESKTOP-1");

        assert_eq!(
            resolution,
            CommandResolution::Resolved("notepad.exe -multiInst".to_string())
        );
    }
}
