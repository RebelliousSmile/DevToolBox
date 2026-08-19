//! Docker view shared types — the `AutomationRow` precedent
//! (`src/ui/automations_view.rs:37-51`) applied to the Docker tab (see
//! `aidd_docs/tasks/2026_08/2026_08_19-docker-tab.md`, Phase 1/2): the row
//! shapes returned to the UI are OS-neutral and declared here so every
//! build (including Windows/macOS, which have no Docker data source yet)
//! keeps compiling, while `crate::linux::docker` (Linux-only, compiles to
//! nothing elsewhere) is the only module that knows how to *produce* them
//! by mapping its private wire-format serde structs onto these types.
//!
//! Phase 2 adds the OS-neutral façade (`available()`/`fetch()`/the four
//! action wrappers, `automations_view.rs:66-160` layout), the pure
//! "data in, actions out" `render` view (`cleanup_view.rs` precedent) and
//! the `DockerAction` intent enum. Phase 3 wires all of this into
//! `EguiApp` (tab visibility, lazy fetch, confirmation modals,
//! `PendingAction` dispatch — see `src/ui/egui_app.rs`), so every item here
//! now has a production caller and the module-level `#![allow(dead_code)]`
//! that covered the pre-Phase-3 gap has been removed.

use eframe::egui;

/// One row of the Docker tab's "Conteneurs" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerEntry {
    pub id: String,
    pub name: String,
    /// The image reference as reported by `docker ps -a` (repo:tag, a short
    /// ID, or a digest ref — whatever Docker itself printed), kept verbatim
    /// for display; used-image cross-referencing happens on the Linux side
    /// before this row is built.
    pub image: String,
    pub state: ContainerState,
    /// Free-text status as reported by Docker (e.g. `"Up 3 hours"`,
    /// `"Exited (0) 22 hours ago"`), shown alongside `state` since it
    /// carries detail no enum variant captures.
    pub status: String,
    /// The container's writable-layer size (e.g. `"767kB"`), already
    /// stripped of the `(virtual …)` suffix by `crate::linux::docker`'s
    /// `extract_rw_size` — what `docker rm` actually frees. Empty when
    /// unknown (should not normally happen: `docker ps -a --size` always
    /// fills this field on this reference machine, but a future CLI shape
    /// change degrading it to blank must not crash the confirm-message
    /// formatting).
    pub rw_size: String,
}

/// A container's lifecycle state, mapped from Docker's free-text `State`
/// field (see `crate::linux::docker`'s `ContainerState::from_raw`) into a
/// closed set the view can match on to decide which actions are offered.
/// `Unknown` is the conservative default for any value this mapping wasn't
/// built to recognize — Phase 2 gates both stop and remove off for it (per
/// the plan's Phase 1 task 8: "anything unknown ⇒ no action offered").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Running,
    Paused,
    Restarting,
    Exited,
    Created,
    Dead,
    Unknown(String),
}

impl ContainerState {
    /// `true` for the states a running-container stop action is offered on
    /// (`running`/`paused`/`restarting`).
    pub fn is_stoppable(&self) -> bool {
        matches!(
            self,
            ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
        )
    }

    /// `true` for the states a container-removal action is offered on
    /// (`exited`/`created`/`dead`).
    pub fn is_removable(&self) -> bool {
        matches!(
            self,
            ContainerState::Exited | ContainerState::Created | ContainerState::Dead
        )
    }
}

/// One row of the Docker tab's "Images" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageEntry {
    pub id: String,
    /// Display identity: `"<repository>:<tag>"`, including the literal
    /// `"<none>:<none>"` Docker itself prints for an untagged image.
    pub identity: String,
    pub size: String,
    pub created: String,
    /// Conservative "in use" flag: `true` when at least one container
    /// references this image (by ID prefix or normalized repo:tag), or when
    /// any container's image reference could not be resolved at all (see
    /// the Risk register's "used-on-doubt" rule — deletion is never offered
    /// on uncertainty). Never `false` on doubt.
    pub used: bool,
    /// The exact reference `docker rmi` should receive: the row's
    /// `identity` (repo:tag) when tagged, the short `id` when untagged
    /// (`rmi <id>` refuses multi-tagged images without `--force`, which is
    /// banned; removing by tag untags cleanly, while an untagged row has no
    /// tag to remove by).
    pub rmi_reference: String,
}

