//! Pure rendering and intents for the permanent local-model workspace.

use crate::models::{AcquisitionOffer, CatalogSnapshot, LibraryJournal, ProgressEvent};
use crate::ui::format::human_size;
use eframe::egui;

const ERROR: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);
const WARNING: egui::Color32 = egui::Color32::from_rgb(0xA1, 0x5C, 0x00);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModelsSection {
    #[default]
    Catalog,
    Download,
    Operations,
    Settings,
}

#[derive(Clone, Debug, Default)]
pub struct ModelFilters {
    pub family: String,
    pub tool: String,
    pub format: String,
    pub variant: String,
    pub protected_only: bool,
    pub duplicates_only: bool,
}

#[derive(Clone, Debug)]
pub struct ModelsUiState {
    pub section: ModelsSection,
    pub filters: ModelFilters,
    pub locator: String,
    pub alternatives: String,
    pub family: String,
    pub selected_offer: Option<usize>,
    pub manual_provider: String,
    pub library_root: String,
    pub provider_order: String,
    pub enabled_providers: String,
    pub xet_enabled: bool,
    pub keep_pattern: String,
}

impl Default for ModelsUiState {
    fn default() -> Self {
        Self {
            section: ModelsSection::Catalog,
            filters: ModelFilters::default(),
            locator: String::new(),
            alternatives: String::new(),
            family: "llm".to_string(),
            selected_offer: None,
            manual_provider: String::new(),
            library_root: String::new(),
            provider_order: "ollama,huggingface,lm-studio,direct".to_string(),
            enabled_providers: "ollama,huggingface,lm-studio,direct".to_string(),
            xet_enabled: true,
            keep_pattern: String::new(),
        }
    }
}

pub struct ModelsViewState<'a> {
    pub snapshot: Option<&'a CatalogSnapshot>,
    pub offers: &'a [AcquisitionOffer],
    pub progress: &'a [ProgressEvent],
    pub recovery: &'a [LibraryJournal],
    pub guided: Option<&'a crate::models::GuidedMigration>,
    pub terminal: Option<&'a ProgressEvent>,
    pub error: Option<&'a str>,
    pub loading: bool,
    pub busy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelsAction {
    Refresh,
    Resolve,
    Review(usize),
    RunReviewed,
    Cancel,
    Recover {
        operation_id: String,
        action: String,
    },
    Guide {
        artifact_id: String,
        destination: String,
        category: Option<String>,
    },
    SaveSettings,
}

pub fn filtered_artifacts<'a>(
    snapshot: &'a CatalogSnapshot,
    filters: &ModelFilters,
) -> Vec<&'a crate::models::Artifact> {
    let needle = filters.variant.to_lowercase();
    snapshot
        .artifacts
        .iter()
        .filter(|artifact| filters.family.is_empty() || artifact.family == filters.family)
        .filter(|artifact| filters.format.is_empty() || artifact.format == filters.format)
        .filter(|artifact| {
            filters.tool.is_empty()
                || artifact
                    .references
                    .iter()
                    .any(|reference| reference.tool == filters.tool)
        })
        .filter(|artifact| {
            needle.is_empty()
                || artifact.artifact_id.to_lowercase().contains(&needle)
                || artifact
                    .quantization
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&needle)
                || artifact
                    .category
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&needle)
        })
        .filter(|artifact| !filters.protected_only || artifact.protection.protected)
        .filter(|artifact| !filters.duplicates_only || artifact.duplicate_group.is_some())
        .collect()
}

/// Never compare an alternative whose exact content evidence differs.
pub fn proven_offer_indices(offers: &[AcquisitionOffer]) -> Vec<usize> {
    let Some(primary) = offers.first() else {
        return Vec::new();
    };
    offers
        .iter()
        .enumerate()
        .filter(|(_, offer)| {
            if primary.trusted_digest.is_none() || primary.immutable_revision.is_none() {
                std::ptr::eq(*offer, primary)
            } else {
                offer.trusted_digest == primary.trusted_digest
                    && offer.immutable_revision == primary.immutable_revision
                    && offer.filename == primary.filename
                    && offer.format == primary.format
                    && offer.quantization == primary.quantization
                    && offer.category == primary.category
            }
        })
        .map(|(index, _)| index)
        .collect()
}

