//! Native read-only rendering for application removal recommendations.

use eframe::egui;

use crate::applications::{ApplicationCandidate, RecommendationReport};

#[derive(Default)]
pub struct ApplicationFilters {
    pub search: String,
    pub source: String,
    pub min_size_mib: u64,
    pub min_covered_days: u32,
    pub show_protected: bool,
}

pub fn filtered_candidates<'a>(
    report: &'a RecommendationReport,
    filters: &ApplicationFilters,
) -> Vec<&'a ApplicationCandidate> {
    let search = filters.search.trim().to_lowercase();
    report
        .candidates
        .iter()
        .filter(|candidate| filters.show_protected || !candidate.protection.protected)
        .filter(|candidate| filters.source.is_empty() || candidate.source == filters.source)
        .filter(|candidate| {
            search.is_empty()
                || candidate.name.to_lowercase().contains(&search)
                || candidate.app_id.to_lowercase().contains(&search)
        })
        .filter(|candidate| {
            candidate.size.installed_bytes.unwrap_or(0) >= filters.min_size_mib * 1024 * 1024
        })
        .filter(|candidate| candidate.usage.covered_days >= filters.min_covered_days)
        .collect()
}

fn human_size(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => super::format::human_size(bytes),
        None => "inconnue".to_string(),
    }
}

fn usage_label(candidate: &ApplicationCandidate) -> String {
    match candidate.usage.kind.as_str() {
        "known_last_seen" => candidate
            .usage
            .last_seen
            .as_deref()
            .map(|timestamp| format!("vu le {timestamp}"))
            .unwrap_or_else(|| "dernier usage incomplet".to_string()),
        "not_observed" => format!(
            "non observé pendant {} jours couverts",
            candidate.usage.covered_days
        ),
        _ => "usage inconnu".to_string(),
    }
}

