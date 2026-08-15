//! Shared discovery and invocation rules for bundled Python modules.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn action_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(current) = std::env::current_dir() {
        candidates.push(current);
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    discover_action_root(std::env::var_os("DEVTOOLBOX_HOME"), candidates)
}

fn discover_action_root(configured: Option<OsString>, candidates: Vec<PathBuf>) -> PathBuf {
    if let Some(root) = configured {
        return PathBuf::from(root);
    }
    for candidate in &candidates {
        for ancestor in candidate.ancestors() {
            if ancestor.join("scripts").is_dir() {
                return ancestor.to_path_buf();
            }
        }
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn exists_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|directory| {
        directory.join(name).is_file() || directory.join(format!("{name}.exe")).is_file()
    })
}

pub fn python_for_script(script: &Path) -> String {
    let directory = script.parent().unwrap_or_else(|| Path::new("."));
    #[cfg(windows)]
    let local = directory.join(".venv").join("Scripts").join("python.exe");
    #[cfg(not(windows))]
    let local = directory.join(".venv").join("bin").join("python");
    select_python(
        local,
        std::env::var_os("DEVTOOLBOX_PYTHON"),
        exists_on_path("python3"),
    )
}

fn select_python(local: PathBuf, configured: Option<OsString>, python3_on_path: bool) -> String {
    if local.is_file() {
        return local.display().to_string();
    }
    if let Some(configured) = configured {
        if !configured.is_empty() {
            return configured.to_string_lossy().into_owned();
        }
    }
    if python3_on_path {
        "python3".to_string()
    } else {
        "python".to_string()
    }
}

pub fn recommendation_command(history_path: &Path) -> Result<Command, String> {
    let root = action_root();
    recommendation_command_from_root(root, history_path)
}

fn recommendation_command_from_root(root: PathBuf, history_path: &Path) -> Result<Command, String> {
    let module_entry = root
        .join("scripts")
        .join("app_recommendations")
        .join("__main__.py");
    if !module_entry.is_file() {
        return Err(format!(
            "module Python app_recommendations introuvable sous {}",
            root.display()
        ));
    }
    let inventory_entry = root
        .join("scripts")
        .join("system_inventory")
        .join("__init__.py");
    if !inventory_entry.is_file() {
        return Err(format!(
            "dépendance Python system_inventory introuvable sous {}",
            root.display()
        ));
    }
    let interpreter = python_for_script(&module_entry);
    let mut command = Command::new(&interpreter);
    command
        .args(["-m", "scripts.app_recommendations", "--json", "--history"])
        .arg(history_path)
        .current_dir(root)
        // Without this, Python picks the console's codepage (e.g. cp1252
        // on a French Windows install) for stdout when it isn't a real
        // console — a piped `Stdio` here always qualifies. Any accented
        // character (app names, size labels, …) then comes out as raw
        // Latin-1 bytes instead of UTF-8, which `serde_json::from_slice`
        // rejects outright since Rust strings must be valid UTF-8.
        .env("PYTHONIOENCODING", "utf-8")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn explicit_missing_root_yields_clear_module_error() {
        let missing =
            std::env::temp_dir().join(format!("devtoolbox-missing-{}", std::process::id()));
        let result = recommendation_command_from_root(missing, Path::new("history.json"));
        assert!(result.unwrap_err().contains("introuvable"));
    }

    #[test]
    fn repository_module_resolves_with_root_as_working_directory() {
        let command = recommendation_command(Path::new("history.json")).unwrap();
        assert!(command
            .get_args()
            .any(|argument| argument == "scripts.app_recommendations"));
        assert!(command
            .get_current_dir()
            .unwrap()
            .join("scripts/app_recommendations/__main__.py")
            .is_file());
        assert!(command
            .get_current_dir()
            .unwrap()
            .join("scripts/system_inventory/__init__.py")
            .is_file());
    }

    #[test]
    fn configured_distribution_root_wins_outside_repository() {
        let root = PathBuf::from("/opt/devtoolbox-package");
        let resolved = discover_action_root(
            Some(root.clone().into_os_string()),
            vec![PathBuf::from("/tmp/unrelated-launch-directory")],
        );
        assert_eq!(resolved, root);
    }

    #[test]
    fn interpreter_priority_is_local_then_configured_then_path() {
        let root = std::env::temp_dir().join(format!(
            "devtoolbox-python-selection-{}",
            std::process::id()
        ));
        let linux_local = root.join(".venv/bin/python");
        let windows_local = root.join(".venv/Scripts/python.exe");
        std::fs::create_dir_all(linux_local.parent().unwrap()).unwrap();
        std::fs::create_dir_all(windows_local.parent().unwrap()).unwrap();
        File::create(&linux_local).unwrap();
        File::create(&windows_local).unwrap();

        for local in [&linux_local, &windows_local] {
            assert_eq!(
                select_python(
                    local.to_path_buf(),
                    Some(OsString::from("configured-python")),
                    true,
                ),
                local.display().to_string()
            );
        }
        assert_eq!(
            select_python(
                root.join("missing"),
                Some(OsString::from("configured-python")),
                true,
            ),
            "configured-python"
        );
        assert_eq!(select_python(root.join("missing"), None, true), "python3");
        assert_eq!(select_python(root.join("missing"), None, false), "python");
        std::fs::remove_dir_all(root).unwrap();
    }
}
