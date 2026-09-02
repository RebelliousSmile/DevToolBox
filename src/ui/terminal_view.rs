//! Terminal view backend — cross-platform port of `src/ui/app.rs`'s Terminal
//! panel (`append_terminal`, `feed_terminal_text`, `sync_terminal_listbox`,
//! and the `windows::process::launch_captured`/`ActionEvent` streaming
//! pipeline it consumed).
//!
//! Deliberately does **not** reuse `crate::windows::process` — that module
//! (including `ActionEvent`, `tokenize`, `launch_captured`, `resolve_action`)
//! is `#[cfg(windows)]`-gated end to end via `src/windows/mod.rs`'s
//! `#![cfg(windows)]`, and this view must launch real processes and stream
//! their output identically on Linux. This file re-implements the same
//! shape (tokenize → resolve `@python` actions → spawn with piped
//! stdout/stderr → stream lines → finished/failed) using plain
//! `std::process::Command`. On Windows the spawn still needs
//! `CREATE_NO_WINDOW` (`#[cfg(windows)]`-gated below): without it, a
//! console subsystem child (python.exe…) spawned from this GUI app opens
//! its own empty console window, and closing that window kills the child
//! with `STATUS_CONTROL_C_EXIT` (0xC000013A) — not cosmetic, since all
//! output is already captured by the integrated terminal.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::Sender;

/// Buffer caps — same thresholds as `app.rs`'s `MAX_TERMINAL_LINES` /
/// `TRIMMED_TERMINAL_LINES`.
const MAX_LINES: usize = 5_000;
const TRIMMED_LINES: usize = 3_500;

/// Streaming events from a launched process — cross-platform analogue of
/// `crate::windows::process::ActionEvent` (same variant shape, minus
/// `AutomationsLoaded` which isn't relevant here).
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    #[allow(dead_code)]
    Started {
        command: String,
        pid: u32,
    },
    Output(String),
    Finished {
        code: Option<i32>,
    },
    Failed(String),
}

/// Parse a raw command string into `(program, args)` — quote/space-aware,
/// mirroring `crate::windows::process::tokenize`'s rules (duplicated rather
/// than reused: that function lives in the `#[cfg(windows)]`-gated
/// `windows::process` module, unavailable on Linux).
pub fn tokenize(command: &str) -> Result<(String, Vec<String>), String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("la commande est vide".to_string());
    }

    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in command.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    let mut iter = tokens.into_iter();
    // `command` was already checked non-empty above, so at least one token exists.
    let program = iter
        .next()
        .expect("non-empty command yields at least one token");
    Ok((program, iter.collect()))
}

/// Resolved (program, args, working directory) for a real spawn — mirrors
/// `crate::windows::process::ActionSpec`.
struct ActionSpec {
    program: String,
    args: Vec<String>,
    working_directory: Option<PathBuf>,
}

/// Locate the DevToolBox root used to resolve bundled scripts — same rules
/// as `crate::windows::process::action_root` (duplicated for the same
/// `#[cfg(windows)]`-gating reason as the rest of this file): `DEVTOOLBOX_HOME`
/// takes priority; otherwise ancestors of the current working directory and
/// executable are searched for a `scripts` directory.
fn action_root() -> PathBuf {
    crate::python_runtime::action_root()
}

/// `true` if an executable named `name` exists somewhere on `PATH` —
/// used only to pick between the `python3`/`python` fallbacks below (no
/// need for this on Windows, where `python3` is not the norm, hence no
/// equivalent in `crate::windows::process::resolve_action`).
#[cfg(test)]
fn exists_on_path(name: &str) -> bool {
    crate::python_runtime::exists_on_path(name)
}