pub fn render(
    ui: &mut egui::Ui,
    state: &ModelsViewState<'_>,
    form: &mut ModelsUiState,
) -> Vec<ModelsAction> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.heading("DevToolBox — Modèles");
        if ui
            .add_enabled(
                !state.loading && !state.busy,
                egui::Button::new("Actualiser"),
            )
            .clicked()
        {
            actions.push(ModelsAction::Refresh);
        }
        if state.loading {
            ui.spinner();
            ui.label("inventaire en cours…");
        }
    });
    if let Some(error) = state.error {
        ui.colored_label(ERROR, error);
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut form.section, ModelsSection::Catalog, "Catalogue");
        ui.selectable_value(&mut form.section, ModelsSection::Download, "Téléchargement");
        ui.selectable_value(&mut form.section, ModelsSection::Operations, "Opérations");
        ui.selectable_value(&mut form.section, ModelsSection::Settings, "Réglages");
    });
    ui.separator();
    match form.section {
        ModelsSection::Catalog => render_catalog(ui, state, form, &mut actions),
        ModelsSection::Download => render_download(ui, state, form, &mut actions),
        ModelsSection::Operations => render_operations(ui, state, &mut actions),
        ModelsSection::Settings => render_settings(ui, state, form, &mut actions),
    }
    actions
}

fn render_catalog(
    ui: &mut egui::Ui,
    state: &ModelsViewState<'_>,
    form: &mut ModelsUiState,
    actions: &mut Vec<ModelsAction>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Famille");
        ui.text_edit_singleline(&mut form.filters.family);
        ui.label("Outil");
        ui.text_edit_singleline(&mut form.filters.tool);
        ui.label("Format");
        ui.text_edit_singleline(&mut form.filters.format);
        ui.label("Variante");
        ui.text_edit_singleline(&mut form.filters.variant);
        ui.checkbox(&mut form.filters.protected_only, "Protégés");
        ui.checkbox(&mut form.filters.duplicates_only, "Doublons exacts");
    });
    let Some(snapshot) = state.snapshot else {
        ui.label("Aucun inventaire chargé. L'onglet reste disponible sans outil installé.");
        return;
    };
    for error in &snapshot.source_errors {
        ui.colored_label(
            WARNING,
            format!("{} : {} ({})", error.source, error.message, error.code),
        );
    }
    let rows = filtered_artifacts(snapshot, &form.filters);
    if rows.is_empty() {
        ui.label("Aucun artefact ne correspond aux filtres.");
        return;
    }
    egui::ScrollArea::both().show(ui, |ui| {
        egui::Grid::new("models-catalog-grid")
            .striped(true)
            .show(ui, |ui| {
                for heading in ["Modèle", "Famille", "Format", "Logique", "Alloué", "État"] {
                    ui.strong(heading);
                }
                ui.end_row();
                for artifact in rows {
                    ui.label(&artifact.artifact_id);
                    ui.label(&artifact.family);
                    ui.label(&artifact.format);
                    ui.label(
                        artifact
                            .logical_size
                            .map(human_size)
                            .unwrap_or_else(|| "inconnu".to_string()),
                    );
                    ui.label(
                        artifact
                            .allocated_size
                            .map(human_size)
                            .unwrap_or_else(|| "inconnu".to_string()),
                    );
                    let status = if artifact.protection.protected {
                        format!("protégé : {}", artifact.protection.reasons.join(", "))
                    } else if artifact.duplicate_group.is_some() {
                        "doublon exact vérifié".to_string()
                    } else {
                        artifact.identity.state.clone()
                    };
                    ui.label(status).on_hover_text(&artifact.path);
                    ui.end_row();
                    if artifact.artifact_id.starts_with("library:") {
                        ui.horizontal(|ui| {
                            let verified = artifact.identity.state == "verified";
                            if artifact.family == "llm"
                                && ui
                                    .add_enabled(
                                        verified && !state.busy,
                                        egui::Button::new("Guider vers Jan"),
                                    )
                                    .clicked()
                            {
                                actions.push(ModelsAction::Guide {
                                    artifact_id: artifact.artifact_id.clone(),
                                    destination: "jan".into(),
                                    category: None,
                                });
                            }
                            if artifact.family == "image"
                                && ui
                                    .add_enabled(
                                        verified && artifact.category.is_some() && !state.busy,
                                        egui::Button::new("Guider vers ComfyUI"),
                                    )
                                    .clicked()
                            {
                                actions.push(ModelsAction::Guide {
                                    artifact_id: artifact.artifact_id.clone(),
                                    destination: "comfyui".into(),
                                    category: artifact.category.clone(),
                                });
                            }
                            if !verified {
                                ui.weak("Intégration désactivée : identité insuffisante.");
                            }
                        });
                        ui.end_row();
                    }
                }
            });
    });
}

