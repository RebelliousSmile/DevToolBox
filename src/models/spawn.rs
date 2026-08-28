//! Background workers for model inventory and mutating NDJSON operations.

#![allow(dead_code)]

use super::model::{CatalogSnapshot, ProgressEvent};
use super::parse::{parse_snapshot, ProgressValidator};
use crate::python_runtime;
use std::collections::VecDeque;
use std::io::{BufRead, Read};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const STDERR_TAIL_LINES: usize = 20;
const CANCEL_GRACE: Duration = Duration::from_secs(5);

static MUTATION: OnceLock<Mutex<()>> = OnceLock::new();
static CANCEL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum ModelWorkerEvent {
    Inventory(Result<CatalogSnapshot, String>),
    Json(Result<serde_json::Value, String>),
    Progress(ProgressEvent),
    /// Emitted only after the Python worker and its provider descendants stopped.
    Terminal(Result<ProgressEvent, String>),
}

#[derive(Debug, Clone)]
pub struct CancelHandle {
    path: Arc<PathBuf>,
}

impl CancelHandle {
    pub fn cancel(&self) -> Result<(), String> {
        std::fs::write(self.path.as_ref(), b"cancel\n")
            .map_err(|error| format!("signal d'annulation impossible: {error}"))
    }

    pub fn is_cancelled(&self) -> bool {
        self.path.is_file()
    }
}

pub fn spawn_inventory(sender: Sender<ModelWorkerEvent>) {
    std::thread::spawn(move || {
        let result = inventory_command().and_then(run_inventory);
        let _ = sender.send(ModelWorkerEvent::Inventory(result));
    });
}

/// Run a read-only JSON command without taking the mutation slot.
pub fn spawn_query(arguments: Vec<String>, sender: Sender<ModelWorkerEvent>) {
    std::thread::spawn(move || {
        let result = python_runtime::model_orchestrator_command(arguments).and_then(run_json);
        let _ = sender.send(ModelWorkerEvent::Json(result));
    });
}

/// Run a short JSON mutation under the same global slot as streamed operations.
pub fn spawn_json_mutation(arguments: Vec<String>, sender: Sender<ModelWorkerEvent>) {
    std::thread::spawn(move || {
        let lock = MUTATION.get_or_init(|| Mutex::new(()));
        let result = match lock.try_lock() {
            Ok(_guard) => python_runtime::model_orchestrator_command(arguments).and_then(run_json),
            Err(_) => Err("une autre mutation de modèle est déjà active".to_string()),
        };
        let _ = sender.send(ModelWorkerEvent::Json(result));
    });
}

pub fn spawn_operation(
    operation_id: impl Into<String>,
    arguments: Vec<String>,
    sender: Sender<ModelWorkerEvent>,
) -> CancelHandle {
    let operation_id = operation_id.into();
    let path = Arc::new(cancel_path());
    let handle = CancelHandle { path: path.clone() };
    std::thread::spawn(move || {
        let lock = MUTATION.get_or_init(|| Mutex::new(()));
        let Ok(_guard) = lock.try_lock() else {
            let _ = sender.send(ModelWorkerEvent::Terminal(Err(
                "une autre mutation de modèle est déjà active".to_string(),
            )));
            return;
        };
        let result = operation_command(arguments, path.as_ref())
            .and_then(|command| run_operation(command, &operation_id, path.as_ref(), &sender));
        drop(_guard);
        let _ = sender.send(ModelWorkerEvent::Terminal(result));
        let _ = std::fs::remove_file(path.as_ref());
    });
    handle
}

fn inventory_command() -> Result<Command, String> {
    python_runtime::model_orchestrator_command(["inventory"])
}

fn operation_command(arguments: Vec<String>, cancel_path: &PathBuf) -> Result<Command, String> {
    let mut command = python_runtime::model_orchestrator_command(arguments)?;
    command.env("DEVTOOLBOX_MODEL_CANCEL_FILE", cancel_path);
    configure_process_group(&mut command);
    Ok(command)
}