impl ImageEntry {
    /// `true` when this row is Docker's `<none>:<none>` placeholder for an
    /// untagged image. Its `rmi_reference` is the image ID, and removing it
    /// is an outright, unambiguous deletion — there is no tag to strip
    /// first. A tagged row's removal is comparatively ambiguous: it always
    /// untags the row's `identity`, but whether that also deletes the
    /// underlying image data depends on whether any other tag still points
    /// to it (information this row alone doesn't carry). Phase 3's
    /// confirmation-message wording (« l'image sera détaguée » vs
    /// « l'image sera supprimée ») branches on this accessor rather than on
    /// a new `DockerAction::RemoveImage` payload field, keeping the action
    /// enum a plain identifier carrier.
    pub fn is_untagged(&self) -> bool {
        self.identity == "<none>:<none>"
    }
}

/// One row of the Docker tab's "Volumes" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEntry {
    pub name: String,
    pub driver: String,
    /// `true` when this volume is a member of Docker's own "dangling"
    /// set (`docker volume ls -f dangling=true`) — the authoritative
    /// orphan/unused signal, not a re-derivation of it.
    pub orphan: bool,
    /// This volume's on-disk size, or `None` when it hasn't been computed
    /// yet — `docker volume ls` never reports it (always `"N/A"`, confirmed
    /// on this machine); it's only known once `DockerAction::ComputeVolumeSizes`
    /// has run `crate::linux::docker::volume_sizes` (a `docker system df -v`
    /// disk scan) and `EguiApp` has merged the result into the snapshot by
    /// name.
    pub size: Option<String>,
}

/// The full Docker tab snapshot: one `fetch()` call's worth of containers,
/// images and volumes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerSnapshot {
    pub containers: Vec<ContainerEntry>,
    pub images: Vec<ImageEntry>,
    pub volumes: Vec<VolumeEntry>,
}

impl Default for ContainerState {
    /// Matches the mapping's own conservative default: an absent/unparsed
    /// state is `Unknown`, never a guessed active/inactive variant.
    fn default() -> Self {
        ContainerState::Unknown(String::new())
    }
}

// ---------------------------------------------------------------------------
// OS-neutral façade — exact cfg layout of `automations_view.rs:66-160`
// ---------------------------------------------------------------------------

/// `true` when the Docker tab has a data source on this OS at all — drives
/// tab visibility in `EguiApp` (Phase 3). Linux resolves the real `docker`
/// binary; every other OS is hardcoded `false` for now (Risk register: "tab
/// button rendered only when `docker_available`, which is hardcoded `false`
/// on non-Linux for now").
pub fn available() -> bool {
    available_impl()
}

#[cfg(target_os = "linux")]
fn available_impl() -> bool {
    crate::linux::docker::binary_available()
}

#[cfg(not(target_os = "linux"))]
fn available_impl() -> bool {
    false
}

/// Fetch the current Docker snapshot (containers/images/volumes) for this
/// OS. The error string is already a formatted, French, ready-to-display
/// message (`DockerError`'s `Display` impl on Linux) — callers (the view,
/// then `EguiApp`) never need to re-classify it; a daemon-unreachable error
/// and a plain command failure both just render as "the fetch failed, offer
/// Réessayer", which `render` below does unconditionally on any `Err`.
pub fn fetch() -> Result<DockerSnapshot, String> {
    fetch_impl()
}