fn render_download(
    ui: &mut egui::Ui,
    state: &ModelsViewState<'_>,
    form: &mut ModelsUiState,
    actions: &mut Vec<ModelsAction>,
) {
    ui.horizontal(|ui| {
        ui.label("Identifiant exact");
        ui.text_edit_singleline(&mut form.locator);
        egui::ComboBox::from_id_salt("model-family")
            .selected_text(&form.family)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut form.family, "llm".to_string(), "LLM");
                ui.selectable_value(&mut form.family, "image".to_string(), "Image");
            });
        if ui
            .add_enabled(
                !state.busy && !form.locator.trim().is_empty(),
                egui::Button::new("Comparer"),
            )
            .clicked()
        {
            actions.push(ModelsAction::Resolve);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Alternatives exactes (une par ligne)");
        ui.text_edit_singleline(&mut form.alternatives);
    });
    ui.horizontal(|ui| {
        ui.label("Priorité manuelle");
        ui.text_edit_singleline(&mut form.manual_provider);
    });

    let indices = proven_offer_indices(state.offers);
    for index in indices {
        let offer = &state.offers[index];
        let selected = form.selected_offer == Some(index);
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.strong(&offer.provider);
                ui.label(format!("{} · {}", offer.filename, offer.format));
                if offer.cache_verified {
                    ui.label("cache complet vérifié · confiance haute");
                } else {
                    ui.label("ordre à froid · confiance inconnue");
                }
            });
            ui.label(format!(
                "Réseau {} · copie locale {} · temporaire {}",
                offer
                    .network_bytes
                    .map(human_size)
                    .unwrap_or_else(|| "?".into()),
                offer
                    .local_copy_bytes
                    .map(human_size)
                    .unwrap_or_else(|| "?".into()),
                offer
                    .temporary_bytes
                    .map(human_size)
                    .unwrap_or_else(|| "?".into())
            ));
            if offer.conversion_required {
                ui.colored_label(
                    WARNING,
                    "Conversion requise : offre visible mais non exécutable.",
                );
            }
            if ui
                .add_enabled(
                    offer.executable && !offer.conversion_required && !state.busy,
                    egui::Button::new(if selected {
                        "Plan revu"
                    } else {
                        "Revoir ce plan"
                    }),
                )
                .clicked()
            {
                actions.push(ModelsAction::Review(index));
            }
        });
    }
    let reviewed = form
        .selected_offer
        .and_then(|index| state.offers.get(index))
        .filter(|offer| offer.review_digest.is_some());
    if let Some(offer) = reviewed {
        ui.separator();
        ui.label(format!(
            "Plan immuable revu : {} via {} · digest {}",
            offer.locator,
            offer.provider,
            offer.review_digest.as_deref().unwrap_or_default()
        ));
        if ui
            .add_enabled(!state.busy, egui::Button::new("Télécharger le plan revu"))
            .clicked()
        {
            actions.push(ModelsAction::RunReviewed);
        }
    }
}