/// Resolve `@python <script> [args...]` into a real program/args/cwd,
/// mirroring `crate::windows::process::resolve_action`'s cascade (frozen
/// master-plan decision 10) adapted for Linux:
/// 1. The script directory's local venv interpreter, in the host
///    platform's layout — `.venv/bin/python` on POSIX,
///    `.venv\Scripts\python.exe` on Windows. Both are handled by
///    `crate::python_runtime::python_for_script`, which this cascade
///    delegates to.
/// 2. `DEVTOOLBOX_PYTHON` env var.
/// 3. `python3` on `PATH`.
/// 4. `python` on `PATH` — extra fallback for minimal distros that don't
///    ship a `python3` symlink (decision 10's Linux addendum; Windows'
///    `resolve_action` has no equivalent since `python3` isn't the norm
///    there).
///
/// Non-`@python` commands pass through unchanged, same as the Windows
/// cascade.
#[cfg(test)]
fn resolve_action(command: &str, root: &Path) -> Result<ActionSpec, String> {
    resolve_action_with_user_scripts(command, root, None)
}

fn resolve_action_with_user_scripts(
    command: &str,
    bundled_root: &Path,
    user_scripts_directory: Option<&Path>,
) -> Result<ActionSpec, String> {
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
        .ok_or_else(|| "@python requires a script path".to_string())?;
    let script_path = if Path::new(script).is_absolute() {
        PathBuf::from(script)
    } else {
        let bundled_candidate = bundled_root.join(script);
        if bundled_candidate.is_file() {
            bundled_candidate
        } else if let Some(user_root) = user_scripts_directory {
            user_root.join(script)
        } else {
            bundled_candidate
        }
    };
    if !script_path.is_file() {
        return Err(format!(
            "Python script not found: {}",
            script_path.display()
        ));
    }
    let working_directory = script_path.parent().map(Path::to_path_buf);
    let python = crate::python_runtime::python_for_script(&script_path);
    // `-u`: unbuffered stdout/stderr. Piped (non-tty) output is otherwise
    // block-buffered by Python, so a long-running script would show nothing
    // in the integrated terminal until it exits.
    let mut resolved_args = vec!["-u".to_string(), script_path.display().to_string()];
    resolved_args.extend(script_args.iter().cloned());
    Ok(ActionSpec {
        program: python,
        args: resolved_args,
        working_directory,
    })
}

fn stream_output<R: Read + Send + 'static>(
    stream: R,
    sender: Sender<TerminalEvent>,
) -> std::thread::JoinHandle<()> {
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
                    let _ = sender.send(TerminalEvent::Output(
                        String::from_utf8_lossy(&buffer).into_owned(),
                    ));
                }
                Err(error) => {
                    let _ = sender.send(TerminalEvent::Failed(format!(
                        "lecture de la sortie impossible: {error}"
                    )));
                    break;
                }
            }
        }
    })
}

/// Launch `command` in the background, streaming [`TerminalEvent`]s to
/// `sender` as it runs. Returns the spawned process's pid on success.
///
/// `command` is resolved through [`resolve_action`] first, so `@python
/// <script> [args...]` commands (the 14 bundled actions in
/// `config/builtin-actions.json`) are transparently rewritten to a real
/// Python interpreter invocation before spawning — every other command
/// passes through unchanged.
#[cfg(test)]
pub fn launch_captured(command: &str, sender: Sender<TerminalEvent>) -> Result<u32, String> {
    launch_captured_with_user_scripts(command, None, sender)
}

/// Launch an action while resolving missing relative Python paths against a
/// user-selected scripts directory. Bundled paths that exist below the
/// distribution root always win, keeping internal tools isolated.
pub fn launch_captured_with_user_scripts(
    command: &str,
    user_scripts_directory: Option<&Path>,
    sender: Sender<TerminalEvent>,
) -> Result<u32, String> {
    let spec = resolve_action_with_user_scripts(command, &action_root(), user_scripts_directory)?;
    launch_captured_program(
        &spec.program,
        &spec.args,
        spec.working_directory.as_deref(),
        command,
        sender,
    )
}