pub fn render(
    ui: &mut egui::Ui,
    report: Option<&RecommendationReport>,
    error: Option<&str>,
    loading: bool,
    filters: &mut ApplicationFilters,
    selected: &mut Option<String>,
) -> bool {
    let mut refresh = false;
    ui.horizontal(|ui| {
        ui.heading("DevToolBox — Applications");
        if ui
            .add_enabled(!loading, egui::Button::new("Rafraîchir"))
            .clicked()
        {
            refresh = true;
        }
        if loading {
            ui.spinner();
            ui.label("relevé en cours…");
        }
    });

    if let Some(error) = error {
        ui.colored_label(egui::Color32::from_rgb(0xC4, 0x2B, 0x1C), error);
    }
    let Some(report) = report else {
        ui.label(if loading {
            "Chargement du premier relevé…"
        } else {
            "Aucun rapport disponible."
        });
        return refresh;
    };

    let known_bytes: u64 = report
        .candidates
        .iter()
        .filter_map(|candidate| candidate.size.installed_bytes)
        .sum();
    let reclaimable_bytes: u64 = report
        .candidates
        .iter()
        .filter_map(|candidate| candidate.size.reclaimable_bytes)
        .sum();
    let reclaimable_label = if report
        .candidates
        .iter()
        .any(|candidate| candidate.size.reclaimable_bytes.is_some())
    {
        human_size(Some(reclaimable_bytes))
    } else {
        "inconnu".to_string()
    };
    let usage_signals = report
        .candidates
        .iter()
        .filter(|candidate| candidate.usage.kind != "unknown")
        .count();
    ui.label(format!(
        "Empreinte connue : {} · gain récupérable estimé : {} · {} candidats · {} signaux d’usage · relevé {}",
        human_size(Some(known_bytes)),
        reclaimable_label,
        report
            .candidates
            .iter()
            .filter(|candidate| !candidate.protection.protected)
            .count(),
        usage_signals,
        report.generated_at
    ));
    if !report.source_errors.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(0xA1, 0x5C, 0x00),
            format!(
                "Rapport partiel : {} source(s) indisponible(s)",
                report.source_errors.len()
            ),
        );
        for source_error in &report.source_errors {
            ui.small(format!(
                "{} [{}] : {}",
                source_error.source, source_error.code, source_error.message
            ));
        }
    }
    for warning in &report.warnings {
        ui.small(format!("Avertissement : {warning}"));
    }
    ui.separator();

    let mut sources: Vec<&str> = report
        .candidates
        .iter()
        .map(|candidate| candidate.source.as_str())
        .collect();
    sources.sort_unstable();
    sources.dedup();
    ui.horizontal_wrapped(|ui| {
        let search_label = ui.label("Recherche");
        ui.text_edit_singleline(&mut filters.search)
            .labelled_by(search_label.id);
        egui::ComboBox::from_label("Source")
            .selected_text(if filters.source.is_empty() {
                "Toutes"
            } else {
                &filters.source
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filters.source, String::new(), "Toutes");
                for source in &sources {
                    ui.selectable_value(&mut filters.source, (*source).to_string(), *source);
                }
            });
        ui.label("Taille mini (Mio)");
        ui.add(egui::DragValue::new(&mut filters.min_size_mib).speed(100));
        ui.label("Ancienneté couverte (jours)");
        ui.add(egui::DragValue::new(&mut filters.min_covered_days).speed(10));
        ui.checkbox(
            &mut filters.show_protected,
            "Afficher les éléments protégés",
        );
    });
    ui.separator();

    let candidates = filtered_candidates(report, filters);
    egui::ScrollArea::vertical()
        .max_height(330.0)
        .show(ui, |ui| {
            egui::Grid::new("applications-grid")
                .striped(true)
                .min_col_width(85.0)
                .show(ui, |ui| {
                    ui.strong("Priorité");
                    ui.strong("Application");
                    ui.strong("Taille");
                    ui.strong("Dernier usage");
                    ui.strong("Confiance");
                    ui.strong("Source");
                    ui.end_row();
                    for candidate in candidates {
                        ui.label(candidate.score.to_string());
                        let selected_now = selected.as_deref() == Some(candidate.app_id.as_str());
                        if ui.selectable_label(selected_now, &candidate.name).clicked() {
                            *selected = Some(candidate.app_id.clone());
                        }
                        ui.label(human_size(candidate.size.installed_bytes));
                        ui.label(usage_label(candidate));
                        ui.label(&candidate.confidence);
                        ui.label(&candidate.source);
                        ui.end_row();
                    }
                });
        });

    if let Some(candidate) = selected.as_deref().and_then(|app_id| {
        report
            .candidates
            .iter()
            .find(|candidate| candidate.app_id == app_id)
    }) {
        ui.separator();
        ui.strong(format!("{} — justification", candidate.name));
        ui.label(format!(
            "Empreinte : {} · gain récupérable estimé : {} · méthode : {} · périmètre : {} · confiance taille : {}",
            human_size(candidate.size.installed_bytes),
            human_size(candidate.size.reclaimable_bytes),
            candidate.size.method,
            candidate.size.scope,
            candidate.size.confidence
        ));
        ui.label(format!(
            "Usage : {} · confiance : {}{}",
            usage_label(candidate),
            candidate.usage.confidence,
            candidate
                .usage
                .tracked_since
                .as_deref()
                .map(|value| format!(" · suivi depuis {value}"))
                .unwrap_or_default()
        ));
        for reason in &candidate.reasons {
            ui.label(format!("• {reason}"));
        }
        for reason in &candidate.protection.reasons {
            ui.colored_label(
                egui::Color32::from_rgb(0xA1, 0x5C, 0x00),
                format!("Protégé : {reason}"),
            );
        }
        if !candidate.protection.protected {
            if let Some(command) = &candidate.command {
                let label = if command.origin == "publisher_unverified" {
                    "Copier la commande éditeur non vérifiée"
                } else {
                    "Copier la commande"
                };
                if command.origin == "publisher_unverified" {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xA1, 0x5C, 0x00),
                        "Cette commande provient de l’éditeur et doit être relue avant usage.",
                    );
                }
                ui.monospace(&command.value);
                if ui.button(label).clicked() {
                    ui.ctx().copy_text(command.value.clone());
                }
            }
        }
    }
    refresh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applications::{CommandSuggestion, Protection, SizeEvidence, UsageEvidence};
    use std::collections::HashMap;

    fn candidate(name: &str, protected: bool) -> ApplicationCandidate {
        ApplicationCandidate {
            app_id: format!("apt:{}", name.to_lowercase()),
            source: "apt".to_string(),
            name: name.to_string(),
            size: SizeEvidence {
                installed_bytes: Some(1024 * 1024 * 500),
                reclaimable_bytes: None,
                method: "dpkg".to_string(),
                scope: "package".to_string(),
                confidence: "high".to_string(),
            },
            executable_hints: vec![],
            usage: UsageEvidence {
                kind: "not_observed".to_string(),
                last_seen: None,
                tracked_since: Some("2026-01-01T00:00:00Z".to_string()),
                covered_days: 90,
                confidence: "medium".to_string(),
            },
            protection: Protection {
                protected,
                reasons: vec![],
            },
            command: Some(CommandSuggestion {
                value: "remove".to_string(),
                origin: "manager_verified".to_string(),
            }),
            score: 35,
            confidence: "medium".to_string(),
            reasons: vec![],
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn filters_combine_search_size_age_and_protection() {
        let report = RecommendationReport {
            schema_version: 1,
            generated_at: "now".to_string(),
            platform: "linux".to_string(),
            candidates: vec![candidate("Editor", false), candidate("Runtime", true)],
            source_errors: vec![],
            warnings: vec![],
        };
        let filters = ApplicationFilters {
            search: "edit".to_string(),
            source: "apt".to_string(),
            min_size_mib: 250,
            min_covered_days: 30,
            show_protected: false,
        };
        assert_eq!(filtered_candidates(&report, &filters)[0].name, "Editor");
    }

    #[test]
    fn unknown_size_is_not_promoted_by_minimum_filter() {
        assert_eq!(human_size(None), "inconnue");
    }
}