fn run_inventory(mut command: Command) -> Result<CatalogSnapshot, String> {
    let output = command
        .output()
        .map_err(|error| format!("impossible de lancer l'inventaire modèles: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "inventaire modèles échoué: {}",
            stderr_tail(&output.stderr)
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "sortie inventaire modèles non UTF-8".to_string())?;
    parse_snapshot(stdout.trim())
}

fn run_json(mut command: Command) -> Result<serde_json::Value, String> {
    let output = command
        .output()
        .map_err(|error| format!("impossible de lancer la commande modèles: {error}"))?;
    if !output.status.success() {
        let stdout_message = serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(|message| message.as_str())
                    .map(str::to_string)
            });
        let detail = stdout_message.unwrap_or_else(|| stderr_tail(&output.stderr));
        return Err(format!(
            "commande modèles échouée (code {:?}): {}",
            output.status.code(),
            detail
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("réponse JSON modèles invalide: {error}"))
}

fn run_operation(
    mut command: Command,
    operation_id: &str,
    cancel_path: &PathBuf,
    sender: &Sender<ModelWorkerEvent>,
) -> Result<ProgressEvent, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("impossible de lancer l'opération modèles: {error}"))?;
    let Some(stdout) = child.stdout.take() else {
        terminate_process_tree(&mut child);
        return Err("stdout modèles indisponible".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_process_tree(&mut child);
        return Err("stderr modèles indisponible".to_string());
    };

    let (line_tx, line_rx) = mpsc::channel();
    let stdout_reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_reader = std::thread::spawn(move || read_stderr_tail(stderr));
    let mut validator = ProgressValidator::new(operation_id);
    let mut terminal = None;
    let mut cancel_started = None;

    let status = loop {
        match line_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(line)) => {
                let event = match validator.parse_line(&line) {
                    Ok(event) => event,
                    Err(error) => {
                        terminate_process_tree(&mut child);
                        return Err(error);
                    }
                };
                if event.is_terminal() {
                    terminal = Some(event);
                } else if sender.send(ModelWorkerEvent::Progress(event)).is_err() {
                    let _ = std::fs::write(cancel_path, b"cancel\n");
                }
            }
            Ok(Err(error)) => {
                terminate_process_tree(&mut child);
                return Err(format!("lecture NDJSON modèles impossible: {error}"));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break child
                    .wait()
                    .map_err(|error| format!("attente du worker modèles impossible: {error}"))?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancel_path.is_file() {
            let started = cancel_started.get_or_insert_with(Instant::now);
            if started.elapsed() >= CANCEL_GRACE && child.try_wait().ok().flatten().is_none() {
                terminate_process_tree(&mut child);
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("attente du worker modèles impossible: {error}"))?
        {
            break status;
        }
    };

    let _ = stdout_reader.join();
    let stderr = stderr_reader
        .join()
        .unwrap_or_else(|_| "lecture stderr interrompue".to_string());
    let Some(terminal) = terminal else {
        return Err(format!(
            "worker modèles terminé sans événement terminal (code {:?}): {stderr}",
            status.code()
        ));
    };
    if terminal.kind == "completed" && !status.success() {
        return Err(format!(
            "worker modèles incohérent: succès protocolaire mais code {:?}: {stderr}",
            status.code()
        ));
    }
    Ok(terminal)
}

fn cancel_path() -> PathBuf {
    let sequence = CANCEL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "devtoolbox-model-cancel-{}-{sequence}.flag",
        std::process::id()
    ))
}

fn read_stderr_tail(mut stderr: impl Read) -> String {
    let mut bytes = Vec::new();
    let _ = stderr.read_to_end(&mut bytes);
    stderr_tail(&bytes)
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let mut lines = VecDeque::with_capacity(STDERR_TAIL_LINES);
    for line in text.lines() {
        if lines.len() == STDERR_TAIL_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }
    lines.into_iter().collect::<Vec<_>>().join("\n")
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(crate::process_flags::CREATE_NO_WINDOW | 0x0000_0200);
}