#[cfg(target_os = "linux")]
fn fetch_impl() -> Result<DockerSnapshot, String> {
    crate::linux::docker::fetch().map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
fn fetch_impl() -> Result<DockerSnapshot, String> {
    Err("Docker n'est pas pris en charge sur cet OS.".to_string())
}

/// Stop a running/paused/restarting container by `id`. Plain `docker stop`
/// on Linux (`crate::linux::docker::stop_container`), no `--force`
/// equivalent, ever.
pub fn stop_container(id: &str) -> Result<(), String> {
    stop_container_impl(id)
}

#[cfg(target_os = "linux")]
fn stop_container_impl(id: &str) -> Result<(), String> {
    crate::linux::docker::stop_container(id).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
fn stop_container_impl(id: &str) -> Result<(), String> {
    let _ = id;
    Err("Docker n'est pas pris en charge sur cet OS.".to_string())
}

/// Remove a stopped/created/dead container by `id`. Plain `docker rm`, no
/// `-f`.
pub fn remove_container(id: &str) -> Result<(), String> {
    remove_container_impl(id)
}

#[cfg(target_os = "linux")]
fn remove_container_impl(id: &str) -> Result<(), String> {
    crate::linux::docker::remove_container(id).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
fn remove_container_impl(id: &str) -> Result<(), String> {
    let _ = id;
    Err("Docker n'est pas pris en charge sur cet OS.".to_string())
}

/// Remove an unused image by `reference` — the caller passes
/// [`ImageEntry::rmi_reference`] (repo:tag for a tagged row, ID for an
/// untagged `<none>:<none>` row). Plain `docker rmi`, no `-f`/`--force`.
pub fn remove_image(reference: &str) -> Result<(), String> {
    remove_image_impl(reference)
}

#[cfg(target_os = "linux")]
fn remove_image_impl(reference: &str) -> Result<(), String> {
    crate::linux::docker::remove_image(reference).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
fn remove_image_impl(reference: &str) -> Result<(), String> {
    let _ = reference;
    Err("Docker n'est pas pris en charge sur cet OS.".to_string())
}

/// Remove an orphan (dangling) volume by `name`. Plain `docker volume rm`,
/// no `-f`/`--force`.
pub fn remove_volume(name: &str) -> Result<(), String> {
    remove_volume_impl(name)
}

#[cfg(target_os = "linux")]
fn remove_volume_impl(name: &str) -> Result<(), String> {
    crate::linux::docker::remove_volume(name).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
fn remove_volume_impl(name: &str) -> Result<(), String> {
    let _ = name;
    Err("Docker n'est pas pris en charge sur cet OS.".to_string())
}

/// Compute every volume's on-disk size via `docker system df -v` — a slow
/// disk scan (~4.6s measured on the Linux reference machine), never called
/// as part of `fetch`. Returns `(name, size)` pairs the caller merges into
/// its own snapshot by name.
pub fn volume_sizes() -> Result<Vec<(String, String)>, String> {
    volume_sizes_impl()
}

#[cfg(target_os = "linux")]
fn volume_sizes_impl() -> Result<Vec<(String, String)>, String> {
    crate::linux::docker::volume_sizes().map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
fn volume_sizes_impl() -> Result<Vec<(String, String)>, String> {
    Err("Docker n'est pas pris en charge sur cet OS.".to_string())
}

// ---------------------------------------------------------------------------
// Pure view — "data in, actions out" (`cleanup_view.rs` precedent)
// ---------------------------------------------------------------------------

const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);

/// Everything `render` needs to draw one frame of the Docker tab. Owned and
/// assembled by `EguiApp` (Phase 3) from its own `docker: Option<Result<DockerSnapshot,
/// String>>` field plus a busy flag — this type never touches `EguiApp`
/// itself (the `cleanup_view::CleanupViewState` precedent).
pub struct DockerViewState<'a> {
    /// `None` before the first fetch has completed (successfully or not).
    pub snapshot: Option<&'a DockerSnapshot>,
    /// Last fetch failure to show in the banner, with a « Réessayer »
    /// button — set whenever the last `fetch()` returned `Err`, regardless
    /// of which `DockerError` variant produced the string (see `fetch`'s
    /// doc comment: the façade already did the classification the view
    /// would otherwise have to redo).
    pub error: Option<&'a str>,
    /// Global single-command-slot guard: disables every button while an
    /// action or a refetch is in flight.
    pub busy: bool,
}

/// One user intent emitted by `render`. Each destructive variant's `String`
/// is the exact identifier the façade action wrapper expects (container ID,
/// `rmi` reference, volume name) — never a display label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerAction {
    Refresh,
    Retry,
    StopContainer(String),
    RemoveContainer(String),
    RemoveImage(String),
    RemoveVolume(String),
    /// Not destructive — `EguiApp` must never open a confirm dialog for
    /// this one (see its dispatch in `render_docker_view`). Triggers a
    /// `docker system df -v` disk scan whose result gets merged into the
    /// current snapshot by volume name.
    ComputeVolumeSizes,
}

/// Reason a container's « Arrêter » button is disabled, or `None` when
/// stopping is offered. Mirrors `ContainerState::is_stoppable`.
fn stop_disabled_reason(state: &ContainerState) -> Option<&'static str> {
    if state.is_stoppable() {
        return None;
    }
    match state {
        ContainerState::Unknown(_) => Some("état du conteneur inconnu"),
        _ => Some("conteneur déjà arrêté"),
    }
}

/// Reason a container's « Supprimer » button is disabled, or `None` when
/// removal is offered. Mirrors `ContainerState::is_removable`.
fn remove_disabled_reason(state: &ContainerState) -> Option<&'static str> {
    if state.is_removable() {
        return None;
    }
    match state {
        ContainerState::Unknown(_) => Some("état du conteneur inconnu"),
        _ => Some("conteneur non arrêté"),
    }
}

fn container_state_label(state: &ContainerState) -> String {
    match state {
        ContainerState::Running => "en cours".to_string(),
        ContainerState::Paused => "en pause".to_string(),
        ContainerState::Restarting => "redémarrage".to_string(),
        ContainerState::Exited => "arrêté".to_string(),
        ContainerState::Created => "créé".to_string(),
        ContainerState::Dead => "mort".to_string(),
        ContainerState::Unknown(raw) if raw.is_empty() => "état inconnu".to_string(),
        ContainerState::Unknown(raw) => format!("état inconnu ({raw})"),
    }
}

/// Attach `.on_disabled_hover_text(reason)` only when the button is
/// state-disabled (`reason.is_some()`) — a button disabled purely by the
/// global `busy` guard gets no tooltip, since the section-level spinner
/// already explains that.
fn with_disabled_reason(button: egui::Response, reason: Option<&'static str>) -> egui::Response {
    match reason {
        Some(reason) => button.on_disabled_hover_text(reason),
        None => button,
    }
}

fn render_containers_section(
    ui: &mut egui::Ui,
    containers: &[ContainerEntry],
    buttons_enabled: bool,
    actions: &mut Vec<DockerAction>,
) {
    ui.strong("Conteneurs");
    if containers.is_empty() {
        ui.label("Aucun conteneur.");
        return;
    }
    egui::ScrollArea::horizontal()
        .id_salt("docker-containers-scroll")
        .show(ui, |ui| {
            egui::Grid::new("docker-containers-grid")
                .striped(true)
                .min_col_width(85.0)
                .show(ui, |ui| {
                    ui.strong("Nom");
                    ui.strong("Image");
                    ui.strong("État");
                    ui.strong("Statut");
                    ui.strong("Action");
                    ui.end_row();
                    for container in containers {
                        let label = if container.name.is_empty() {
                            &container.id
                        } else {
                            &container.name
                        };
                        ui.label(label);
                        ui.label(&container.image);
                        ui.label(container_state_label(&container.state));
                        ui.label(&container.status);
                        ui.horizontal(|ui| {
                            let stop_reason = stop_disabled_reason(&container.state);
                            let stop_button = with_disabled_reason(
                                ui.add_enabled(
                                    buttons_enabled && stop_reason.is_none(),
                                    egui::Button::new("Arrêter"),
                                ),
                                stop_reason,
                            );
                            if stop_button.clicked() {
                                actions.push(DockerAction::StopContainer(container.id.clone()));
                            }

                            let remove_reason = remove_disabled_reason(&container.state);
                            let remove_button = with_disabled_reason(
                                ui.add_enabled(
                                    buttons_enabled && remove_reason.is_none(),
                                    egui::Button::new("Supprimer"),
                                ),
                                remove_reason,
                            );
                            if remove_button.clicked() {
                                actions.push(DockerAction::RemoveContainer(container.id.clone()));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
}

fn render_images_section(
    ui: &mut egui::Ui,
    images: &[ImageEntry],
    buttons_enabled: bool,
    actions: &mut Vec<DockerAction>,
) {
    ui.strong("Images");
    if images.is_empty() {
        ui.label("Aucune image.");
        return;
    }
    egui::ScrollArea::horizontal()
        .id_salt("docker-images-scroll")
        .show(ui, |ui| {
            egui::Grid::new("docker-images-grid")
                .striped(true)
                .min_col_width(85.0)
                .show(ui, |ui| {
                    ui.strong("Image");
                    ui.strong("Taille");
                    ui.strong("Créée le");
                    ui.strong("Utilisée");
                    ui.strong("Action");
                    ui.end_row();
                    for image in images {
                        // Untagged `<none>:<none>` rows display their short
                        // ID as identity (plan Phase 2 task 6) — the
                        // repo:tag identity is meaningless for them.
                        let label = if image.is_untagged() {
                            &image.id
                        } else {
                            &image.identity
                        };
                        ui.label(label);
                        ui.label(&image.size);
                        ui.label(&image.created);
                        ui.label(if image.used { "oui" } else { "non" });
                        let reason = image
                            .used
                            .then_some("utilisée par un ou plusieurs conteneurs (N indisponible)");
                        let remove_button = with_disabled_reason(
                            ui.add_enabled(
                                buttons_enabled && reason.is_none(),
                                egui::Button::new("Supprimer l'image"),
                            ),
                            reason,
                        );
                        if remove_button.clicked() {
                            actions.push(DockerAction::RemoveImage(image.rmi_reference.clone()));
                        }
                        ui.end_row();
                    }
                });
        });
}

fn render_volumes_section(
    ui: &mut egui::Ui,
    volumes: &[VolumeEntry],
    buttons_enabled: bool,
    actions: &mut Vec<DockerAction>,
) {
    ui.horizontal(|ui| {
        ui.strong("Volumes");
        if ui
            .add_enabled(buttons_enabled, egui::Button::new("Calculer les tailles"))
            .clicked()
        {
            actions.push(DockerAction::ComputeVolumeSizes);
        }
    });
    if volumes.is_empty() {
        ui.label("Aucun volume.");
        return;
    }
    egui::ScrollArea::horizontal()
        .id_salt("docker-volumes-scroll")
        .show(ui, |ui| {
            egui::Grid::new("docker-volumes-grid")
                .striped(true)
                .min_col_width(85.0)
                .show(ui, |ui| {
                    ui.strong("Nom");
                    ui.strong("Driver");
                    ui.strong("Orphelin");
                    ui.strong("Taille");
                    ui.strong("Action");
                    ui.end_row();
                    for volume in volumes {
                        ui.label(&volume.name);
                        ui.label(&volume.driver);
                        ui.label(if volume.orphan { "oui" } else { "non" });
                        ui.label(volume.size.as_deref().unwrap_or("?"));
                        let reason = (!volume.orphan).then_some("volume rattaché à un conteneur");
                        let remove_button = with_disabled_reason(
                            ui.add_enabled(
                                buttons_enabled && reason.is_none(),
                                egui::Button::new("Supprimer le volume"),
                            ),
                            reason,
                        );
                        if remove_button.clicked() {
                            actions.push(DockerAction::RemoveVolume(volume.name.clone()));
                        }
                        ui.end_row();
                    }
                });
        });
}

/// Draw one frame of the Docker tab from `state` and return every intent the
/// user clicked this frame — zero `EguiApp` access, exactly the
/// `cleanup_view::render` contract.
pub fn render(ui: &mut egui::Ui, state: &DockerViewState<'_>) -> Vec<DockerAction> {
    let mut actions = Vec::new();
    let buttons_enabled = !state.busy;

    ui.horizontal(|ui| {
        ui.heading("Docker");
        if ui
            .add_enabled(buttons_enabled, egui::Button::new("Actualiser"))
            .clicked()
        {
            actions.push(DockerAction::Refresh);
        }
        if state.busy {
            ui.spinner();
        }
    });

    if let Some(error) = state.error {
        ui.colored_label(ERROR_COLOR, error);
        if ui
            .add_enabled(buttons_enabled, egui::Button::new("Réessayer"))
            .clicked()
        {
            actions.push(DockerAction::Retry);
        }
        return actions;
    }

    let Some(snapshot) = state.snapshot else {
        ui.label(if state.busy {
            "Chargement des données Docker…"
        } else {
            "Aucune donnée Docker chargée."
        });
        return actions;
    };

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.separator();
        render_containers_section(ui, &snapshot.containers, buttons_enabled, &mut actions);
        ui.separator();
        render_images_section(ui, &snapshot.images, buttons_enabled, &mut actions);
        ui.separator();
        render_volumes_section(ui, &snapshot.volumes, buttons_enabled, &mut actions);
    });

    actions
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    // --- ImageEntry::is_untagged --------------------------------------------

    #[test]
    fn is_untagged_detects_the_none_none_placeholder() {
        let entry = image_entry("581c17389e54", "<none>:<none>", false, "581c17389e54");
        assert!(entry.is_untagged());
    }

    #[test]
    fn is_untagged_is_false_for_a_tagged_row() {
        let entry = image_entry("581c17389e54", "nginx:alpine", false, "nginx:alpine");
        assert!(!entry.is_untagged());
    }

    // --- stop_disabled_reason / remove_disabled_reason -----------------------

    #[test]
    fn stop_disabled_reason_is_none_for_every_stoppable_state() {
        for state in [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
        ] {
            assert_eq!(stop_disabled_reason(&state), None, "state {state:?}");
        }
    }

    #[test]
    fn stop_disabled_reason_is_some_for_every_removable_state() {
        for state in [
            ContainerState::Exited,
            ContainerState::Created,
            ContainerState::Dead,
        ] {
            assert!(stop_disabled_reason(&state).is_some(), "state {state:?}");
        }
    }

    #[test]
    fn stop_disabled_reason_explains_unknown_state_specifically() {
        let reason = stop_disabled_reason(&ContainerState::Unknown("mystère".to_string()))
            .expect("unknown state must disable stop");
        assert!(reason.contains("inconnu"));
    }

    #[test]
    fn remove_disabled_reason_is_none_for_every_removable_state() {
        for state in [
            ContainerState::Exited,
            ContainerState::Created,
            ContainerState::Dead,
        ] {
            assert_eq!(remove_disabled_reason(&state), None, "state {state:?}");
        }
    }

    #[test]
    fn remove_disabled_reason_is_some_for_every_stoppable_state() {
        for state in [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
        ] {
            assert!(remove_disabled_reason(&state).is_some(), "state {state:?}");
        }
    }

    #[test]
    fn remove_disabled_reason_explains_unknown_state_specifically() {
        let reason = remove_disabled_reason(&ContainerState::Unknown("mystère".to_string()))
            .expect("unknown state must disable remove");
        assert!(reason.contains("inconnu"));
    }

    // --- container_state_label ------------------------------------------------

    #[test]
    fn container_state_label_covers_every_known_variant_without_panicking() {
        let states = [
            ContainerState::Running,
            ContainerState::Paused,
            ContainerState::Restarting,
            ContainerState::Exited,
            ContainerState::Created,
            ContainerState::Dead,
            ContainerState::Unknown("weird".to_string()),
            ContainerState::Unknown(String::new()),
        ];
        for state in &states {
            assert!(!container_state_label(state).is_empty());
        }
    }

    // --- façade — Linux delegation sanity (this dev machine has docker) ------

    #[cfg(target_os = "linux")]
    #[test]
    fn available_delegates_to_linux_binary_detection() {
        assert_eq!(available(), crate::linux::docker::binary_available());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fetch_through_facade_succeeds_on_this_machine_when_docker_is_available() {
        if !available() {
            eprintln!("docker introuvable sur cette machine: test ignoré");
            return;
        }
        // Smoke test: the façade must compile the Result<_, DockerError> ->
        // Result<_, String> conversion end to end without panicking, and a
        // successful fetch on this reference machine (Docker 29.7.2, daemon
        // reachable — see `crate::linux::docker`'s own real-machine tests)
        // must actually come back `Ok`.
        let snapshot = fetch();
        assert!(
            snapshot.is_ok(),
            "expected fetch() to succeed on this machine, got {snapshot:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn remove_container_through_facade_reports_a_readable_error_for_a_bogus_id() {
        if !available() {
            eprintln!("docker introuvable sur cette machine: test ignoré");
            return;
        }
        let result = remove_container("this-container-id-does-not-exist-devtoolbox");
        let error =
            result.expect_err("removing a nonexistent container id must fail, not panic/succeed");
        assert!(!error.is_empty(), "façade error string must not be empty");
    }

    // --- render(): fixtures ----------------------------------------------------

    fn container_entry(id: &str, name: &str, state: ContainerState) -> ContainerEntry {
        ContainerEntry {
            id: id.to_string(),
            name: name.to_string(),
            image: format!("{name}-image:latest"),
            state,
            status: "Up 3 hours".to_string(),
            rw_size: "767kB".to_string(),
        }
    }

    fn image_entry(id: &str, identity: &str, used: bool, rmi_reference: &str) -> ImageEntry {
        ImageEntry {
            id: id.to_string(),
            identity: identity.to_string(),
            size: "10MB".to_string(),
            created: "hier".to_string(),
            used,
            rmi_reference: rmi_reference.to_string(),
        }
    }

    fn volume_entry(name: &str, orphan: bool) -> VolumeEntry {
        VolumeEntry {
            name: name.to_string(),
            driver: "local".to_string(),
            orphan,
            size: None,
        }
    }

    fn volume_entry_with_size(name: &str, orphan: bool, size: &str) -> VolumeEntry {
        VolumeEntry {
            size: Some(size.to_string()),
            ..volume_entry(name, orphan)
        }
    }

    struct State {
        snapshot: Option<DockerSnapshot>,
        error: Option<String>,
        busy: bool,
        actions: Vec<DockerAction>,
    }

    impl State {
        fn with_snapshot(snapshot: DockerSnapshot) -> Self {
            State {
                snapshot: Some(snapshot),
                error: None,
                busy: false,
                actions: Vec::new(),
            }
        }
    }

    fn build_harness(state: State) -> Harness<'static, State> {
        // A busy view renders a spinner, which keeps requesting repaints
        // forever; `Harness::run()`'s settle-check would never succeed for
        // it (it panics past `max_steps`), so busy harnesses are stepped a
        // fixed number of frames instead — still enough for widgets to be
        // laid out and queryable/clickable.
        let busy = state.busy;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1000.0, 900.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    let view_state = DockerViewState {
                        snapshot: state.snapshot.as_ref(),
                        error: state.error.as_deref(),
                        busy: state.busy,
                    };
                    let emitted = render(ui, &view_state);
                    state.actions.extend(emitted);
                },
                state,
            );
        if busy {
            harness.run_steps(2);
        } else {
            harness.run();
        }
        harness
    }

    // --- render(): gating — used image / non-orphan volume / running-        --
    // --- container removal / unknown state never emit a destructive action ---

    #[test]
    fn used_image_delete_button_exists_but_is_disabled() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![image_entry("aaa", "nginx:alpine", true, "nginx:alpine")],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(
            harness.query_all_by_label("Supprimer l'image").count(),
            1,
            "the button must still be rendered, just disabled"
        );
        harness.get_by_label("Supprimer l'image").click();
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "a used image must never emit a RemoveImage action"
        );
    }

    #[test]
    fn non_orphan_volume_delete_button_exists_but_is_disabled() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry("proxy_certs", false)],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(
            harness.query_all_by_label("Supprimer le volume").count(),
            1,
            "the button must still be rendered, just disabled"
        );
        harness.get_by_label("Supprimer le volume").click();
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "a non-orphan volume must never emit a RemoveVolume action"
        );
    }

    #[test]
    fn running_container_remove_button_exists_but_is_disabled() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "proxy", ContainerState::Running)],
            images: vec![],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(
            harness.query_all_by_label("Supprimer").count(),
            1,
            "the remove button must still be rendered, just disabled"
        );
        harness.get_by_label("Supprimer").click();
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "a running container must never emit a RemoveContainer action"
        );
        // Sanity: the stop button on the same row is the enabled one.
        assert_eq!(harness.query_all_by_label("Arrêter").count(), 1);
    }

    #[test]
    fn unknown_state_container_has_both_buttons_disabled() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry(
                "c1",
                "mystery",
                ContainerState::Unknown("future-state".to_string()),
            )],
            images: vec![],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(harness.query_all_by_label("Arrêter").count(), 1);
        assert_eq!(harness.query_all_by_label("Supprimer").count(), 1);
        harness.get_by_label("Arrêter").click();
        harness.get_by_label("Supprimer").click();
        harness.run();
        assert!(
            harness.state().actions.is_empty(),
            "an unknown container state must offer no action at all"
        );
    }

    // --- render(): active buttons emit exactly the right action --------------

    #[test]
    fn stoppable_container_stop_click_emits_stop_container_with_its_id() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "proxy", ContainerState::Running)],
            images: vec![],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        harness.get_by_label("Arrêter").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::StopContainer("c1".to_string())]
        );
    }

    #[test]
    fn removable_container_remove_click_emits_remove_container_with_its_id() {
        let snapshot = DockerSnapshot {
            containers: vec![container_entry("c1", "proxy", ContainerState::Exited)],
            images: vec![],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        harness.get_by_label("Supprimer").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::RemoveContainer("c1".to_string())]
        );
    }

    #[test]
    fn unused_image_remove_click_emits_remove_image_with_its_rmi_reference() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![image_entry("aaa", "nginx:alpine", false, "nginx:alpine")],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        harness.get_by_label("Supprimer l'image").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::RemoveImage("nginx:alpine".to_string())]
        );
    }

    #[test]
    fn untagged_image_remove_click_emits_remove_image_with_its_id() {
        // Untagged `<none>:<none>` rows are removed by ID (`rmi_reference`),
        // never by their meaningless placeholder identity.
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![image_entry(
                "581c17389e54",
                "<none>:<none>",
                false,
                "581c17389e54",
            )],
            volumes: vec![],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        // The row identity shown must be the short ID, not "<none>:<none>".
        assert_eq!(harness.query_all_by_label("581c17389e54").count(), 1);
        assert_eq!(harness.query_all_by_label("<none>:<none>").count(), 0);
        harness.get_by_label("Supprimer l'image").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::RemoveImage("581c17389e54".to_string())]
        );
    }

    #[test]
    fn orphan_volume_remove_click_emits_remove_volume_with_its_name() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry("dangling-vol", true)],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        harness.get_by_label("Supprimer le volume").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::RemoveVolume("dangling-vol".to_string())]
        );
    }

    #[test]
    fn volume_size_column_shows_question_mark_when_not_yet_computed() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry("dangling-vol", true)],
        };
        let harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(
            harness.query_all_by_label("?").count(),
            1,
            "an unknown volume size must render as a literal '?'"
        );
    }

    #[test]
    fn volume_size_column_shows_computed_size_when_present() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry_with_size("dangling-vol", true, "6.64MB")],
        };
        let harness = build_harness(State::with_snapshot(snapshot));
        assert_eq!(harness.query_all_by_label("6.64MB").count(), 1);
        assert_eq!(harness.query_all_by_label("?").count(), 0);
    }

    #[test]
    fn calculer_les_tailles_click_emits_compute_volume_sizes() {
        let snapshot = DockerSnapshot {
            containers: vec![],
            images: vec![],
            volumes: vec![volume_entry("dangling-vol", true)],
        };
        let mut harness = build_harness(State::with_snapshot(snapshot));
        harness.get_by_label("Calculer les tailles").click();
        harness.run();
        assert_eq!(
            harness.state().actions,
            vec![DockerAction::ComputeVolumeSizes]
        );
    }

    #[test]
    fn calculer_les_tailles_is_rendered_even_with_no_volumes() {
        // The button lives in the section header, not gated by an empty list.
        let harness = build_harness(State::with_snapshot(DockerSnapshot::default()));
        assert_eq!(
            harness.query_all_by_label("Calculer les tailles").count(),
            1
        );
    }

    // --- render(): refresh / retry / busy / empty / error --------------------

    #[test]
    fn actualiser_button_emits_refresh() {
        let mut harness = build_harness(State::with_snapshot(DockerSnapshot::default()));
        harness.get_by_label("Actualiser").click();
        harness.run();
        assert_eq!(harness.state().actions, vec![DockerAction::Refresh]);
    }

    #[test]
    fn error_state_shows_reessayer_and_emits_retry_on_click() {
        let state = State {
            snapshot: None,
            error: Some("daemon Docker inaccessible: connection refused".to_string()),
            busy: false,
            actions: Vec::new(),
        };
        let mut harness = build_harness(state);
        assert_eq!(harness.query_all_by_label("Réessayer").count(), 1);
        // No section is rendered while an error is shown.
        assert_eq!(harness.query_all_by_label("Conteneurs").count(), 0);
        harness.get_by_label("Réessayer").click();
        harness.run();
        assert_eq!(harness.state().actions, vec![DockerAction::Retry]);
    }

    #[test]
    fn busy_state_disables_every_button_so_no_action_is_emitted() {
        let state = State {
            snapshot: Some(DockerSnapshot {
                containers: vec![container_entry("c1", "proxy", ContainerState::Running)],
                images: vec![image_entry("aaa", "nginx:alpine", false, "nginx:alpine")],
                volumes: vec![volume_entry("dangling-vol", true)],
            }),
            error: None,
            busy: true,
            actions: Vec::new(),
        };
        let mut harness = build_harness(state);
        harness.get_by_label("Actualiser").click();
        harness.get_by_label("Arrêter").click();
        harness.get_by_label("Supprimer l'image").click();
        harness.get_by_label("Supprimer le volume").click();
        harness.get_by_label("Calculer les tailles").click();
        // Same spinner-driven continuous-repaint reason as in build_harness:
        // step a fixed number of frames rather than `run()`'s settle-check.
        harness.run_steps(2);
        assert!(
            harness.state().actions.is_empty(),
            "busy must gate every button, including otherwise-allowed ones"
        );
    }

    #[test]
    fn empty_snapshot_shows_french_placeholders_for_every_section() {
        let harness = build_harness(State::with_snapshot(DockerSnapshot::default()));
        assert_eq!(harness.query_all_by_label("Aucun conteneur.").count(), 1);
        assert_eq!(harness.query_all_by_label("Aucune image.").count(), 1);
        assert_eq!(harness.query_all_by_label("Aucun volume.").count(), 1);
    }

    #[test]
    fn no_snapshot_and_no_error_shows_a_loading_or_empty_placeholder() {
        let state = State {
            snapshot: None,
            error: None,
            busy: false,
            actions: Vec::new(),
        };
        let harness = build_harness(state);
        assert_eq!(
            harness
                .query_all_by_label("Aucune donnée Docker chargée.")
                .count(),
            1
        );
    }
}
