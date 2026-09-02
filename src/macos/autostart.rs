//! Per-user LaunchAgent registration for macOS.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL: &str = "com.rebellioussmile.devtoolbox";
const FILE_NAME: &str = "com.rebellioussmile.devtoolbox.plist";

pub fn register() -> io::Result<()> {
    let path = launch_agent_path();
    let executable = std::env::current_exe()?;
    register_at(&path, &executable, run_launchctl)
}

pub fn unregister() -> io::Result<()> {
    unregister_at(&launch_agent_path(), run_launchctl)
}

pub fn is_registered() -> bool {
    launch_agent_path().is_file()
}

fn launch_agent_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join("Library").join("LaunchAgents").join(FILE_NAME)
}

fn register_at<F>(path: &Path, executable: &Path, mut launchctl: F) -> io::Result<()>
where
    F: FnMut(&str, &Path) -> io::Result<()>,
{
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "LaunchAgent path has no parent",
        )
    })?;
    fs::create_dir_all(parent)?;
    atomic_write(path, plist_contents(executable).as_bytes())?;
    if let Err(error) = launchctl("bootstrap", path) {
        log::warn!("LaunchAgent written but launchctl bootstrap failed: {error}");
    }
    Ok(())
}

fn unregister_at<F>(path: &Path, mut launchctl: F) -> io::Result<()>
where
    F: FnMut(&str, &Path) -> io::Result<()>,
{
    if let Err(error) = launchctl("bootout", path) {
        log::warn!("launchctl bootout failed during unregister: {error}");
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension(format!("plist.tmp-{}", std::process::id()));
    fs::write(&temporary, contents)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn plist_contents(executable: &Path) -> String {
    let executable = escape_xml(&executable.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{executable}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn run_launchctl(action: &str, path: &Path) -> io::Result<()> {
    let uid = Command::new("/usr/bin/id").arg("-u").output()?;
    if !uid.status.success() {
        return Err(io::Error::other("id -u failed"));
    }
    let domain = format!("gui/{}", String::from_utf8_lossy(&uid.stdout).trim());
    let status = Command::new("/bin/launchctl")
        .arg(action)
        .arg(domain)
        .arg(path)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "launchctl {action} exited with {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_path(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "devtoolbox-launch-agent-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        root.join("Library").join("LaunchAgents").join(FILE_NAME)
    }

    #[test]
    fn register_and_unregister_are_idempotent_without_real_launchctl() {
        let path = isolated_path("roundtrip");
        let executable = Path::new("/Applications/Dev & Tools.app/Contents/MacOS/devtoolbox");
        let mut calls = Vec::new();
        register_at(&path, executable, |action, _| {
            calls.push(action.to_string());
            Err(io::Error::other("fixture refusal"))
        })
        .unwrap();
        register_at(&path, executable, |_, _| Ok(())).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains(LABEL));
        assert!(contents.contains("Dev &amp; Tools.app"));
        assert_eq!(calls, ["bootstrap"]);
        unregister_at(&path, |_, _| Err(io::Error::other("not loaded"))).unwrap();
        unregister_at(&path, |_, _| Ok(())).unwrap();
        assert!(!path.exists());
        let _ = fs::remove_dir_all(path.ancestors().nth(3).unwrap());
    }
}
