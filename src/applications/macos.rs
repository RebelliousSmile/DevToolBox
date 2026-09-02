//! Best-effort macOS process observation using the system ps command.

use std::io;
use std::path::PathBuf;
use std::process::Command;

const MAX_PROCESS_PATHS: usize = 32_768;

pub fn executable_paths() -> io::Result<Vec<PathBuf>> {
    let output = match Command::new("/bin/ps").args(["-axo", "comm="]).output() {
        Ok(output) if output.status.success() => output,
        Ok(_) | Err(_) => return Ok(Vec::new()),
    };
    Ok(parse_process_paths(&output.stdout))
}

fn parse_process_paths(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('/'))
        .take(MAX_PROCESS_PATHS)
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_keeps_absolute_paths_and_is_bounded() {
        let mut fixture = String::from("COMMAND\nrelative\n");
        for index in 0..(MAX_PROCESS_PATHS + 10) {
            fixture.push_str(&format!(
                "/Applications/App{index}.app/Contents/MacOS/App\n"
            ));
        }
        let paths = parse_process_paths(fixture.as_bytes());
        assert_eq!(paths.len(), MAX_PROCESS_PATHS);
        assert!(paths
            .iter()
            .all(|path| path.to_string_lossy().starts_with('/')));
    }
}