/// Launch an already-resolved `program args...`, streaming the same
/// [`TerminalEvent`]s to `sender`. Returns the spawned process's pid.
///
/// Split out of [`launch_captured`] for the Docker tab's compose commands
/// (Part 2): their argv is *built*, not parsed — a compose file path can hold
/// spaces, and round-tripping it through a command **string** just so
/// [`resolve_action`] can split it again would be a quoting bug waiting to
/// happen. `label` is what the `Started` event reports, purely for display.
///
/// `working_dir` matters for compose specifically: `up -d` resolves relative
/// build contexts and `env_file` paths against the *current* directory when
/// the compose file is given as an absolute `-f`, so the caller passes the
/// file's own directory.
pub fn launch_captured_program(
    program: &str,
    args: &[String],
    working_dir: Option<&Path>,
    label: &str,
    sender: Sender<TerminalEvent>,
) -> Result<u32, String> {
    let mut process = std::process::Command::new(program);
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Kept from the Windows branch through the compose extraction: without
    // it a console-subsystem child opens its own window, and closing that
    // window kills the child (see this module's header).
    crate::process_flags::hide_console_window(&mut process);
    if let Some(directory) = working_dir {
        process.current_dir(directory);
    }

    let mut child = process
        .spawn()
        .map_err(|error| format!("{program}: {error}"))?;
    let pid = child.id();

    let _ = sender.send(TerminalEvent::Started {
        command: label.to_string(),
        pid,
    });

    let mut reader_handles = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        reader_handles.push(stream_output(stdout, sender.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        reader_handles.push(stream_output(stderr, sender.clone()));
    }

    std::thread::spawn(move || {
        let status = child.wait();
        // Join the stdout/stderr reader threads before signalling completion so
        // every `Output` event is guaranteed to have been sent before `Finished`/
        // `Failed` — otherwise the unsynchronized sender threads can race and
        // deliver `Finished` first, especially under scheduling contention.
        for handle in reader_handles {
            let _ = handle.join();
        }
        match status {
            Ok(status) => {
                let _ = sender.send(TerminalEvent::Finished {
                    code: status.code(),
                });
            }
            Err(error) => {
                let _ = sender.send(TerminalEvent::Failed(error.to_string()));
            }
        }
    });

    Ok(pid)
}

/// Split `text` into complete lines pushed to `lines`, buffering any
/// trailing partial line in `partial`. Same rules as
/// `app.rs::feed_terminal_text`: `\r` is dropped, `\n` flushes the pending
/// line.
#[allow(dead_code)]
pub fn feed_text(lines: &mut VecDeque<String>, partial: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '\r' => {}
            '\n' => lines.push_back(std::mem::take(partial)),
            _ => partial.push(ch),
        }
    }
}

