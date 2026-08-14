//! Application recommendation support shared by the Python report and egui.

#![allow(dead_code)]

use serde::Deserialize;
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
pub mod usage;
#[cfg(windows)]
mod windows;

#[allow(unused_imports)]
pub use usage::{UsageService, UsageTarget};

pub trait ProcessProvider: Send + Sync + 'static {
    fn executable_paths(&self) -> io::Result<Vec<PathBuf>>;
}

pub struct SystemProcessProvider;

impl ProcessProvider for SystemProcessProvider {
    fn executable_paths(&self) -> io::Result<Vec<PathBuf>> {
        #[cfg(target_os = "linux")]
        {
            linux::executable_paths()
        }
        #[cfg(windows)]
        {
            windows::executable_paths()
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct SizeEvidence {
    pub installed_bytes: Option<u64>,
    pub reclaimable_bytes: Option<u64>,
    pub method: String,
    pub scope: String,
    pub confidence: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UsageEvidence {
    pub kind: String,
    pub last_seen: Option<String>,
    pub tracked_since: Option<String>,
    pub covered_days: u32,
    pub confidence: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Protection {
    pub protected: bool,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CommandSuggestion {
    pub value: String,
    pub origin: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApplicationCandidate {
    pub app_id: String,
    pub source: String,
    pub name: String,
    pub size: SizeEvidence,
    #[serde(default)]
    pub executable_hints: Vec<String>,
    pub usage: UsageEvidence,
    pub protection: Protection,
    pub command: Option<CommandSuggestion>,
    pub score: u32,
    pub confidence: String,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SourceError {
    pub source: String,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RecommendationReport {
    pub schema_version: u32,
    pub generated_at: String,
    pub platform: String,
    #[serde(default)]
    pub candidates: Vec<ApplicationCandidate>,
    #[serde(default)]
    pub source_errors: Vec<SourceError>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl RecommendationReport {
    pub fn usage_targets(&self) -> Vec<UsageTarget> {
        self.candidates
            .iter()
            .filter(|candidate| !candidate.executable_hints.is_empty())
            .map(|candidate| UsageTarget {
                app_id: candidate.app_id.clone(),
                executable_hints: candidate
                    .executable_hints
                    .iter()
                    .map(PathBuf::from)
                    .collect(),
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct ReportEvent {
    pub generation: u64,
    pub result: Result<RecommendationReport, String>,
}

pub fn spawn_report(generation: u64, history_path: PathBuf, sender: Sender<ReportEvent>) {
    std::thread::spawn(move || {
        let result = run_report(&history_path);
        let _ = sender.send(ReportEvent { generation, result });
    });
}

fn run_report(history_path: &std::path::Path) -> Result<RecommendationReport, String> {
    let mut command = crate::python_runtime::recommendation_command(history_path)?;
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
    let deadline = Instant::now() + Duration::from_secs(35);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("délai global du rapport dépassé".to_string());
            }
            Err(error) => return Err(format!("attente du rapport impossible: {error}")),
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "lecture stdout interrompue".to_string())?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "lecture stderr interrompue".to_string())?;
    if !status.success() {
        return Err(String::from_utf8_lossy(&stderr).trim().to_string());
    }
    parse_report(&stdout)
}

fn parse_report(bytes: &[u8]) -> Result<RecommendationReport, String> {
    let report: RecommendationReport =
        serde_json::from_slice(bytes).map_err(|error| format!("rapport JSON invalide: {error}"))?;
    if report.schema_version != 1 {
        return Err(format!(
            "version de rapport incompatible: {}",
            report.schema_version
        ));
    }
    Ok(report)
}

#[cfg(test)]
mod report_tests {
    use super::*;

    #[test]
    fn fixture_deserializes_and_builds_usage_targets() {
        let json = r#"{
          "schema_version":1,"generated_at":"2026-01-01T00:00:00Z","platform":"linux",
          "candidates":[{"app_id":"apt:editor","source":"apt","name":"Editor",
          "size":{"installed_bytes":10,"reclaimable_bytes":null,"method":"dpkg","scope":"package","confidence":"high"},
          "executable_hints":["/usr/bin/editor"],
          "usage":{"kind":"unknown","last_seen":null,"tracked_since":null,"covered_days":0,"confidence":"unknown"},
          "protection":{"protected":false,"reasons":[]},"command":null,"score":0,"confidence":"low","reasons":[],"metadata":{}}],
          "source_errors":[],"warnings":[]}"#;
        let report: RecommendationReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.usage_targets().len(), 1);
        assert_eq!(report.usage_targets()[0].app_id, "apt:editor");
    }

    #[test]
    fn invalid_json_and_schema_are_rejected() {
        assert!(parse_report(b"not json")
            .unwrap_err()
            .contains("JSON invalide"));
        let incompatible = br#"{"schema_version":2,"generated_at":"now","platform":"test","candidates":[],"source_errors":[],"warnings":[]}"#;
        assert!(parse_report(incompatible)
            .unwrap_err()
            .contains("version de rapport incompatible"));
    }

    #[test]
    fn python_generated_contract_fixture_deserializes() {
        let fixture = include_bytes!(
            "../../scripts/app_recommendations/tests/fixtures/contract_report_v1.json"
        );
        let report = parse_report(fixture).unwrap();
        assert_eq!(report.platform, "contract-fixture");
        assert_eq!(report.candidates[0].app_id, "fixture:editor");
        assert_eq!(report.candidates[0].score, 50);
        assert_eq!(report.candidates[0].usage.covered_days, 90);
    }
}