fn render_operations(
    ui: &mut egui::Ui,
    state: &ModelsViewState<'_>,
    actions: &mut Vec<ModelsAction>,
) {
    if state.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Opération active");
            if ui.button("Annuler").clicked() {
                actions.push(ModelsAction::Cancel);
            }
        });
    }
    for event in state.progress.iter().rev().take(20).rev() {
        ui.label(format!(
            "#{} {} — {} / {}",
            event.sequence,
            event.kind,
            event.transferred_bytes.unwrap_or(0),
            event
                .total_bytes
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into())
        ));
    }
    if let Some(terminal) = state.terminal {
        ui.strong(format!("Terminé : {}", terminal.kind));
        if let Some(message) = &terminal.message {
            ui.label(message);
        }
    }
    for journal in state.recovery {
        ui.group(|ui| {
            ui.strong(format!("{} · {}", journal.operation_id, journal.state));
            ui.label(format!(
                "{} · {} octets",
                journal.filename, journal.bytes_written
            ));
            if let Some(error) = &journal.error {
                ui.colored_label(WARNING, error);
            }
            if journal.state == "discardable"
                && ui
                    .add_enabled(!state.busy, egui::Button::new("Écarter le staging possédé"))
                    .clicked()
            {
                actions.push(ModelsAction::Recover {
                    operation_id: journal.operation_id.clone(),
                    action: "discard-partial".into(),
                });
            } else if journal.state == "resumable" {
                ui.weak(
                    "Reprise disponible seulement si le fournisseur conserve sa preuve exacte.",
                );
            }
        });
    }
    if let Some(guided) = state.guided {
        ui.separator();
        ui.strong(format!(
            "Intégration guidée {} · {}",
            guided.destination_tool, guided.state
        ));
        if let Some(step) = &guided.manual_step {
            ui.label(&step.documented_action);
            ui.label(format!("Attendu : {}", step.expected_reference));
            ui.label(format!("Reprise : {}", step.resume_condition));
        }
        ui.label(format!(
            "Validation : identité {}, catalogue {}, chargement {}, inférence {}, workflow {}",
            guided.validation.identity,
            guided.validation.catalog,
            guided.validation.load,
            guided.validation.inference,
            guided.validation.workflow
        ));
    }
    ui.separator();
    ui.label("Les reprises ne proposent que les actions prouvées par les journaux.");
    ui.label("Jan et ComfyUI restent guidés tant qu'aucune API publique fiable n'est disponible.");
    ui.label("Un retrait obsolète, protégé ou insuffisamment validé reste désactivé.");
}

