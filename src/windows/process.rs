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

// `launch` (fire-and-forget) + `LaunchOutcome` are exercised by tests; the live
// UI path uses `launch_captured`. Keep both available without dead-code noise.
#![allow(dead_code)]

use serde::Deserialize;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::Sender;

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
    /// A structured action is malformed or references a missing script.
    InvalidAction {
        /// Human-readable validation failure.
        message: String,
    },
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
            LaunchError::InvalidAction { message } => write!(f, "invalid action: {message}"),
            LaunchError::NotFound { program } => {
                write!(f, "executable not found: {}", program)
            }
            LaunchError::Spawn { source } => {
                write!(f, "failed to spawn process: {}", source)
            }
        }
    }
}

#[derive(Debug, PartialEq)]
struct ActionSpec {
    program: String,
    args: Vec<String>,
    working_directory: Option<PathBuf>,
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

#[derive(Debug, Clone)]
pub enum ActionEvent {
    Started { command: String, pid: u32 },
    Output(String),
    Finished { code: Option<i32> },
    Failed(String),
    AutomationsLoaded(Result<Vec<ScheduledTask>, String>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScheduledTask {
    pub name: String,
    pub category: String,
    pub next_run: String,
    pub state: String,
    pub author: String,
}

fn stream_output<R: Read + Send + 'static>(stream: R, sender: Sender<ActionEvent>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {
                    while matches!(buffer.last(), Some(b'\n' | b'\r')) {
                        buffer.pop();
                    }
                    let _ = sender.send(ActionEvent::Output(
                        String::from_utf8_lossy(&buffer).into_owned(),
                    ));
                }
                Err(error) => {
                    let _ = sender.send(ActionEvent::Failed(format!(
                        "lecture de la sortie impossible: {error}"
                    )));
                    break;
                }
            }
        }
    });
}

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
    let root = action_root();
    let spec = resolve_action(command, &root)?;

    log::info!(
        "Launching: program={:?} args={:?} cwd={:?}",
        spec.program,
        spec.args,
        spec.working_directory
    );

    let result = build_action_command(&spec).spawn();

    match result {
        Ok(child) => {
            log::info!("Launched successfully (program={:?})", spec.program);
            Ok(child)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            log::warn!("Executable not found: {:?}", spec.program);
            Err(LaunchError::NotFound {
                program: spec.program,
            })
        }
        Err(e) => {
            log::error!("Spawn failed for {:?}: {}", spec.program, e);
            Err(LaunchError::Spawn { source: e })
        }
    }
}

/// Launch an action in the background and stream stdout/stderr as events.
pub fn launch_captured(command: &str, sender: Sender<ActionEvent>) -> Result<u32, LaunchError> {
    let spec = resolve_action(command, &action_root())?;
    let mut process = build_action_command(&spec);
    process.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = process.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            LaunchError::NotFound {
                program: spec.program.clone(),
            }
        } else {
            LaunchError::Spawn { source: error }
        }
    })?;
    let pid = child.id();
    let _ = sender.send(ActionEvent::Started {
        command: command.to_string(),
        pid,
    });

    if let Some(stdout) = child.stdout.take() {
        stream_output(stdout, sender.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        stream_output(stderr, sender.clone());
    }
    std::thread::spawn(move || match child.wait() {
        Ok(status) => {
            let _ = sender.send(ActionEvent::Finished {
                code: status.code(),
            });
        }
        Err(error) => {
            let _ = sender.send(ActionEvent::Failed(error.to_string()));
        }
    });
    Ok(pid)
}

/// Locate the WinFXStart root used to resolve bundled scripts.
///
/// `WINFXSTART_HOME` has priority. Otherwise, ancestors of the current working
/// directory and executable are searched for a `scripts` directory. This works
/// both from the repository and from `target/debug` / `target/release`.
fn action_root() -> PathBuf {
    if let Some(root) = std::env::var_os("WINFXSTART_HOME") {
        return PathBuf::from(root);
    }

    let mut candidates = Vec::new();
    if let Ok(current) = std::env::current_dir() {
        candidates.push(current);
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.to_path_buf());
        }
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

