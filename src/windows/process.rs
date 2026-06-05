//! Process launching via `std::process::Command` with the Win32
//! `CREATE_NO_WINDOW` creation flag.
//!
//! # Design
//! `launch(command)` accepts a raw command string (e.g. `"notepad.exe"`,
//! `"cmd.exe /c"`, `"ipconfig /all"`, `"\"C:\\Program Files\\App\\x.exe\" /flag"`),
//! tokenizes it in a quote/space-aware manner, and spawns the resulting
//! program + arguments with no stray console window.
//!
//! On Windows, `std::process::Command` spawns through `CreateProcessW`
//! internally, so this uses the Win32 process API via a safe, standard wrapper.
//! The `Win32_System_Threading` feature in `Cargo.toml` keeps the explicit
//! Win32 surface available for future issues (e.g. process tracking).

use std::fmt;
use std::io;
use std::process::Child;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// `CREATE_NO_WINDOW` — suppresses a stray console window when spawning
/// console sub-processes (cmd.exe, ipconfig, …).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur when launching a command.
#[derive(Debug)]
pub enum LaunchError {
    /// The command string was empty or contained only whitespace.
    Empty,
    /// The executable was not found (maps from `io::ErrorKind::NotFound`).
    NotFound {
        /// The program name that was not found.
        program: String,
    },
    /// Any other I/O error during spawn.
    Spawn {
        /// Underlying I/O error.
        source: io::Error,
    },
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::Empty => write!(f, "command string is empty"),
            LaunchError::NotFound { program } => {
                write!(f, "executable not found: {}", program)
            }
            LaunchError::Spawn { source } => {
                write!(f, "failed to spawn process: {}", source)
            }
        }
    }
}

impl std::error::Error for LaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LaunchError::Spawn { source } => Some(source),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// Parse a raw command string into `(program, args)`.
///
/// Rules:
/// - Leading/trailing whitespace is ignored.
/// - A double-quoted segment (`"…"`) is treated as a single token; surrounding
///   quotes are stripped from the program token.
/// - Outside quotes, tokens are delimited by ASCII whitespace.
/// - An empty or whitespace-only string returns `Err(LaunchError::Empty)`.
pub fn tokenize(command: &str) -> Result<(String, Vec<String>), LaunchError> {
    let command = command.trim();
    if command.is_empty() {
        return Err(LaunchError::Empty);
    }

    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in command.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                // Quotes are stripped — do not push the char.
            }
            ' ' | '\t' if !in_quotes => {
                // Whitespace outside quotes: flush current token if non-empty.
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    // Flush the last token.
    if !current.is_empty() {
        tokens.push(current);
    }

    // After parsing, tokens must be non-empty (we already checked for empty input).
    debug_assert!(!tokens.is_empty());

    let mut iter = tokens.into_iter();
    let program = iter.next().unwrap();
    let args: Vec<String> = iter.collect();

    Ok((program, args))
}

// ---------------------------------------------------------------------------
// Launcher
// ---------------------------------------------------------------------------

/// Outcome of a successful launch: a handle to the child process.
pub type LaunchOutcome = Child;

/// Launch a command string as a new process.
///
/// The command is parsed via [`tokenize`] then spawned with the Win32
/// `CREATE_NO_WINDOW` flag so no stray console window appears.
///
/// # Errors
/// - [`LaunchError::Empty`] — empty or whitespace-only command string.
/// - [`LaunchError::NotFound`] — executable not found on `PATH` or at the
///   specified path.
/// - [`LaunchError::Spawn`] — any other I/O error during spawn.
pub fn launch(command: &str) -> Result<LaunchOutcome, LaunchError> {
    let (program, args) = tokenize(command)?;

    log::info!("Launching: program={:?} args={:?}", program, args);

    let result = build_command(&program, &args).spawn();

    match result {
        Ok(child) => {
            log::info!("Launched successfully (program={:?})", program);
            Ok(child)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            log::warn!("Executable not found: {:?}", program);
            Err(LaunchError::NotFound { program })
        }
        Err(e) => {
            log::error!("Spawn failed for {:?}: {}", program, e);
            Err(LaunchError::Spawn { source: e })
        }
    }
}

/// Build the `std::process::Command` with `CREATE_NO_WINDOW` on Windows.
///
/// Extracted for testability — the flag constant can be verified without
/// actually spawning a real process.
fn build_command(program: &str, args: &[String]) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Tokenizer ---

    #[test]
    fn tokenize_bare_exe() {
        let (prog, args) = tokenize("notepad.exe").unwrap();
        assert_eq!(prog, "notepad.exe");
        assert!(args.is_empty());
    }

    #[test]
    fn tokenize_exe_with_one_arg() {
        let (prog, args) = tokenize("cmd.exe /c").unwrap();
        assert_eq!(prog, "cmd.exe");
        assert_eq!(args, vec!["/c"]);
    }

    #[test]
    fn tokenize_exe_with_multiple_args() {
        let (prog, args) = tokenize("ipconfig /all").unwrap();
        assert_eq!(prog, "ipconfig");
        assert_eq!(args, vec!["/all"]);
    }

    #[test]
    fn tokenize_quoted_path_with_spaces() {
        let (prog, args) =
            tokenize(r#""C:\Program Files\App\app.exe" /flag"#).unwrap();
        assert_eq!(prog, r"C:\Program Files\App\app.exe");
        assert_eq!(args, vec!["/flag"]);
    }

    #[test]
    fn tokenize_quoted_path_no_args() {
        let (prog, args) =
            tokenize(r#""C:\Program Files\My App\tool.exe""#).unwrap();
        assert_eq!(prog, r"C:\Program Files\My App\tool.exe");
        assert!(args.is_empty());
    }

    #[test]
    fn tokenize_empty_string_returns_error() {
        assert!(matches!(tokenize(""), Err(LaunchError::Empty)));
    }

    #[test]
    fn tokenize_whitespace_only_returns_error() {
        assert!(matches!(tokenize("   \t  "), Err(LaunchError::Empty)));
    }

    // --- CREATE_NO_WINDOW constant ---

    #[test]
    fn create_no_window_flag_value() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }

    // --- Error mapping ---

    #[test]
    fn launch_not_found_returns_typed_error() {
        // Use a name that is guaranteed to not exist on any system.
        let result = launch("__winfxstart_definitely_missing_exe_9z8y7x__");
        match result {
            Err(LaunchError::NotFound { program }) => {
                assert_eq!(program, "__winfxstart_definitely_missing_exe_9z8y7x__");
            }
            other => panic!("expected LaunchError::NotFound, got {:?}", other),
        }
    }

    // --- Error display ---

    #[test]
    fn launch_error_display_empty() {
        let msg = format!("{}", LaunchError::Empty);
        assert!(msg.contains("empty"), "got: {}", msg);
    }

    #[test]
    fn launch_error_display_not_found() {
        let err = LaunchError::NotFound {
            program: "foo.exe".into(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("foo.exe"), "got: {}", msg);
    }
}
