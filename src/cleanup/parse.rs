//! Stdout parser for `clean.py --json`.
//!
//! In apply mode the script's `_JsonWriter` prints every payload it builds,
//! so stdout may hold SEVERAL concatenated top-level JSON documents (the
//! plan first, then the merged plan+`run`). The parser therefore splits the
//! stream into complete documents with a brace-depth scan — never line
//! heuristics — and deserializes the **last** complete one.

#![allow(dead_code)]

use super::model::{CleanupPlan, RunPayload};
use serde::Deserialize;

/// What a `clean.py --json` invocation produced.
#[derive(Debug)]
pub enum Payload {
    /// Analyse only (`--json` without `--apply`).
    Plan(CleanupPlan),
    /// Apply run: the plan plus its `"run"` report.
    Applied { plan: CleanupPlan, run: RunPayload },
}

pub fn parse_output(stdout: &str) -> Result<Payload, String> {
    let document = last_document(stdout)
        .ok_or_else(|| "aucun document JSON complet sur la sortie du script".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(document).map_err(|error| format!("sortie JSON invalide: {error}"))?;
    let run = match value.get("run") {
        Some(run_value) => Some(
            RunPayload::deserialize(run_value)
                .map_err(|error| format!("rapport d'exécution invalide: {error}"))?,
        ),
        None => None,
    };
    let plan = CleanupPlan::deserialize(&value)
        .map_err(|error| format!("plan de nettoyage invalide: {error}"))?;
    Ok(match run {
        Some(run) => Payload::Applied { plan, run },
        None => Payload::Plan(plan),
    })
}

/// The last complete (`{` balanced back to depth 0) top-level document.
///
/// The scan is string-aware: braces inside JSON strings, including escaped
/// quotes, never move the depth. An unterminated trailing document (e.g. an
/// interrupted run killed mid-print) is simply ignored, leaving the previous
/// complete document as the result.
fn last_document(stdout: &str) -> Option<&str> {
    let mut last = None;
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in stdout.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' if depth > 0 => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(from) = start.take() {
                        last = Some(&stdout[from..=index]);
                    }
                }
            }
            _ => {}
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = r#"{
        "level": "moderate",
        "apply": false,
        "candidates": [
            {"module": "pip-cache", "path": "C:\\Users\\x\\pip", "label": "cache pip",
             "estimated_bytes": 1000, "level": "safe", "needs_network": true}
        ],
        "total_estimated_bytes": 1000,
        "unpriced_modules": [],
        "warnings": []
    }"#;

    #[test]
    fn single_plan_document_parses_as_plan() {
        match parse_output(PLAN).unwrap() {
            Payload::Plan(plan) => {
                assert_eq!(plan.level, "moderate");
                assert_eq!(plan.candidates.len(), 1);
                assert_eq!(plan.candidates[0].estimated_bytes, Some(1000));
            }
            Payload::Applied { .. } => panic!("no run expected"),
        }
    }

    #[test]
    fn concatenated_documents_yield_the_last_one_with_its_run() {
        let applied = format!(
            "{PLAN}\n\n{}",
            r#"{"level": "safe", "apply": true, "candidates": [],
                "run": {"status": "completed",
                        "results": [{"module": "pip-cache", "freed": 900,
                                     "measured": 100, "locked_paths": [],
                                     "operation_failures": []}]}}"#
        );
        let stdout = format!("{PLAN}\n\n{applied}");
        match parse_output(&stdout).unwrap() {
            Payload::Applied { plan, run } => {
                assert!(plan.apply);
                assert_eq!(run.status, "completed");
                assert!(run.results[0].is_success());
                assert_eq!(run.results[0].freed, Some(900));
            }
            Payload::Plan(_) => panic!("run expected"),
        }
    }

    #[test]
    fn unterminated_trailing_document_falls_back_to_previous_complete_one() {
        let stdout = format!("{PLAN}\n\n{{\"level\": \"safe\", \"candidates\": [");
        match parse_output(&stdout).unwrap() {
            Payload::Plan(plan) => assert_eq!(plan.level, "moderate"),
            Payload::Applied { .. } => panic!("no run expected"),
        }
    }

    #[test]
    fn braces_inside_strings_do_not_split_documents() {
        let stdout = r#"{"level": "safe", "apply": false,
            "candidates": [{"module": "m", "label": "brace } in { label",
                            "path": "C:\\weird\"path"}],
            "warnings": []}"#;
        match parse_output(stdout).unwrap() {
            Payload::Plan(plan) => assert_eq!(plan.candidates[0].module, "m"),
            Payload::Applied { .. } => panic!("no run expected"),
        }
    }

    #[test]
    fn garbage_and_empty_input_are_rejected_with_a_message() {
        assert!(parse_output("").unwrap_err().contains("aucun document"));
        assert!(parse_output("not json at all")
            .unwrap_err()
            .contains("aucun document"));
        assert!(parse_output("{\"candidates\": 42}")
            .unwrap_err()
            .contains("invalide"));
    }

    #[test]
    fn interrupted_run_is_surfaced_by_status() {
        let stdout = r#"{"level": "safe", "apply": true, "candidates": [],
            "run": {"status": "interrupted", "results": []}}"#;
        match parse_output(stdout).unwrap() {
            Payload::Applied { run, .. } => assert!(run.is_interrupted()),
            Payload::Plan(_) => panic!("run expected"),
        }
    }
}