#[cfg(unix)]
fn terminate_process_tree(child: &mut Child) {
    signal_process_group(child.id(), "-TERM");
    let expiry = Instant::now() + Duration::from_secs(2);
    while Instant::now() < expiry {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    signal_process_group(child.id(), "-KILL");
    let _ = child.wait();
}

#[cfg(unix)]
fn signal_process_group(process_id: u32, signal_name: &str) {
    let mut signal = Command::new("kill");
    signal.args([signal_name, "--", &format!("-{process_id}")]);
    let _ = signal.status();
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut Child) {
    let mut taskkill = Command::new("taskkill");
    taskkill.args(["/PID", &child.id().to_string(), "/T", "/F"]);
    crate::process_flags::hide_console_window(&mut taskkill);
    let _ = taskkill.status();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    static TEST_MUTATION: OnceLock<Mutex<()>> = OnceLock::new();

    fn receive_terminal(receiver: Receiver<ModelWorkerEvent>) -> Result<ProgressEvent, String> {
        loop {
            match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
                ModelWorkerEvent::Terminal(result) => return result,
                ModelWorkerEvent::Progress(_)
                | ModelWorkerEvent::Inventory(_)
                | ModelWorkerEvent::Json(_) => {}
            }
        }
    }

    #[test]
    fn python_generated_snapshot_deserializes() {
        let command = python_runtime::model_orchestrator_command(["fixture"]).unwrap();
        let snapshot = run_inventory(command).unwrap();
        assert_eq!(snapshot.schema_version, 1);
        assert_eq!(snapshot.artifacts[0].artifact_id, "fixture-gguf");
    }

    #[test]
    fn python_generated_event_fixture_is_validated_asynchronously() {
        let _serial = TEST_MUTATION
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (sender, receiver) = mpsc::channel();
        let _cancel = spawn_operation(
            "fixture-operation",
            vec![
                "event-fixture".into(),
                "--operation-id".into(),
                "fixture-operation".into(),
            ],
            sender,
        );
        let terminal = receive_terminal(receiver).unwrap();
        assert_eq!(terminal.kind, "completed");
        assert_eq!(terminal.transferred_bytes, Some(1024));
    }

    #[test]
    fn inventory_runs_without_taking_the_mutation_slot() {
        let (sender, receiver) = mpsc::channel();
        spawn_inventory(sender);
        match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
            ModelWorkerEvent::Inventory(Ok(snapshot)) => assert_eq!(snapshot.schema_version, 1),
            other => panic!("unexpected inventory outcome: {other:?}"),
        }
    }

    #[test]
    fn read_only_json_query_returns_typed_value() {
        let (sender, receiver) = mpsc::channel();
        spawn_query(vec!["schema".into()], sender);
        match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
            ModelWorkerEvent::Json(Ok(value)) => assert_eq!(value["schema_version"], 1),
            other => panic!("unexpected query outcome: {other:?}"),
        }
    }

    #[test]
    fn a_second_mutation_is_rejected_while_inventory_remains_available() {
        let _serial = TEST_MUTATION
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lock = MUTATION.get_or_init(|| Mutex::new(()));
        let _guard = lock.lock().unwrap();
        let (sender, receiver) = mpsc::channel();
        let _cancel = spawn_operation(
            "blocked",
            vec![
                "event-fixture".into(),
                "--operation-id".into(),
                "blocked".into(),
            ],
            sender,
        );
        assert!(receive_terminal(receiver)
            .unwrap_err()
            .contains("déjà active"));

        let (sender, receiver) = mpsc::channel();
        spawn_inventory(sender);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(10)).unwrap(),
            ModelWorkerEvent::Inventory(Ok(_))
        ));
    }

    #[test]
    fn cancellation_is_routed_and_terminal_waits_for_descendants() {
        let _serial = TEST_MUTATION
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (sender, receiver) = mpsc::channel();
        let cancel = spawn_operation(
            "fixture-cancel",
            vec![
                "cancel-fixture".into(),
                "--operation-id".into(),
                "fixture-cancel".into(),
            ],
            sender,
        );
        match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
            ModelWorkerEvent::Progress(event) => assert_eq!(event.kind, "schema"),
            other => panic!("unexpected first event: {other:?}"),
        }
        cancel.cancel().unwrap();
        let terminal = receive_terminal(receiver).unwrap();
        assert_eq!(terminal.kind, "cancelled");
    }
}
