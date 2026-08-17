//! Pure rendering for the « Bibliothèques » cleanup section.
//!
//! Same philosophy as `applications_view`: data in, clicked actions out,
//! zero access to `EguiApp` state. The integration layer (Part 3) owns the
//! channels, the confirmation dialog, and the single-command-slot guard —
//! this module only draws and reports intents.
#![allow(dead_code)]

use eframe::egui;
use std::collections::HashMap;

use super::format::human_size;
use crate::cleanup::{ModuleResult, ModuleRow};

const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);
const WARNING_COLOR: egui::Color32 = egui::Color32::from_rgb(0xA1, 0x5C, 0x00);
const SUCCESS_COLOR: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x7A, 0x34);

/// Outcome of the last `--apply` run for one module, as retained by the
/// integration layer. `interrupted` mirrors the run report's `status`
/// (`"interrupted"`) so a run stopped midway never renders as a success,
/// whatever its per-module lists say.
#[derive(Debug, Clone)]
pub struct LastRun {
    pub result: ModuleResult,
    pub interrupted: bool,
}

pub struct CleanupViewState<'a> {
    /// `None` before the first analysis.
    pub rows: Option<&'a [ModuleRow]>,
    /// Analysis failure to show in the banner (code + stderr tail).
    pub error: Option<&'a str>,
    pub analyzing: bool,
    /// Global single-command-slot guard (computed by the integration
    /// layer): disables every button, spinner stays owned by `analyzing`.
    pub busy: bool,
    /// Last apply outcome per module name.
    pub last_runs: &'a HashMap<String, LastRun>,
    /// Rows come from a previous plan after a failed re-analysis.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupAction {
    Analyze,
    Clean(String),
}

/// The brief's decision: only safe rows get a Clean button — moderate rows
/// are greyed with no button at all (not merely disabled).
pub fn clean_allowed(row: &ModuleRow) -> bool {
    row.level == "safe"
}

/// Wording of the post-run badge. The failure count comes from the two
/// failure lists — never from `failed`, which is a byte total.
pub fn row_badge(run: &LastRun) -> String {
    let freed = freed_label(run.result.freed);
    if run.interrupted {
        format!("Nettoyage interrompu — {freed} libérés avant l'arrêt")
    } else if run.result.is_success() {
        match run.result.freed {
            Some(bytes) => format!("Nettoyé : {} libérés", human_size(bytes)),
            None => "Nettoyé (espace libéré non mesurable)".to_string(),
        }
    } else {
        format!(
            "{freed} libérés, {} en échec (fichiers verrouillés)",
            run.result.failure_count()
        )
    }
}

fn freed_label(freed: Option<u64>) -> String {
    freed.map(human_size).unwrap_or_else(|| "?".to_string())
}

/// Size shown on a row: post-run `measured` wins over the plan estimate;
/// a partially measured estimate is a floor; `None` is spelled out, never
/// rendered as a lying zero.
pub fn display_size(row: &ModuleRow, last: Option<&ModuleResult>) -> String {
    if let Some(measured) = last.and_then(|result| result.measured) {
        return human_size(measured);
    }
    match row.estimated {
        Some(bytes) if row.partially_measured => format!("≥ {} (partiel)", human_size(bytes)),
        Some(bytes) => human_size(bytes),
        None => "non mesurable".to_string(),
    }
}

