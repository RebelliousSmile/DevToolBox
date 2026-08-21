//! Spawn a console child, capture its output, kill it if it overstays.
//!
//! Extracted from `docker::engine::run_command_capturing`, which was itself
//! written from the `cleanup::spawn::run_command` precedent. The host port
//! scan in [`crate::net`] is the second caller that needs the same three
//! guarantees, and copying the loop a third time is what the repo's DRY rule
//! forbids:
//!
//! - **No console flash on Windows** — every child here is a console-subsystem
//!   binary (`docker.exe`, `netstat.exe`, `tasklist.exe`), so they all need
//!   `CREATE_NO_WINDOW`.
//! - **No pipe deadlock** — stdout and stderr are drained on their own threads,
//!   so a child that fills a pipe buffer cannot block the wait loop that would
//!   otherwise be the thing to read it.
//! - **A hard deadline** — the caller runs on the UI thread, so a child that
//!   never exits has to be killed rather than waited on.
//!
//! What is deliberately *not* here: the meaning of a non-zero exit. Classifying
//! stderr is a per-CLI concern ("daemon unreachable" means nothing to
//! `netstat`), so this module reports the exit status and hands the text back
//! verbatim.

use std::io::{BufReader, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How often the wait loop re-checks a child that has not exited yet. Small
/// enough that a fast command is not measurably delayed by the poll, large
/// enough not to spin a core.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// What one invocation produced, exit status included.
///
/// A non-zero exit is **not** an error here: `docker inspect` exits non-zero
/// as soon as one id is unknown while still printing every id it resolved, and
/// dropping that stdout would lose the resolved ones.
pub struct Capture {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// The only two ways an invocation fails to produce a [`Capture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The program could not be spawned at all — not on `PATH`, or not
    /// executable. Callers usually translate this into "this feature is
    /// unavailable on this machine" rather than into an error.
    SpawnFailed,
    /// The deadline elapsed and the child was killed. Carries the budget that
    /// was exceeded so the message can name it.
    TimedOut(Duration),
}

/// Run `program args…`, returning its output or failing within `timeout`.
///
/// Output is decoded with `from_utf8_lossy`: a Windows console tool can emit
/// bytes that are not valid UTF-8 (an OEM code page), and losing an accent is
/// a far better outcome than losing the whole listing.
pub fn run_capturing(program: &str, args: &[&str], timeout: Duration) -> Result<Capture, RunError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process_flags::hide_console_window(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Err(RunError::SpawnFailed),
    };

    let stdout_handle = child.stdout.take().map(drain);
    let stderr_handle = child.stderr.take().map(drain);

    let expiry = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < expiry => std::thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(_) => break None,
        }
    };

    // Joined after the wait loop, never inside it: the threads end when the
    // pipes close, which only happens once the child is gone.
    let stdout_bytes = stdout_handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let stderr_bytes = stderr_handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    let Some(status) = status else {
        return Err(RunError::TimedOut(timeout));
    };
    Ok(Capture {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
    })
}

/// Read one pipe to exhaustion on its own thread.
fn drain<R: Read + Send + 'static>(pipe: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = BufReader::new(pipe).read_to_end(&mut bytes);
        bytes
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Picked over a real tool so the test stays honest on both OSes: the
    /// binary that does not exist is the one property every platform agrees on.
    #[test]
    fn an_unspawnable_program_reports_spawn_failed() {
        let result = run_capturing(
            "devtoolbox-no-such-binary-9f2c",
            &[],
            Duration::from_secs(1),
        );
        assert_eq!(result.err(), Some(RunError::SpawnFailed));
    }
}