fn resolve_action(command: &str, root: &Path) -> Result<ActionSpec, LaunchError> {
    let (program, args) = tokenize(command)?;
    if program != "@python" {
        return Ok(ActionSpec {
            program,
            args,
            working_directory: None,
        });
    }

    let (script, script_args) = args
        .split_first()
        .ok_or_else(|| LaunchError::InvalidAction {
            message: "@python requires a script path".to_string(),
        })?;
    let normalized_script: String = script
        .chars()
        .map(|character| {
            if character == '/' {
                std::path::MAIN_SEPARATOR
            } else {
                character
            }
        })
        .collect();
    let script_path = if Path::new(&normalized_script).is_absolute() {
        PathBuf::from(&normalized_script)
    } else {
        root.join(&normalized_script)
    };
    if !script_path.is_file() {
        return Err(LaunchError::InvalidAction {
            message: format!("Python script not found: {}", script_path.display()),
        });
    }
    let working_directory = script_path.parent().map(Path::to_path_buf);
    let local_python = working_directory
        .as_ref()
        .map(|directory| directory.join(".venv").join("Scripts").join("python.exe"))
        .filter(|candidate| candidate.is_file());
    let python = local_python
        .map(|path| path.display().to_string())
        .or_else(|| std::env::var("WINFXSTART_PYTHON").ok())
        .unwrap_or_else(|| "python3".to_string());
    let mut resolved_args = vec![script_path.display().to_string()];
    resolved_args.extend(script_args.iter().cloned());
    Ok(ActionSpec {
        program: python,
        args: resolved_args,
        working_directory,
    })
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

fn build_action_command(spec: &ActionSpec) -> std::process::Command {
    let mut command = build_command(&spec.program, &spec.args);
    if let Some(directory) = &spec.working_directory {
        command.current_dir(directory);
    }
    command
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
        let (prog, args) = tokenize(r#""C:\Program Files\App\app.exe" /flag"#).unwrap();
        assert_eq!(prog, r"C:\Program Files\App\app.exe");
        assert_eq!(args, vec!["/flag"]);
    }

    #[test]
    fn tokenize_quoted_path_no_args() {
        let (prog, args) = tokenize(r#""C:\Program Files\My App\tool.exe""#).unwrap();
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

    #[test]
    fn legacy_command_resolves_unchanged() {
        let spec = resolve_action("ipconfig /all", Path::new("C:/unused")).unwrap();
        assert_eq!(spec.program, "ipconfig");
        assert_eq!(spec.args, vec!["/all"]);
        assert_eq!(spec.working_directory, None);
    }

    #[test]
    fn python_action_requires_script_path() {
        assert!(matches!(
            resolve_action("@python", Path::new("C:/unused")),
            Err(LaunchError::InvalidAction { .. })
        ));
    }

    #[test]
    fn python_action_resolves_script_and_working_directory() {
        let temp = std::env::temp_dir().join(format!("winfxstart_action_{}", std::process::id()));
        let script_dir = temp.join("scripts").join("sample");
        std::fs::create_dir_all(&script_dir).unwrap();
        let script = script_dir.join("sample.py");
        std::fs::write(&script, "print('ok')").unwrap();

        let spec = resolve_action(
            "@python scripts/sample/sample.py config.yaml --only pro",
            &temp,
        )
        .unwrap();
        assert_eq!(spec.args[0], script.display().to_string());
        assert_eq!(&spec.args[1..], ["config.yaml", "--only", "pro"]);
        assert_eq!(spec.working_directory, Some(script_dir));

        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn bundled_python_actions_reference_existing_scripts() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../config/builtin-actions.json")).unwrap();
        let actions: Vec<crate::storage::Command> =
            serde_json::from_value(catalog.get("commands").cloned().unwrap_or_default()).unwrap();
        let python_actions: Vec<_> = actions
            .iter()
            .filter(|command| command.command.starts_with("@python"))
            .collect();
        assert_eq!(python_actions.len(), 14);
        for action in python_actions {
            resolve_action(&action.command, root)
                .unwrap_or_else(|error| panic!("invalid action {}: {error}", action.id));
        }
    }
}