/// Trim `lines` down once it grows past the caps — same two-stage rule as
/// `app.rs::sync_terminal_listbox`'s trimming half (that function's other
/// half kept a Win32 listbox control in sync; this view re-renders directly
/// from the `VecDeque` every frame instead, so there's no listbox to sync).
pub fn trim_lines(lines: &mut VecDeque<String>) {
    while lines.len() > MAX_LINES {
        lines.pop_front();
    }
    while lines.len() > TRIMMED_LINES {
        lines.pop_front();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn feed_text_splits_crlf_lines() {
        let mut lines = VecDeque::new();
        let mut partial = String::new();
        feed_text(&mut lines, &mut partial, "alpha\r\nbeta\nchar");
        assert_eq!(lines.into_iter().collect::<Vec<_>>(), vec!["alpha", "beta"]);
        assert_eq!(partial, "char");
    }

    #[test]
    fn tokenize_splits_on_whitespace_respecting_quotes() {
        let (program, args) = tokenize("\"/opt/my app/run\" --flag value").unwrap();
        assert_eq!(program, "/opt/my app/run");
        assert_eq!(args, vec!["--flag", "value"]);
    }

    #[test]
    fn tokenize_rejects_empty_command() {
        assert!(tokenize("   ").is_err());
    }

    #[test]
    fn trim_lines_caps_buffer_once_over_max() {
        let mut lines: VecDeque<String> = (0..MAX_LINES + 1).map(|i| i.to_string()).collect();
        trim_lines(&mut lines);
        assert_eq!(lines.len(), TRIMMED_LINES);
    }

    #[test]
    fn trim_lines_also_trims_when_over_trimmed_cap_but_under_max() {
        // Mirrors app.rs's actual (slightly surprising) behavior: the trim
        // runs down to TRIMMED_LINES any time the buffer exceeds it, not
        // only once MAX_LINES is exceeded.
        let mut lines: VecDeque<String> = (0..TRIMMED_LINES + 50).map(|i| i.to_string()).collect();
        trim_lines(&mut lines);
        assert_eq!(lines.len(), TRIMMED_LINES);
    }

    /// Spawns a REAL `echo` process (a real coreutils binary on this Linux
    /// dev machine, not a shell builtin-only construct) and asserts its
    /// actual captured stdout arrives through the event channel — end-to-end
    /// verification of the streaming pipeline against a real OS process.
    #[test]
    fn launch_captured_streams_real_process_output() {
        let (tx, rx) = channel();
        #[cfg(windows)]
        let command = "cmd.exe /C echo hello-terminal-view";
        #[cfg(not(windows))]
        let command = "echo hello-terminal-view";
        launch_captured(command, tx).expect("spawn echo");

        let mut saw_output = false;
        let mut finished = false;
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(5)) {
            match event {
                TerminalEvent::Output(line) => {
                    if line.contains("hello-terminal-view") {
                        saw_output = true;
                    }
                }
                TerminalEvent::Finished { code } => {
                    assert_eq!(code, Some(0));
                    finished = true;
                    break;
                }
                TerminalEvent::Failed(err) => panic!("process failed: {err}"),
                TerminalEvent::Started { .. } => {}
            }
        }
        assert!(saw_output, "expected the real echoed output to be captured");
        assert!(finished, "expected a Finished event");
    }

    #[test]
    fn launch_captured_reports_failure_for_a_nonexistent_program() {
        let (tx, rx) = channel();
        let result = launch_captured("devtoolbox-definitely-not-a-real-binary --x", tx);
        assert!(
            result.is_err(),
            "spawning a nonexistent program must fail, not panic"
        );
        drop(rx);
    }

    #[test]
    fn legacy_command_resolves_unchanged() {
        let spec = resolve_action("ip addr", Path::new("/unused")).unwrap();
        assert_eq!(spec.program, "ip");
        assert_eq!(spec.args, vec!["addr"]);
        assert_eq!(spec.working_directory, None);
    }

    #[test]
    fn python_action_requires_script_path() {
        assert!(resolve_action("@python", Path::new("/unused")).is_err());
    }

    #[test]
    fn python_action_resolves_script_and_working_directory() {
        let temp =
            std::env::temp_dir().join(format!("devtoolbox_terminal_view_{}", std::process::id()));
        let script_dir = temp.join("scripts").join("sample");
        std::fs::create_dir_all(&script_dir).unwrap();
        let script = script_dir.join("sample.py");
        std::fs::write(&script, "print('ok')").unwrap();

        let spec = resolve_action(
            "@python scripts/sample/sample.py config.yaml --only pro",
            &temp,
        )
        .unwrap();
        assert_eq!(spec.args[0], "-u");
        // Path comparison (component-wise) rather than string comparison:
        // this resolver keeps the config's `/` separators as-is, so on
        // Windows the resolved string mixes `\` and `/`.
        assert_eq!(Path::new(&spec.args[1]), script.as_path());
        assert_eq!(&spec.args[2..], ["config.yaml", "--only", "pro"]);
        assert_eq!(spec.working_directory, Some(script_dir));
        assert!(
            spec.program == "python3" || spec.program == "python",
            "expected a PATH-resolved interpreter (no local .venv here), got: {}",
            spec.program
        );

        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn user_scripts_directory_resolves_custom_paths_without_shadowing_bundled_tools() {
        let temp = std::env::temp_dir().join(format!(
            "devtoolbox_terminal_user_scripts_{}",
            std::process::id()
        ));
        let bundled_script = temp.join("distribution/scripts/builtin.py");
        let user_script = temp.join("user-scripts/custom.py");
        let shadow_script = temp.join("user-scripts/scripts/builtin.py");
        for script in [&bundled_script, &user_script, &shadow_script] {
            std::fs::create_dir_all(script.parent().unwrap()).unwrap();
            std::fs::write(script, "print('ok')").unwrap();
        }

        let custom = resolve_action_with_user_scripts(
            "@python custom.py",
            &temp.join("distribution"),
            Some(&temp.join("user-scripts")),
        )
        .unwrap();
        assert_eq!(Path::new(&custom.args[1]), user_script.as_path());

        let bundled = resolve_action_with_user_scripts(
            "@python scripts/builtin.py",
            &temp.join("distribution"),
            Some(&temp.join("user-scripts")),
        )
        .unwrap();
        assert_eq!(Path::new(&bundled.args[1]), bundled_script.as_path());

        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn python_action_prefers_local_venv_interpreter() {
        let temp = std::env::temp_dir().join(format!(
            "devtoolbox_terminal_view_venv_{}",
            std::process::id()
        ));
        let script_dir = temp.join("scripts").join("sample");
        std::fs::create_dir_all(&script_dir).unwrap();
        std::fs::write(script_dir.join("sample.py"), "print('ok')").unwrap();
        // `python_for_script` looks for the interpreter in the host
        // platform's venv layout, so the fixture has to be laid out the same
        // way or the cascade falls through to the `PATH` interpreter and the
        // assertion below compares a bare `python3` against a full path.
        // Only `is_file()` is checked, so the contents are irrelevant —
        // nothing is executed here.
        #[cfg(windows)]
        let (venv_bin, interpreter) = (script_dir.join(".venv").join("Scripts"), "python.exe");
        #[cfg(not(windows))]
        let (venv_bin, interpreter) = (script_dir.join(".venv").join("bin"), "python");
        std::fs::create_dir_all(&venv_bin).unwrap();
        let venv_python = venv_bin.join(interpreter);
        std::fs::write(&venv_python, "#!/bin/sh\n").unwrap();

        let spec = resolve_action("@python scripts/sample/sample.py", &temp).unwrap();
        // Compared as paths, not as strings: `resolve_action` builds the
        // script path by joining the command's own `scripts/sample/…`
        // spelling onto `root`, so on Windows the result keeps those forward
        // slashes while `venv_python` was assembled with `join` and carries
        // backslashes. Both name the same file, and `Path`'s comparison
        // (component-wise, and `/` is a separator on Windows too) says so
        // while a string comparison would not.
        assert_eq!(Path::new(&spec.program), venv_python.as_path());

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

    /// Real, non-mocked end-to-end check: spawns an actual throwaway `.py`
    /// script through `launch_captured`'s `@python` cascade and asserts its
    /// real stdout arrives — proves `@python` commands work through the
    /// live UI launch path on this machine, not just that `resolve_action`
    /// returns a plausible-looking spec.
    #[test]
    fn launch_captured_runs_a_real_python_action_end_to_end() {
        if !exists_on_path("python3") && !exists_on_path("python") {
            eprintln!("skipping: no python3/python interpreter on PATH");
            return;
        }
        let temp = std::env::temp_dir().join(format!(
            "devtoolbox_terminal_view_e2e_{}",
            std::process::id()
        ));
        let script_dir = temp.join("scripts");
        std::fs::create_dir_all(&script_dir).unwrap();
        std::fs::write(
            script_dir.join("hello.py"),
            "print('hello-from-python-action')\n",
        )
        .unwrap();

        let command = format!("@python {}", script_dir.join("hello.py").display());
        let (tx, rx) = channel();
        launch_captured(&command, tx).expect("spawn @python action");

        let mut saw_output = false;
        let mut finished = false;
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(5)) {
            match event {
                TerminalEvent::Output(line) => {
                    if line.contains("hello-from-python-action") {
                        saw_output = true;
                    }
                }
                TerminalEvent::Finished { code } => {
                    assert_eq!(code, Some(0));
                    finished = true;
                    break;
                }
                TerminalEvent::Failed(err) => panic!("python action failed: {err}"),
                TerminalEvent::Started { .. } => {}
            }
        }
        assert!(
            saw_output,
            "expected the real python script's stdout to be captured"
        );
        assert!(finished, "expected a Finished event");

        let _ = std::fs::remove_dir_all(temp);
    }
}
