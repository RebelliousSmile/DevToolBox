//! Cross-platform helper for the one Win32 process flag this app cares about.
//!
//! Every place that spawns a console-subsystem child (`docker`, `python`,
//! `cmd.exe`, arbitrary user commands from the terminal view) needs
//! `CREATE_NO_WINDOW`, or Windows opens a console window for it — a visible
//! flash for short-lived commands, and for long-lived ones a window whose
//! closing kills the child. The flag has no Unix analogue, so the helper is a
//! no-op there and callers stay free of `#[cfg]`.
//!
//! This module exists because the constant had been copy-pasted into four
//! modules (`windows::process`, `python_runtime`, `cleanup::spawn`,
//! `ui::terminal_view`) before the Docker backend became the fifth caller.

use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// `CREATE_NO_WINDOW` — documented in the Win32 process creation flags.
///
/// Public so the `windows::process` unit test that pins the value keeps
/// covering it after the extraction.
#[cfg_attr(not(windows), allow(dead_code))]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Keep a spawned console child from opening its own console window.
///
/// No-op off Windows. Returns the same `&mut Command` so it can sit in a
/// builder chain.
pub fn hide_console_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_matches_the_win32_value() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }

    /// The helper must stay usable in a chain on every OS — off Windows it
    /// does nothing, but it still has to hand the command back.
    #[test]
    fn hiding_the_console_returns_the_same_command() {
        let mut command = Command::new("does-not-need-to-exist");
        hide_console_window(&mut command).arg("--version");
        let rendered = format!("{command:?}");
        assert!(rendered.contains("--version"), "got: {rendered}");
    }
}
