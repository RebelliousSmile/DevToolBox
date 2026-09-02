//! Safe preparation and optional user-data removal for platform uninstallers.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallInventory {
    pub integrations: Vec<PathBuf>,
    pub temporary: Vec<PathBuf>,
    pub user_data: Vec<PathBuf>,
}

pub fn inventory() -> UninstallInventory {
    let mut user_data = vec![
        crate::platform::data_dir(),
        crate::platform::config_path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        crate::platform::state_log_path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
    ];
    user_data.sort();
    user_data.dedup();
    UninstallInventory {
        integrations: vec![startup_integration_path()],
        temporary: vec![std::env::temp_dir().join("devtoolbox")],
        user_data,
    }
}

fn startup_integration_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    return crate::platform::data_dir()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("LaunchAgents/com.rebellioussmile.devtoolbox.plist");
    #[cfg(target_os = "linux")]
    return std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::platform::config_path()
                .parent()
                .unwrap()
                .to_path_buf()
        })
        .join("autostart/devtoolbox.desktop");
    #[cfg(windows)]
    return PathBuf::from("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\\DevToolBox");
}

pub fn prepare() -> Result<UninstallInventory, String> {
    crate::platform::sync_startup(false).map_err(|error| error.to_string())?;
    let inventory = inventory();
    for temporary in &inventory.temporary {
        safe_remove_tree(temporary, temporary)?;
    }
    Ok(inventory)
}

pub fn delete_user_data(confirmed: bool) -> Result<Vec<PathBuf>, String> {
    if !confirmed {
        return Err("confirmation explicite requise; les données sont conservées".to_string());
    }
    let inventory = inventory();
    let mut removed = Vec::new();
    for root in inventory.user_data {
        safe_remove_tree(&root, &root)?;
        removed.push(root);
    }
    Ok(removed)
}

fn safe_remove_tree(root: &Path, candidate: &Path) -> Result<(), String> {
    if !candidate.exists() {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(candidate).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "suppression refusée pour le lien {}",
            candidate.display()
        ));
    }
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if canonical_candidate != canonical_root && !canonical_candidate.starts_with(&canonical_root) {
        return Err(format!(
            "suppression hors racine refusée: {}",
            candidate.display()
        ));
    }
    std::fs::remove_dir_all(&canonical_candidate).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_removal_needs_explicit_confirmation() {
        assert!(delete_user_data(false)
            .unwrap_err()
            .contains("confirmation"));
    }

    #[test]
    fn a_symlink_is_never_followed_for_recursive_removal() {
        let base =
            std::env::temp_dir().join(format!("devtoolbox-uninstall-{}", std::process::id()));
        let outside = base.with_extension("outside");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let link = base.join("link");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, &link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(safe_remove_tree(&base, &link).is_err());
        assert!(outside.exists());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
