//! Background invocations of `clean.py`, mirroring
//! `crate::applications::spawn_report`: one thread, one terminal event on a
//! channel, generation counter checked by the caller's drain loop.

use super::parse::{parse_output, Payload};
use crate::python_runtime;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::process_flags::hide_console_window;

/// Walking every cache root can take minutes on a cold disk — far beyond
/// the 35 s the applications report allows itself. Deletion (`--apply`)
/// gets even more headroom.
const ANALYZE_DEADLINE: Duration = Duration::from_secs(600);
const CLEAN_DEADLINE: Duration = Duration::from_secs(900);

/// How many trailing stderr lines a failure message carries.
const STDERR_TAIL_LINES: usize = 10;

#[derive(Debug)]
pub struct CleanupEvent {
    pub generation: u64,
    pub result: Result<Payload, String>,
}

/// Analyse only: `clean.py --json --level moderate` (plan, no deletion).
pub fn spawn_analyze(generation: u64, sender: Sender<CleanupEvent>) {
    std::thread::spawn(move || {
        let result = run_clean(&["--json", "--level", "moderate"], ANALYZE_DEADLINE);
        let _ = sender.send(CleanupEvent { generation, result });
    });
}

/// Apply one module: `clean.py --only <module> --apply --json`. The safe
/// level is implicit and never prompts; `stdin` is null anyway so any
/// unexpected prompt fails fast (EOF) instead of hanging the thread.
pub fn spawn_clean(module: &str, generation: u64, sender: Sender<CleanupEvent>) {
    let module = module.to_string();
    std::thread::spawn(move || {
        let result = run_clean(&["--only", &module, "--apply", "--json"], CLEAN_DEADLINE);
        let _ = sender.send(CleanupEvent { generation, result });
    });
}

fn run_clean(arguments: &[&str], deadline: Duration) -> Result<Payload, String> {
    let mut command = clean_command(arguments)?;
    run_command(&mut command, deadline)
}

fn clean_command(arguments: &[&str]) -> Result<Command, String> {
    clean_command_from_root(python_runtime::action_root(), arguments)
}

fn clean_command_from_root(root: PathBuf, arguments: &[&str]) -> Result<Command, String> {
    let script = root.join("scripts").join("winclean").join("clean.py");
    if !script.is_file() {
        return Err(format!(
            "script Python winclean/clean.py introuvable sous {}",
            root.display()
        ));
    }
    let interpreter = python_runtime::python_for_script(&script);
    let working_directory = script
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.clone());
    let mut command = Command::new(&interpreter);
    command
        .arg(&script)
        .args(arguments)
        .current_dir(working_directory)
        // Same rationale as `python_runtime::recommendation_command`: a
        // piped stdout on a French Windows install otherwise emits cp1252,
        // which breaks UTF-8 JSON parsing on any accented character.
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_window(&mut command);
    Ok(command)
}

fn run_command(command: &mut Command, deadline: Duration) -> Result<Payload, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("impossible de lancer Python: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout Python indisponible".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr Python indisponible".to_string())?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = std::io::BufReader::new(stdout).read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = std::io::BufReader::new(stderr).read_to_end(&mut bytes);
        bytes
    });
    let expiry = Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < expiry => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("délai global du nettoyage dépassé".to_string());
            }
            Err(error) => return Err(format!("attente du nettoyage impossible: {error}")),
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "lecture stdout interrompue".to_string())?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "lecture stderr interrompue".to_string())?;
    if !status.success() {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "inconnu".to_string());
        return Err(format!(
            "clean.py a échoué (code {code}) : {}",
            stderr_tail(&stderr)
        ));
    }
    parse_output(&String::from_utf8_lossy(&stdout))
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.trim().lines().collect();
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_root_yields_clear_french_error() {
        let missing =
            std::env::temp_dir().join(format!("devtoolbox-cleanup-missing-{}", std::process::id()));
        let error = clean_command_from_root(missing, &["--json"]).unwrap_err();
        assert!(error.contains("introuvable"));
    }

    #[test]
    fn repository_script_resolves_with_script_directory_as_cwd() {
        let command = clean_command(&["--json", "--level", "moderate"]).unwrap();
        assert!(command.get_args().any(|argument| argument == "moderate"));
        assert!(command.get_current_dir().unwrap().ends_with("winclean"));
    }

    #[test]
    fn run_command_captures_a_real_process_and_parses_the_last_document() {
        // A temp file dumped by `type`/`cat` sidesteps shell quoting of the
        // JSON braces and quotes.
        let json = r#"{"level":"safe","apply":false,"candidates":[]}"#;
        let fixture = std::env::temp_dir().join(format!(
            "devtoolbox-cleanup-smoke-{}.json",
            std::process::id()
        ));
        std::fs::write(&fixture, json).unwrap();
        let fixture_path = fixture.display().to_string();
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/C", "type", &fixture_path]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("cat");
            command.arg(&fixture_path);
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let outcome = run_command(&mut command, Duration::from_secs(10));
        let _ = std::fs::remove_file(&fixture);
        match outcome.unwrap() {
            Payload::Plan(plan) => assert_eq!(plan.level, "safe"),
            Payload::Applied { .. } => panic!("no run expected"),
        }
    }

    #[test]
    fn failing_process_reports_exit_code_and_stderr_tail() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/C", "echo boom-stderr 1>&2 & exit 3"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "echo boom-stderr >&2; exit 3"]);
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let error = run_command(&mut command, Duration::from_secs(10)).unwrap_err();
        assert!(error.contains("code 3"), "unexpected error: {error}");
        assert!(error.contains("boom-stderr"), "unexpected error: {error}");
    }

    #[test]
    fn stderr_tail_keeps_only_the_last_lines() {
        let many: String = (0..25).map(|index| format!("line-{index}\n")).collect();
        let tail = stderr_tail(many.as_bytes());
        assert!(tail.starts_with("line-15"));
        assert!(tail.ends_with("line-24"));
    }
}
