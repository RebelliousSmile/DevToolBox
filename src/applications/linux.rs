//! Best-effort Linux process observation through `/proc/<pid>/exe`.

use std::io;
use std::path::PathBuf;

const MAX_PROCESS_PATHS: usize = 32_768;

pub fn executable_paths() -> io::Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir("/proc")?;
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        if paths.len() >= MAX_PROCESS_PATHS {
            break;
        }
        let name = entry.file_name();
        if !name
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        if let Ok(path) = std::fs::read_link(entry.path().join("exe")) {
            paths.push(path);
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_proc_scan_is_bounded_and_contains_no_pid_data() {
        let paths = executable_paths().expect("/proc should be readable on Linux");
        assert!(paths.len() <= MAX_PROCESS_PATHS);
        assert!(paths.iter().all(|path| path.is_absolute()));
    }
}