pub fn render(ui: &mut egui::Ui, state: &CleanupViewState<'_>) -> Vec<CleanupAction> {
    let mut actions = Vec::new();
    let buttons_enabled = !state.busy && !state.analyzing;

    ui.horizontal(|ui| {
        ui.heading("Bibliothèques");
        if ui
            .add_enabled(buttons_enabled, egui::Button::new("Analyser"))
            .clicked()
        {
            actions.push(CleanupAction::Analyze);
        }
        if state.analyzing {
            ui.spinner();
            ui.label("analyse en cours…");
        }
    });

    if let Some(error) = state.error {
        ui.colored_label(ERROR_COLOR, format!("Échec de l'analyse : {error}"));
        if ui
            .add_enabled(buttons_enabled, egui::Button::new("Réessayer"))
            .clicked()
        {
            actions.push(CleanupAction::Analyze);
        }
    }

    let Some(rows) = state.rows else {
        if !state.analyzing && state.error.is_none() {
            ui.label("Aucune analyse effectuée — « Analyser » mesure les caches d'outils.");
        }
        return actions;
    };

    if state.stale {
        ui.colored_label(
            WARNING_COLOR,
            "Tailles issues de la dernière analyse réussie (obsolètes).",
        );
    }

    egui::Grid::new("cleanup-modules-grid")
        .striped(true)
        .min_col_width(85.0)
        .show(ui, |ui| {
            ui.strong("Bibliothèque");
            ui.strong("Taille");
            ui.strong("Contenu");
            ui.strong("Action");
            ui.end_row();
            for row in rows {
                let last = state.last_runs.get(&row.module);
                let cleanable = clean_allowed(row);
                let size = display_size(row, last.map(|run| &run.result));
                let mut content = match row.paths.first() {
                    Some(path) if row.paths.len() > 1 => {
                        format!("{path} (+{} autres)", row.paths.len() - 1)
                    }
                    Some(path) => path.clone(),
                    None => format!("{} élément(s) sans chemin", row.candidate_count),
                };
                if row.needs_network {
                    content.push_str(" · re-téléchargement requis");
                }
                if cleanable {
                    ui.label(&row.module);
                    ui.label(size);
                    ui.label(content).on_hover_text(row.paths.join("\n"));
                } else {
                    // Greyed row: analysable but not cleanable from here.
                    ui.weak(format!("{} (niveau {})", row.module, row.level));
                    ui.weak(size);
                    ui.weak(content).on_hover_text(row.paths.join("\n"));
                }
                ui.horizontal(|ui| {
                    if cleanable
                        && ui
                            .add_enabled(buttons_enabled, egui::Button::new("Nettoyer"))
                            .clicked()
                    {
                        actions.push(CleanupAction::Clean(row.module.clone()));
                    }
                    if let Some(run) = last {
                        let color = if run.interrupted || !run.result.is_success() {
                            WARNING_COLOR
                        } else {
                            SUCCESS_COLOR
                        };
                        ui.colored_label(color, row_badge(run));
                    }
                });
                ui.end_row();
            }
        });

    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    fn module_row(module: &str, level: &str, estimated: Option<u64>) -> ModuleRow {
        ModuleRow {
            module: module.to_string(),
            level: level.to_string(),
            estimated,
            partially_measured: false,
            candidate_count: 1,
            paths: vec![format!("C:\\{module}")],
            needs_network: false,
        }
    }

    fn module_result(
        freed: Option<u64>,
        measured: Option<u64>,
        locked: usize,
        failures: usize,
    ) -> ModuleResult {
        ModuleResult {
            module: "npm-cache".to_string(),
            estimated: Some(1000),
            freed,
            failed: None,
            measured,
            locked_paths: (0..locked)
                .map(|index| format!("C:\\locked-{index}"))
                .collect(),
            operation_failures: (0..failures)
                .map(|_| serde_json::json!({"resource_id": "r", "code": "x", "reason": "y"}))
                .collect(),
        }
    }

    #[test]
    fn success_badge_reports_freed_bytes() {
        let run = LastRun {
            result: module_result(Some(2048), Some(0), 0, 0),
            interrupted: false,
        };
        assert_eq!(row_badge(&run), "Nettoyé : 2.0 Kio libérés");
    }

    #[test]
    fn success_with_unmeasurable_freed_space_stays_honest() {
        let run = LastRun {
            result: module_result(None, None, 0, 0),
            interrupted: false,
        };
        assert_eq!(row_badge(&run), "Nettoyé (espace libéré non mesurable)");
    }

    #[test]
    fn partial_failure_counts_come_from_the_lists_not_from_failed_bytes() {
        let mut result = module_result(Some(1024), None, 2, 1);
        result.failed = Some(999_999_999);
        let run = LastRun {
            result,
            interrupted: false,
        };
        assert_eq!(
            row_badge(&run),
            "1.0 Kio libérés, 3 en échec (fichiers verrouillés)"
        );
    }

    #[test]
    fn interrupted_run_never_reads_as_a_success_even_with_empty_failure_lists() {
        let run = LastRun {
            result: module_result(Some(1024), None, 0, 0),
            interrupted: true,
        };
        assert!(run.result.is_success(), "precondition: lists are empty");
        let badge = row_badge(&run);
        assert!(badge.contains("interrompu"), "unexpected badge: {badge}");
        assert!(!badge.starts_with("Nettoyé"), "unexpected badge: {badge}");
    }

    #[test]
    fn display_size_prefers_measured_then_estimate_with_partial_floor() {
        let mut row = module_row("uv-cache", "safe", Some(2048));
        assert_eq!(display_size(&row, None), "2.0 Kio");
        row.partially_measured = true;
        assert_eq!(display_size(&row, None), "≥ 2.0 Kio (partiel)");
        row.estimated = None;
        assert_eq!(display_size(&row, None), "non mesurable");
        let after_run = module_result(Some(1500), Some(512), 0, 0);
        assert_eq!(display_size(&row, Some(&after_run)), "512 o");
    }

    #[test]
    fn only_safe_rows_get_a_clean_button_and_emit_clean_actions() {
        struct State {
            rows: Vec<ModuleRow>,
            last_runs: HashMap<String, LastRun>,
            actions: Vec<CleanupAction>,
        }
        let state = State {
            rows: vec![
                module_row("npm-cache", "safe", Some(1024)),
                module_row("browser-cache", "moderate", Some(2048)),
            ],
            last_runs: HashMap::new(),
            actions: Vec::new(),
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    let view_state = CleanupViewState {
                        rows: Some(&state.rows),
                        error: None,
                        analyzing: false,
                        busy: false,
                        last_runs: &state.last_runs,
                        stale: false,
                    };
                    let emitted = render(ui, &view_state);
                    state.actions.extend(emitted);
                },
                state,
            );
        harness.run();
        assert_eq!(
            harness.query_all_by_label("Nettoyer").count(),
            1,
            "the moderate row must not construct a Clean button"
        );
        harness.get_by_label("Nettoyer").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![CleanupAction::Clean("npm-cache".to_string())]
        );
    }

    #[test]
    fn busy_state_disables_analyze_so_no_action_is_emitted() {
        struct State {
            actions: Vec<CleanupAction>,
        }
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    let view_state = CleanupViewState {
                        rows: None,
                        error: None,
                        analyzing: false,
                        busy: true,
                        last_runs: &HashMap::new(),
                        stale: false,
                    };
                    state.actions.extend(render(ui, &view_state));
                },
                State {
                    actions: Vec::new(),
                },
            );
        harness.run();
        harness.get_by_label("Analyser").click();
        harness.run();
        assert!(harness.state().actions.is_empty());
    }
}