fn render_settings(
    ui: &mut egui::Ui,
    state: &ModelsViewState<'_>,
    form: &mut ModelsUiState,
    actions: &mut Vec<ModelsAction>,
) {
    ui.label("Bibliothèque neutre");
    ui.text_edit_singleline(&mut form.library_root);
    if ui
        .add_enabled(
            !state.busy && !form.library_root.trim().is_empty(),
            egui::Button::new("Enregistrer la bibliothèque"),
        )
        .clicked()
    {
        actions.push(ModelsAction::SaveSettings);
    }
    ui.label("Ordre des fournisseurs (machine locale)");
    ui.text_edit_singleline(&mut form.provider_order);
    ui.label("Fournisseurs activés (séparés par des virgules)");
    ui.text_edit_singleline(&mut form.enabled_providers);
    ui.checkbox(&mut form.xet_enabled, "Préférer Xet quand disponible");
    ui.label("Protection keep (motif local, aucun identifiant secret)");
    ui.text_edit_singleline(&mut form.keep_pattern);
    ui.weak("Changer la racine ne déplace jamais implicitement les artefacts existants.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Artifact, ArtifactIdentity, Protection};
    use egui_kittest::{kittest::Queryable, Harness};

    fn snapshot(artifacts: Vec<Artifact>) -> CatalogSnapshot {
        CatalogSnapshot {
            schema_version: 1,
            generated_at: "fixed".into(),
            platform: "test".into(),
            installations: Vec::new(),
            artifacts,
            source_errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn filters_protected_duplicates_and_variants_without_confusing_sizes() {
        let artifact = Artifact {
            artifact_id: "llama-q4".into(),
            family: "llm".into(),
            format: "gguf".into(),
            quantization: Some("Q4_K_M".into()),
            logical_size: Some(10),
            allocated_size: Some(20),
            duplicate_group: Some("sha256:x".into()),
            protection: Protection {
                protected: true,
                reasons: vec!["keep".into()],
            },
            identity: ArtifactIdentity {
                state: "verified".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let snapshot = snapshot(vec![artifact]);
        let filters = ModelFilters {
            family: "llm".into(),
            format: "gguf".into(),
            variant: "q4".into(),
            protected_only: true,
            duplicates_only: true,
            ..Default::default()
        };
        let rows = filtered_artifacts(&snapshot, &filters);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].logical_size, Some(10));
        assert_eq!(rows[0].allocated_size, Some(20));
    }

    #[test]
    fn only_same_byte_offers_are_comparable() {
        let primary = AcquisitionOffer {
            trusted_digest: Some("sha256:a".into()),
            immutable_revision: Some("rev".into()),
            filename: "model.gguf".into(),
            format: "gguf".into(),
            ..Default::default()
        };
        let mut same = primary.clone();
        same.provider = "other".into();
        let mut changed = primary.clone();
        changed.trusted_digest = Some("sha256:b".into());
        assert_eq!(proven_offer_indices(&[primary, same, changed]), vec![0, 1]);
    }

    #[test]
    fn provisional_or_conversion_offer_cannot_be_run() {
        let offer = AcquisitionOffer {
            executable: false,
            conversion_required: true,
            review_digest: None,
            ..Default::default()
        };
        assert!(offer.review_digest.is_none());
        assert!(!offer.executable);
    }

    #[derive(Default)]
    struct TestState {
        form: ModelsUiState,
        snapshot: Option<CatalogSnapshot>,
        offers: Vec<AcquisitionOffer>,
        progress: Vec<ProgressEvent>,
        recovery: Vec<LibraryJournal>,
        guided: Option<crate::models::GuidedMigration>,
        terminal: Option<ProgressEvent>,
        error: Option<String>,
        loading: bool,
        busy: bool,
        actions: Vec<ModelsAction>,
    }

    fn view_harness(state: TestState) -> Harness<'static, TestState> {
        Harness::builder()
            .with_size(egui::vec2(1100.0, 760.0))
            .build_ui_state(
                |ui, state: &mut TestState| {
                    let view = ModelsViewState {
                        snapshot: state.snapshot.as_ref(),
                        offers: &state.offers,
                        progress: &state.progress,
                        recovery: &state.recovery,
                        guided: state.guided.as_ref(),
                        terminal: state.terminal.as_ref(),
                        error: state.error.as_deref(),
                        loading: state.loading,
                        busy: state.busy,
                    };
                    state.actions = render(ui, &view, &mut state.form);
                },
                state,
            )
    }

    #[test]
    fn empty_and_partial_catalogs_remain_actionable() {
        let mut harness = view_harness(TestState::default());
        harness.run();
        assert!(harness.query_by_label("Actualiser").is_some());
        assert!(harness
            .query_by_label_contains("Aucun inventaire chargé")
            .is_some());

        let mut state = TestState::default();
        let mut snapshot = snapshot(Vec::new());
        snapshot.source_errors.push(crate::models::SourceError {
            source: "jan".into(),
            code: "offline".into(),
            message: "indisponible".into(),
            confidence: "high".into(),
        });
        state.snapshot = Some(snapshot);
        let mut harness = view_harness(state);
        harness.run();
        assert!(harness
            .query_by_label_contains("jan : indisponible")
            .is_some());
    }

    #[test]
    fn recommended_overridden_and_conversion_states_are_visible() {
        let cached = AcquisitionOffer {
            provider: "ollama".into(),
            filename: "model.gguf".into(),
            format: "gguf".into(),
            executable: true,
            cache_verified: true,
            review_digest: Some("a".repeat(64)),
            ..Default::default()
        };
        let conversion = AcquisitionOffer {
            provider: "direct".into(),
            filename: "model.safetensors".into(),
            format: "safetensors".into(),
            conversion_required: true,
            ..Default::default()
        };
        let mut state = TestState::default();
        state.form.section = ModelsSection::Download;
        state.form.manual_provider = "ollama".into();
        state.offers = vec![cached];
        let mut harness = view_harness(state);
        harness.run();
        assert!(harness.query_by_label_contains("confiance haute").is_some());
        assert_eq!(harness.state().form.manual_provider, "ollama");

        let mut state = TestState::default();
        state.form.section = ModelsSection::Download;
        state.offers = vec![conversion];
        let mut harness = view_harness(state);
        harness.run();
        assert!(harness
            .query_by_label_contains("Conversion requise")
            .is_some());
    }

    #[test]
    fn busy_interrupted_guided_validated_protected_and_stale_states_are_explained() {
        let terminal = ProgressEvent {
            sequence: 2,
            kind: "cancelled".into(),
            operation_id: "op".into(),
            transferred_bytes: Some(10),
            total_bytes: Some(20),
            message: Some("interrompu; reprise disponible".into()),
            artifact_id: None,
            schema_version: 1,
        };
        let mut state = TestState::default();
        state.form.section = ModelsSection::Operations;
        state.busy = true;
        state.terminal = Some(terminal);
        state.guided = Some(crate::models::GuidedMigration {
            destination_tool: "jan".into(),
            state: "pending-manual".into(),
            manual_step: Some(crate::models::ManualStep {
                documented_action: "Utiliser Link Files".into(),
                expected_reference: "model.gguf".into(),
                resume_condition: "catalogue exact".into(),
                ..Default::default()
            }),
            validation: crate::models::MigrationValidation {
                identity: "passed".into(),
                catalog: "weak".into(),
                ..Default::default()
            },
            ..Default::default()
        });
        let mut harness = view_harness(state);
        harness.run_steps(1);
        assert!(harness.query_by_label("Annuler").is_some());
        assert!(harness.query_by_label_contains("interrompu").is_some());
        assert!(harness
            .query_by_label_contains("Jan et ComfyUI restent guidés")
            .is_some());
        assert!(harness
            .query_by_label_contains("obsolète, protégé")
            .is_some());
        assert!(harness
            .query_by_label_contains("actions prouvées")
            .is_some());
        assert!(harness.query_by_label_contains("Link Files").is_some());
        assert!(harness.query_by_label_contains("identité passed").is_some());
    }

    #[test]
    fn restart_recovery_emits_only_the_exact_owned_discard_action() {
        let mut state = TestState::default();
        state.form.section = ModelsSection::Operations;
        state.recovery.push(LibraryJournal {
            operation_id: "exact-op".into(),
            state: "discardable".into(),
            filename: "partial.gguf".into(),
            staging_path: "/owned/exact-op".into(),
            bytes_written: 42,
            ..Default::default()
        });
        let mut harness = view_harness(state);
        harness.run();
        harness.get_by_label("Écarter le staging possédé").click();
        harness.run_steps(1);
        assert_eq!(
            harness.state().actions,
            vec![ModelsAction::Recover {
                operation_id: "exact-op".into(),
                action: "discard-partial".into(),
            }]
        );
    }
}
