//! Compose-stacks view — OS-neutral types, façade and pure rendering for the
//! Docker tab's "Stacks" section (Part 2 of
//! `aidd_docs/tasks/2026_08/2026_08_21-docker-compose-ports-cleanup-master.md`).
//!
//! Same split as [`crate::ui::docker_view`]: every type the UI names lives
//! here, compiled on every OS, and `crate::docker::compose` (which compiles to
//! nothing off Linux) is the only module that knows how to *produce* them from
//! `docker compose` output. Putting [`StackConfig`] / [`StackService`] in the
//! Linux-only module would break the Windows build the moment [`StackEntry`]
//! named them.
//!
//! # Measured contracts (this machine, 2026-08-21, compose v5.5.0)
//!
//! Everything below was captured from the 13 real compose files under `$HOME`
//! before a line of this module was written:
//!
//! - `docker compose -f <file> config --format json` answers in ~89 ms **with
//!   the daemon irrelevant** — it is pure file resolution, which is why
//!   DevToolBox parses no YAML itself.
//! - It writes `level=warning` lines (unset `.env` variables, mostly) to
//!   **stderr** while stdout stays clean JSON. Reading the two merged — the
//!   obvious `2>&1` reflex — corrupts the parse on 6 of the 13 files here.
//!   [`crate::docker::compose::read_config`] therefore consumes stdout only.
//! - `published` is a **string** (`"8081"`) in every one of the 13 files.
//!   Other schema versions emit a number, so [`PublishedPort`] accepts both.
//! - `host_ip` is **absent** from every published port here; a service that
//!   publishes nothing has **no `ports` key at all** (not `null`). Both
//!   degrade to the wildcard / empty case rather than failing the row.
//! - Two distinct files both resolve to the project name `proxy`
//!   (`appv3/proxy` and `onet/pilotphone/proxy`). The row identity is
//!   therefore the **file path**, never the project name.
//! - One project (`suddenly`) is running from a `config_files` path that no
//!   longer exists on disk — see [`link_runs`], which surfaces it instead of
//!   dropping it.

use std::collections::BTreeMap;
use std::path::Path;

use eframe::egui;

use crate::ui::docker_view::{ContainerEntry, ContainerState};
use crate::ui::ports::{self, OwnerKind, PortBinding, PortConflict, PortOwner};

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// What one `docker compose config` call resolved: the project name compose
/// itself derived, and the services it declares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackConfig {
    pub name: String,
    pub services: Vec<StackService>,
}

/// One service of a compose file, reduced to what the Stacks section shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackService {
    pub name: String,
    /// Declared published bindings. Empty both for a service that publishes
    /// nothing and for a `network_mode: host` one — `host_network` is what
    /// tells those two apart.
    pub ports: Vec<PortBinding>,
    /// `network_mode: host`: the service takes the host's network stack
    /// wholesale, so it declares no published port and its real port usage
    /// cannot be compared against anything. Measured here on `qt.proxy`.
    pub host_network: bool,
}

/// One running instance of a compose file, keyed by the `-p` project name it
/// was started under. A single file started under two project names produces
/// two of these, which is why the actions carry a project rather than assuming
/// the default one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackRun {
    pub project: String,
    pub running: usize,
    /// Containers in a state that is genuinely wrong: `Restarting`, or
    /// `Exited` with a non-zero code. An `Exited (0)` one-shot init container
    /// is **not** counted here — see [`is_failing`].
    pub failing: usize,
    pub total: usize,
}

/// The state shown on a stack row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackState {
    /// At least one container running, none failing.
    Running,
    /// At least one running **and** at least one genuinely failing.
    Partial,
    /// No container of this project exists right now.
    Stopped,
    /// The compose file is not on disk anymore. Deliberately a state rather
    /// than a separate `missing: bool` field: two fields encoding one fact are
    /// two fields that can disagree.
    Missing,
    /// The daemon could not be reached, so no run state can be asserted. The
    /// file and its declared ports still show — `config` needs no daemon —
    /// but rendering `arrêtée` here would be a lie.
    Unknown,
}

impl StackState {
    pub fn label(&self) -> &'static str {
        match self {
            StackState::Running => "tourne",
            StackState::Partial => "partielle",
            StackState::Stopped => "arrêtée",
            StackState::Missing => "fichier introuvable",
            StackState::Unknown => "état inconnu",
        }
    }
}

/// One row of the Stacks section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackEntry {
    /// Absolute path of the compose file — the row's identity, because two
    /// different files can resolve to the same project name (measured: two
    /// `proxy` here).
    pub file: String,
    /// Project name as compose resolved it from the file.
    pub project: String,
    pub services: Vec<StackService>,
    pub runs: Vec<StackRun>,
    pub state: StackState,
    /// Why this file could not be read, when it could not. Rendered on the row
    /// itself: one invalid compose file must never blank the whole list.
    pub error: Option<String>,
}

impl StackEntry {
    /// A row for a file that could not be read at all.
    pub fn failed(file: impl Into<String>, error: impl Into<String>) -> Self {
        let file = file.into();
        StackEntry {
            project: default_project_name(&file),
            file,
            services: Vec::new(),
            runs: Vec::new(),
            state: StackState::Stopped,
            error: Some(error.into()),
        }
    }
}

/// What a `$HOME` walk found, plus what it cost. Never silently truncated: a
/// walk that ran long carries a `warning` and the UI says so.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanOutcome {
    pub files: Vec<String>,
    pub visited_dirs: usize,
    pub elapsed_ms: u128,
    /// Directories the walk could not read (permissions, vanished mid-walk).
    /// Counted, never fatal.
    pub denied_dirs: usize,
    pub warning: Option<String>,
}

/// The target of a stack action: which file, and — when the row represents a
/// live run — which project name that run carries.
///
/// Without the project, `Arrêter` on a stack started under an explicit `-p`
/// would address the default project name (the file's parent directory) and
/// silently stop nothing: compose resolves a project by name, not by file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackTarget {
    pub file: String,
    pub project: Option<String>,
}

/// One user intent emitted by [`render`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackAction {
    Scan,
    Up(StackTarget),
    Stop(StackTarget),
    Down(StackTarget),
    /// Drop a memorized file that has vanished from disk. Carries the file
    /// path alone — there is nothing running to address.
    Forget(String),
    /// Dismiss the log panel. Carries nothing: only one compose command runs
    /// at a time, so there is only ever one log to close.
    CloseLog,
}

/// Everything [`render`] needs for one frame. Everything borrowed, nothing
/// owned, so the module stays testable without an app instance.
pub struct ComposeViewState<'a> {
    pub stacks: &'a [StackEntry],
    pub conflicts: &'a [PortConflict],
    pub plugin_available: bool,
    pub scanning: bool,
    /// A compose command is in flight — one at a time, so the inline log
    /// always has exactly one owner.
    pub busy: bool,
    pub log: &'a [String],
    /// File path of the row the inline log belongs to.
    pub log_target: Option<&'a str>,
    pub scan_warning: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// Pure logic
// ---------------------------------------------------------------------------

/// The project name compose derives when none is given: the file's parent
/// directory, lowercased with everything but `[a-z0-9_-]` dropped.
///
/// Only used for display on a row whose `config` call failed — the real name
/// always comes from compose itself, which is the authority on its own
/// normalization rules.
pub fn default_project_name(file: &str) -> String {
    Path::new(file)
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .map(|name| {
            name.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// `true` when a container's state means the stack is genuinely broken.
///
/// `Exited (0)` is a normally-finished one-shot task, never a failure —
/// without that rule the `db-init` / `vault-init` containers of four real
/// stacks here would pin healthy projects to `partielle` forever.
///
/// `137` and `143` are not failures either: they are `128 + SIGKILL` and
/// `128 + SIGTERM`, i.e. exactly what `docker stop` leaves behind when a
/// container does not handle the signal itself. Measured on this machine,
/// the `proxy` project's two deliberately stopped containers both report
/// `Exited (137)`, and reading that as a crash would pin every stack the
/// user stopped by hand to `partielle`. The cost is an OOM kill (also 137)
/// passing for a clean stop — the lesser of the two wrong answers.
pub fn is_failing(state: &ContainerState, exit_code: Option<i32>) -> bool {
    match state {
        ContainerState::Restarting => true,
        ContainerState::Dead => true,
        ContainerState::Exited => !matches!(exit_code, Some(0) | Some(137) | Some(143)),
        _ => false,
    }
}

/// The tourne / partielle / arrêtée rule for one run.
pub fn classify(run: &StackRun) -> StackState {
    // Checked *before* the `running == 0` test: `running` counts genuinely
    // running containers only, so a project whose every service is stuck in
    // `Restarting` has `running == 0` and would otherwise read « arrêtée » —
    // the exact opposite of what is happening. Real case on this machine:
    // `pilotphone`, 6 of its 7 services restarting.
    if run.failing > 0 {
        return StackState::Partial;
    }
    if run.running == 0 {
        return StackState::Stopped;
    }
    StackState::Running
}

/// Aggregate of every run of one file: a `partielle` anywhere wins over a
/// `tourne`, and no run at all means `arrêtée`.
fn aggregate_state(runs: &[StackRun]) -> StackState {
    if runs.is_empty() {
        return StackState::Stopped;
    }
    let states: Vec<StackState> = runs.iter().map(classify).collect();
    if states.contains(&StackState::Partial) {
        StackState::Partial
    } else if states.contains(&StackState::Running) {
        StackState::Running
    } else {
        StackState::Stopped
    }
}

/// Attach the live containers to the compose files they came from.
///
/// The link reads `com.docker.compose.project.config_files` and
/// `com.docker.compose.project` from the **grouped `docker inspect`** Part 1
/// already runs (`.Config.Labels`), not from `docker ps`'s flat `Labels`
/// string: that string joins labels with `,` while `config_files` is itself a
/// `,`-separated list, so a multi-file project is genuinely ambiguous there.
/// Extending Part 1's template costs zero extra docker calls.
///
/// Returns a **new** list rather than mutating in place, for two reasons:
/// one file running under several `-p` names produces more runs than it
/// received, and a project running from a `config_files` path that is not in
/// `stacks` at all gets a row of its own. That last case is real, not
/// theoretical: `suddenly` runs here from a path that no longer exists on
/// disk, and dropping it would hide a live stack the user can still stop.
/// A container with no compose label never appears in Stacks.
pub fn link_runs(
    stacks: &[StackEntry],
    containers: &[ContainerEntry],
    file_exists: &dyn Fn(&str) -> bool,
) -> Vec<StackEntry> {
    // (file, project) → tally, ordered so the output is stable.
    let mut tallies: BTreeMap<(String, String), StackRun> = BTreeMap::new();
    for container in containers {
        let Some(project) = container.compose_project.as_deref() else {
            continue;
        };
        if project.is_empty() {
            continue;
        }
        for file in &container.compose_files {
            let entry = tallies
                .entry((file.clone(), project.to_string()))
                .or_insert_with(|| StackRun {
                    project: project.to_string(),
                    ..StackRun::default()
                });
            entry.total += 1;
            // `Running`, not `is_stoppable()`: a restarting container *can*
            // be stopped but is not « en marche », and counting it in both
            // tallies produced the nonsense « 2/2 en marche · 1 en échec ».
            if matches!(container.state, ContainerState::Running) {
                entry.running += 1;
            }
            if is_failing(&container.state, container.exit_code) {
                entry.failing += 1;
            }
        }
    }

    let mut linked: Vec<StackEntry> = stacks
        .iter()
        .map(|stack| {
            let runs: Vec<StackRun> = tallies
                .iter()
                .filter(|((file, _), _)| file == &stack.file)
                .map(|(_, run)| run.clone())
                .collect();
            let state = match stack.state {
                // A file that is gone stays gone, whatever is running from it.
                StackState::Missing | StackState::Unknown => stack.state,
                _ => aggregate_state(&runs),
            };
            StackEntry {
                runs,
                state,
                ..stack.clone()
            }
        })
        .collect();

    // Runs whose compose file is in no scanned row: surface them anyway, so
    // they can still be stopped or destroyed.
    let known: Vec<&str> = stacks.iter().map(|stack| stack.file.as_str()).collect();
    for ((file, _), run) in &tallies {
        if known.contains(&file.as_str()) {
            continue;
        }
        match linked.iter_mut().find(|entry| &entry.file == file) {
            Some(entry) => entry.runs.push(run.clone()),
            None => linked.push(StackEntry {
                file: file.clone(),
                project: run.project.clone(),
                services: Vec::new(),
                runs: vec![run.clone()],
                // Placeholder: the real state needs every run of this file,
                // which the loop has not finished collecting yet.
                state: StackState::Unknown,
                error: None,
            }),
        }
    }
    // Absent from the scanned list is **not** the same as absent from the
    // disk. Before the first scan the list is empty and every live stack
    // lands here, so asserting `Missing` from list membership alone made the
    // UI claim « fichier introuvable » about four files, three of which were
    // sitting right there. Only the filesystem can tell the two apart, and it
    // is injected so this stays a pure function.
    for entry in linked.iter_mut().skip(stacks.len()) {
        entry.state = if file_exists(&entry.file) {
            aggregate_state(&entry.runs)
        } else {
            StackState::Missing
        };
    }
    linked
}

/// The declared ports of every stack, as owners ready for
/// [`ports::find_conflicts`] alongside Part 1's running-container owners.
///
/// This is the case the conflict badge was actually designed for: two *running*
/// containers can never share a host port — the kernel refuses the second bind
/// — so the only collision worth warning about is a declared stack against
/// what is already up.
///
/// A `Missing` row declares nothing (its file is gone, so its ports are
/// unknown) and a `Running` one is already represented by its containers'
/// real bindings, so neither contributes an owner: only a stack that could
/// still be started does.
pub fn declared_owners(stacks: &[StackEntry]) -> Vec<PortOwner> {
    stacks
        .iter()
        .filter(|stack| matches!(stack.state, StackState::Stopped | StackState::Unknown))
        .filter_map(|stack| {
            let bindings: Vec<PortBinding> = stack
                .services
                .iter()
                .flat_map(|service| service.ports.iter().cloned())
                .collect();
            if bindings.is_empty() {
                return None;
            }
            Some(
                PortOwner::new(
                    stack.file.clone(),
                    format!("stack {}", stack.project),
                    OwnerKind::DeclaredStack,
                    bindings,
                )
                .with_source(stack.file.clone()),
            )
        })
        .collect()
}

/// The declared ports of one stack, in column form (`8081, 443, 3000`).
fn declared_ports_text(stack: &StackEntry) -> String {
    let mut ports: Vec<String> = stack
        .services
        .iter()
        .flat_map(|service| service.ports.iter())
        .map(|binding| format!("{}/{}", binding.host_port, binding.protocol))
        .collect();
    ports.dedup();
    ports.join(", ")
}

/// Conflicts touching one owner label, phrased for a hover text.
fn conflicts_for(conflicts: &[PortConflict], label: &str) -> Vec<String> {
    conflicts
        .iter()
        .filter(|conflict| conflict.owners.iter().any(|owner| owner == label))
        .map(|conflict| {
            let others: Vec<&str> = conflict
                .owners
                .iter()
                .filter(|owner| owner.as_str() != label)
                .map(String::as_str)
                .collect();
            format!(
                "{}/{} également utilisé par {}",
                conflict.host_port,
                conflict.protocol,
                others.join(", ")
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// OS-neutral façade — same cfg layout as `docker_view`'s
// ---------------------------------------------------------------------------

/// `true` when the `docker compose` **plugin** (v2+) answers. The legacy
/// `docker-compose` binary is out of scope.
pub fn plugin_available() -> bool {
    plugin_available_impl()
}

fn plugin_available_impl() -> bool {
    crate::docker::compose::plugin_available()
}

/// Walk `root` for compose files.
pub fn discover(root: &Path) -> ScanOutcome {
    discover_impl(root)
}

fn discover_impl(root: &Path) -> ScanOutcome {
    crate::docker::compose::discover(root)
}

/// Read one compose file. The error string is already a formatted, French,
/// ready-to-display message, exactly as [`crate::ui::docker_view::fetch`]
/// flattens `DockerError` at the same boundary.
pub fn read_config(file: &Path) -> Result<StackConfig, String> {
    read_config_impl(file)
}

fn read_config_impl(file: &Path) -> Result<StackConfig, String> {
    crate::docker::compose::read_config(file).map_err(|error| error.to_string())
}

/// argv for `docker compose … up -d` on `target`.
pub fn up_args(target: &StackTarget) -> Vec<String> {
    compose_args(target, &["up", "-d"])
}

/// argv for `docker compose … stop`.
pub fn stop_args(target: &StackTarget) -> Vec<String> {
    compose_args(target, &["stop"])
}

/// argv for `docker compose … down`. **Never** `-v`: volumes are only ever
/// deleted through the explicit volume flow.
pub fn down_args(target: &StackTarget) -> Vec<String> {
    compose_args(target, &["down"])
}

/// `["compose", "-f", <file>, ("-p", <project>,)? <subcommand…>]`.
///
/// `-p` sits **before** the subcommand, which is where compose accepts it, and
/// is present only when the row targets a named run.
fn compose_args(target: &StackTarget, subcommand: &[&str]) -> Vec<String> {
    let mut args = vec!["compose".to_string(), "-f".to_string(), target.file.clone()];
    if let Some(project) = &target.project {
        args.push("-p".to_string());
        args.push(project.clone());
    }
    args.extend(subcommand.iter().map(|part| (*part).to_string()));
    args
}

// ---------------------------------------------------------------------------
// Pure view — "data in, actions out"
// ---------------------------------------------------------------------------

const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);
const CONFLICT_COLOR: egui::Color32 = egui::Color32::from_rgb(0xB7, 0x6E, 0x00);
const MUTED_COLOR: egui::Color32 = egui::Color32::from_rgb(0x80, 0x80, 0x80);
const RUNNING_COLOR: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x5E, 0x20);

fn state_color(state: StackState) -> egui::Color32 {
    match state {
        StackState::Running => RUNNING_COLOR,
        StackState::Partial => CONFLICT_COLOR,
        StackState::Missing => ERROR_COLOR,
        StackState::Stopped | StackState::Unknown => MUTED_COLOR,
    }
}

/// Draw the Stacks section and return every intent the user expressed.
pub fn render(ui: &mut egui::Ui, state: &ComposeViewState<'_>) -> Vec<StackAction> {
    let mut actions = Vec::new();

    ui.horizontal(|ui| {
        ui.strong("Stacks");
        let can_scan = state.plugin_available && !state.scanning && !state.busy;
        if ui
            .add_enabled(can_scan, egui::Button::new("Scanner"))
            .on_disabled_hover_text(if state.plugin_available {
                "un scan ou une commande compose est déjà en cours"
            } else {
                "plugin « docker compose » introuvable"
            })
            .clicked()
        {
            actions.push(StackAction::Scan);
        }
        if state.scanning {
            ui.spinner();
            ui.label("Scan en cours…");
        }
    });

    if !state.plugin_available {
        ui.colored_label(ERROR_COLOR, "plugin « docker compose » introuvable");
        return actions;
    }

    if let Some(warning) = state.scan_warning {
        ui.colored_label(CONFLICT_COLOR, warning);
    }

    if state.stacks.is_empty() {
        ui.label("Aucun fichier compose mémorisé. Lancez un scan.");
        return actions;
    }

    // The section shares the tab's height with the Docker lists below, so it
    // takes a *share* of what is left rather than a fixed 260 px: that
    // constant wasted most of a maximized window and crowded a small one.
    // The clamp keeps at least two rows visible and never lets the stacks
    // push the containers off-screen.
    let max_height = (ui.available_height() * 0.45).clamp(180.0, 520.0);
    // Measured *before* entering the scroll area, like the Docker tables: the
    // vertical bar is reserved by hand because the width has to be known
    // up-front to decide how many cards go on a row.
    let bar = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
    let (columns, card_width) = grid_metrics(
        (ui.available_width() - bar).max(0.0),
        ui.spacing().item_spacing.x,
    );
    egui::ScrollArea::vertical()
        .id_salt("compose-stacks-scroll")
        .max_height(max_height)
        // `false` horizontally: a scroll area shrinks to its widest child by
        // default, which left every row stuck at its text width instead of
        // spanning the window.
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Rows laid out by hand rather than by `horizontal_wrapped`:
            // `ui.group` claims its space *before* its content is measured, so
            // the wrapping layout never learns that a card no longer fits and
            // clips it at the window edge instead of pushing it onto the next
            // line — which is exactly what the third stack did.
            for row in state.stacks.chunks(columns) {
                ui.horizontal_top(|ui| {
                    for stack in row {
                        render_stack(ui, stack, card_width, state, &mut actions);
                    }
                });
            }
        });

    actions
}

/// Default height of the log panel: about ten monospace lines, which is what
/// a `compose down` on a small stack prints in full.
const LOG_PANEL_DEFAULT_HEIGHT: f32 = 160.0;

/// One shared log for the whole tab — only one compose command runs at a time,
/// so it never needs more than one owner.
///
/// **Anchored, not inline.** It used to be drawn under the stack grid, inside
/// the tab's normal flow: appearing and disappearing shoved the Docker
/// sections below it up and down while the user was reading them, and the
/// panel's own content grew under the cursor. As a bottom panel it claims its
/// height from the window edge before anything else is laid out, so the rest
/// of the tab keeps its position for the whole run.
///
/// **Must be called before the tab's other content**, which is egui's rule for
/// panels: the parent `Ui`'s cursor is shrunk here, and whatever is drawn
/// afterwards gets what's left.
///
/// Returns [`StackAction::CloseLog`] when the user dismisses it.
pub fn render_log_panel(ui: &mut egui::Ui, state: &ComposeViewState<'_>) -> Vec<StackAction> {
    let mut actions = Vec::new();
    let Some(target) = state.log_target else {
        return actions;
    };
    if state.log.is_empty() {
        return actions;
    }
    let title = state
        .stacks
        .iter()
        .find(|stack| stack.file == target)
        .map(|stack| stack.project.as_str())
        .unwrap_or(target);
    egui::Panel::bottom("compose-log-panel")
        .resizable(true)
        .default_size(LOG_PANEL_DEFAULT_HEIGHT)
        // Never let a drag swallow the tab whole, nor shrink the panel past
        // its own header.
        .size_range(64.0..=(ui.available_height() * 0.6).max(64.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(format!("Journal — {title}"));
                if state.busy {
                    ui.spinner();
                }
                // Right-aligned so the close affordance sits where a panel's
                // does, not in the middle of the title.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Closing mid-run would only last until the next output
                    // line reopened the panel, and the lines printed in
                    // between would be lost with the buffer.
                    if ui
                        .add_enabled(!state.busy, egui::Button::new("✕"))
                        .on_hover_text("Fermer le journal")
                        .on_disabled_hover_text("commande en cours")
                        .clicked()
                    {
                        actions.push(StackAction::CloseLog);
                    }
                });
            });
            egui::ScrollArea::vertical()
                .id_salt("compose-log")
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in state.log {
                        ui.label(egui::RichText::new(line).monospace().small());
                    }
                });
        });
    actions
}

/// Narrowest a stack card may get before the grid drops a column.
///
/// Below this « Lancer » + « Arrêter » no longer share a line at the default
/// font, and the file path degrades to an ellipsis.
const MIN_CARD_WIDTH: f32 = 260.0;

/// How many cards fit on one row, and how wide each must be to fill it.
///
/// The width is *derived* from the window instead of fixed: a fixed one left a
/// ragged gutter down the right of a maximized window, which is the whole of
/// what « ça ne s'adapte pas à la largeur » was about. Cards therefore stretch
/// to share whatever the row has, and a column is dropped rather than let one
/// fall under [`MIN_CARD_WIDTH`].
fn grid_metrics(available: f32, spacing: f32) -> (usize, f32) {
    let columns = (((available + spacing) / (MIN_CARD_WIDTH + spacing)).floor() as usize).max(1);
    let width = (available - spacing * (columns - 1) as f32) / columns as f32;
    (columns, width.max(MIN_CARD_WIDTH))
}

fn render_stack(
    ui: &mut egui::Ui,
    stack: &StackEntry,
    card_width: f32,
    state: &ComposeViewState<'_>,
    actions: &mut Vec<StackAction>,
) {
    ui.group(|ui| {
        // `set_width` sizes the frame's *content*; its margins are added on
        // top, so without subtracting them back each card overshoots its share
        // of the row and the last one of the row gets clipped.
        let frame = egui::Frame::group(ui.style());
        let chrome = frame.inner_margin.sum().x + frame.outer_margin.sum().x;
        ui.set_width((card_width - chrome).max(0.0));
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                // Truncated, not wrapped: a long project name must not be
                // allowed to decide the card's height either.
                ui.add(egui::Label::new(egui::RichText::new(&stack.project).strong()).truncate());
                ui.colored_label(state_color(stack.state), stack.state.label());
                if state.log_target == Some(stack.file.as_str()) && state.busy {
                    ui.spinner();
                }
            });
            // The path is the card's identity when two projects share a name,
            // so it stays visible — truncated, with the full one on hover.
            ui.add(
                egui::Label::new(egui::RichText::new(&stack.file).small().color(MUTED_COLOR))
                    .truncate(),
            )
            .on_hover_text(&stack.file);

            if let Some(error) = &stack.error {
                ui.colored_label(ERROR_COLOR, error);
            }

            if !stack.services.is_empty() {
                let ports = declared_ports_text(stack);
                let summary = if ports.is_empty() {
                    format!("{} services · aucun port publié", stack.services.len())
                } else {
                    format!("{} services · ports {ports}", stack.services.len())
                };
                ui.label(egui::RichText::new(summary).small());
            }

            // A card born from a live container rather than from the scan:
            // its file was never read, so its services and declared ports are
            // simply unknown. Saying so beats a card that looks incomplete.
            if stack.services.is_empty()
                && !stack.runs.is_empty()
                && stack.state != StackState::Missing
            {
                ui.label(
                    egui::RichText::new("hors scan · services et ports inconnus")
                        .small()
                        .color(MUTED_COLOR),
                );
            }

            if stack.services.iter().any(|service| service.host_network) {
                ui.colored_label(
                    CONFLICT_COLOR,
                    egui::RichText::new("ports non comparables (network_mode: host)").small(),
                );
            }

            let label = format!("stack {}", stack.project);
            let hits = conflicts_for(state.conflicts, &label);
            if !hits.is_empty() {
                ui.colored_label(CONFLICT_COLOR, "⚠ conflit potentiel")
                    .on_hover_text(hits.join("\n"));
            }

            let enabled = !state.busy && !state.scanning;
            if stack.runs.is_empty() {
                render_actions(ui, stack, None, enabled, actions);
            } else {
                for run in &stack.runs {
                    ui.label(egui::RichText::new(run_summary(run)).small());
                    render_actions(ui, stack, Some(run.project.as_str()), enabled, actions);
                }
            }
        });
    });
}

/// « projet « lab » · 4/5 en marche · 1 en échec »
fn run_summary(run: &StackRun) -> String {
    format!(
        "projet « {} » · {}/{} en marche{}",
        run.project,
        run.running,
        run.total,
        if run.failing > 0 {
            format!(" · {} en échec", run.failing)
        } else {
            String::new()
        }
    )
}

fn render_actions(
    ui: &mut egui::Ui,
    stack: &StackEntry,
    project: Option<&str>,
    enabled: bool,
    actions: &mut Vec<StackAction>,
) {
    let target = StackTarget {
        file: stack.file.clone(),
        project: project.map(str::to_string),
    };
    let missing = stack.state == StackState::Missing;
    // Wrapped: four buttons do not fit on one 260 px line, and a card that
    // silently clipped « Oublier » would make a dead stack unremovable.
    ui.horizontal_wrapped(|ui| {
        // A potential port conflict never disables « Lancer »: the row warns
        // and names the colliding owner, the user decides — the conflicting
        // container may be exactly what they mean to replace, and `up -d`
        // fails loudly and harmlessly anyway when the port really is taken.
        if ui
            .add_enabled(enabled && !missing, egui::Button::new("Lancer"))
            .on_disabled_hover_text("fichier compose introuvable")
            .clicked()
        {
            actions.push(StackAction::Up(target.clone()));
        }
        if ui
            .add_enabled(enabled && project.is_some(), egui::Button::new("Arrêter"))
            .on_disabled_hover_text("aucune instance en cours")
            .clicked()
        {
            actions.push(StackAction::Stop(target.clone()));
        }
        if ui
            .add_enabled(enabled && project.is_some(), egui::Button::new("Détruire"))
            .on_disabled_hover_text("aucune instance en cours")
            .clicked()
        {
            actions.push(StackAction::Down(target.clone()));
        }
        if missing
            && ui
                .add_enabled(enabled, egui::Button::new("Oublier"))
                .clicked()
        {
            actions.push(StackAction::Forget(stack.file.clone()));
        }
    });
}

/// Every conflict worth showing on the Docker tab: the running containers'
/// real bindings, plus the declared ports of the stacks that are not up.
pub fn all_conflicts(container_owners: Vec<PortOwner>, stacks: &[StackEntry]) -> Vec<PortConflict> {
    let mut owners = container_owners;
    owners.extend(declared_owners(stacks));
    ports::find_conflicts(&owners)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(host_port: u16) -> PortBinding {
        PortBinding {
            host_ip: String::new(),
            host_port,
            container_port: host_port,
            protocol: "tcp".to_string(),
        }
    }

    fn service(name: &str, ports: &[u16]) -> StackService {
        StackService {
            name: name.to_string(),
            ports: ports.iter().copied().map(binding).collect(),
            host_network: false,
        }
    }

    fn stack(file: &str, project: &str, services: Vec<StackService>) -> StackEntry {
        StackEntry {
            file: file.to_string(),
            project: project.to_string(),
            services,
            runs: Vec::new(),
            state: StackState::Stopped,
            error: None,
        }
    }

    fn container(
        name: &str,
        state: ContainerState,
        project: Option<&str>,
        file: &str,
    ) -> ContainerEntry {
        ContainerEntry {
            id: format!("id-{name}"),
            name: name.to_string(),
            image: "img".to_string(),
            state,
            status: String::new(),
            rw_size: String::new(),
            ports: Vec::new(),
            last_activity: None,
            compose_project: project.map(str::to_string),
            compose_files: if project.is_some() {
                vec![file.to_string()]
            } else {
                Vec::new()
            },
            compose_service: None,
            declared_host_ports: std::collections::BTreeSet::new(),
            exit_code: None,
        }
    }

    // --- default_project_name ------------------------------------------------

    #[test]
    fn default_project_name_is_the_normalized_parent_directory() {
        assert_eq!(
            default_project_name("/home/tnn/Projets/SmartLockers/API_Mobile/docker-compose.yml"),
            "api_mobile"
        );
        // Compose drops everything outside `[a-z0-9_-]`, dots included.
        assert_eq!(
            default_project_name("/home/tnn/Projets/SmartLockers/qt.proxy/docker-compose.yml"),
            "qtproxy"
        );
        assert_eq!(default_project_name("docker-compose.yml"), "");
    }

    // --- is_failing / classify ----------------------------------------------

    #[test]
    fn an_exited_zero_one_shot_is_never_a_failure() {
        // The `db-init` case: four real stacks here would read `partielle`
        // forever without this rule.
        assert!(!is_failing(&ContainerState::Exited, Some(0)));
        // 128 + SIGKILL and 128 + SIGTERM: what `docker stop` leaves behind.
        assert!(!is_failing(&ContainerState::Exited, Some(137)));
        assert!(!is_failing(&ContainerState::Exited, Some(143)));
        assert!(is_failing(&ContainerState::Exited, Some(1)));
        // No exit code resolved at all: treated as a failure, because a
        // container that exited for an unknown reason is not evidence of health.
        assert!(is_failing(&ContainerState::Exited, None));
        assert!(is_failing(&ContainerState::Restarting, None));
        assert!(is_failing(&ContainerState::Dead, None));
        assert!(!is_failing(&ContainerState::Running, None));
        assert!(!is_failing(&ContainerState::Created, None));
    }

    #[test]
    fn classify_covers_running_partial_and_stopped() {
        assert_eq!(
            classify(&StackRun {
                project: "p".into(),
                running: 3,
                failing: 0,
                total: 4
            }),
            StackState::Running
        );
        assert_eq!(
            classify(&StackRun {
                project: "p".into(),
                running: 2,
                failing: 1,
                total: 4
            }),
            StackState::Partial
        );
        // `pilotphone`: nothing running, everything restarting. « arrêtée »
        // would be the exact opposite of the truth.
        assert_eq!(
            classify(&StackRun {
                project: "p".into(),
                running: 0,
                failing: 2,
                total: 4
            }),
            StackState::Partial
        );
        assert_eq!(
            classify(&StackRun {
                project: "p".into(),
                running: 0,
                failing: 0,
                total: 4
            }),
            StackState::Stopped
        );
    }

    #[test]
    fn a_partial_run_anywhere_wins_over_a_running_one() {
        let runs = vec![
            StackRun {
                project: "a".into(),
                running: 2,
                failing: 0,
                total: 2,
            },
            StackRun {
                project: "b".into(),
                running: 1,
                failing: 1,
                total: 2,
            },
        ];
        assert_eq!(aggregate_state(&runs), StackState::Partial);
        assert_eq!(aggregate_state(&[]), StackState::Stopped);
    }

    // --- link_runs -----------------------------------------------------------

    /// Every file exists — the default for a test that is not about the
    /// filesystem at all.
    fn all_present(_file: &str) -> bool {
        true
    }

    /// No file exists — the `suddenly` situation.
    fn none_present(_file: &str) -> bool {
        false
    }

    #[test]
    fn link_runs_matches_a_labelled_container_and_ignores_an_unlabelled_one() {
        let file = "/home/tnn/lab/docker-compose.yml";
        let stacks = vec![stack(
            file,
            "smartlockers-lab",
            vec![service("web", &[8080])],
        )];
        let containers = vec![
            container(
                "web",
                ContainerState::Running,
                Some("smartlockers-lab"),
                file,
            ),
            container(
                "db-init",
                ContainerState::Exited,
                Some("smartlockers-lab"),
                file,
            ),
            // The real `buildx_buildkit_mybuilder0`: no compose label at all.
            container("buildkit", ContainerState::Running, None, ""),
        ];
        let linked = link_runs(&stacks, &containers, &all_present);
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].runs.len(), 1);
        assert_eq!(linked[0].runs[0].total, 2);
        assert_eq!(linked[0].runs[0].running, 1);
        // The `Exited` container has no resolved exit code, so it counts as
        // failing — see `an_exited_zero_one_shot_is_never_a_failure`.
        assert_eq!(linked[0].runs[0].failing, 1);
        assert_eq!(linked[0].state, StackState::Partial);
    }

    #[test]
    fn an_exited_zero_init_container_leaves_the_stack_running() {
        let file = "/home/tnn/lab/docker-compose.yml";
        let stacks = vec![stack(file, "smartlockers-lab", Vec::new())];
        let mut init = container(
            "db-init",
            ContainerState::Exited,
            Some("smartlockers-lab"),
            file,
        );
        init.exit_code = Some(0);
        let containers = vec![
            container(
                "web",
                ContainerState::Running,
                Some("smartlockers-lab"),
                file,
            ),
            init,
        ];
        let linked = link_runs(&stacks, &containers, &all_present);
        assert_eq!(linked[0].state, StackState::Running);
        assert_eq!(linked[0].runs[0].failing, 0);
    }

    #[test]
    fn one_file_under_two_project_names_yields_two_runs() {
        let file = "/home/tnn/lab/docker-compose.yml";
        let stacks = vec![stack(file, "lab", Vec::new())];
        let containers = vec![
            container("web", ContainerState::Running, Some("lab"), file),
            container("web2", ContainerState::Running, Some("lab-bis"), file),
        ];
        let linked = link_runs(&stacks, &containers, &all_present);
        assert_eq!(linked[0].runs.len(), 2);
        let projects: Vec<&str> = linked[0]
            .runs
            .iter()
            .map(|run| run.project.as_str())
            .collect();
        assert_eq!(projects, vec!["lab", "lab-bis"]);
    }

    #[test]
    fn a_run_whose_compose_file_is_unknown_gets_a_missing_row() {
        // The real `suddenly` case: running from a `config_files` path that
        // does not exist on disk and is in no scan result.
        let ghost = "/home/tnn/Projets/MyApps/suddenly/docker-compose.yml";
        let linked = link_runs(
            &[],
            &[container(
                "web",
                ContainerState::Running,
                Some("suddenly"),
                ghost,
            )],
            &none_present,
        );
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].file, ghost);
        assert_eq!(linked[0].state, StackState::Missing);
        assert_eq!(linked[0].runs.len(), 1);
        assert!(linked[0].services.is_empty());
    }

    #[test]
    fn a_run_whose_file_is_unscanned_but_present_reads_its_real_state() {
        // The regression that shipped: before the first scan the memorized
        // list is empty, so *every* live stack took the branch above and the
        // UI announced « fichier introuvable » about files sitting on disk.
        let present = "/home/tnn/Projets/SmartLockers/lab/code/docker-compose.yml";
        let linked = link_runs(
            &[],
            &[
                container("web", ContainerState::Running, Some("lab"), present),
                container("db", ContainerState::Running, Some("lab"), present),
            ],
            &all_present,
        );
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].state, StackState::Running);
        assert!(
            linked[0].services.is_empty(),
            "unscanned: its services are genuinely unknown"
        );
    }

    #[test]
    fn a_missing_file_stays_missing_even_with_containers_running() {
        let file = "/gone/docker-compose.yml";
        let mut entry = stack(file, "gone", Vec::new());
        entry.state = StackState::Missing;
        let linked = link_runs(
            &[entry],
            &[container(
                "web",
                ContainerState::Running,
                Some("gone"),
                file,
            )],
            &none_present,
        );
        assert_eq!(linked.len(), 1, "no duplicate row for a known file");
        assert_eq!(linked[0].state, StackState::Missing);
        assert_eq!(linked[0].runs.len(), 1, "its run is still listed");
    }

    // --- declared_owners / conflicts ----------------------------------------

    #[test]
    fn a_stopped_stack_colliding_with_a_running_container_is_a_potential_conflict() {
        let stopped = stack(
            "/home/tnn/tasks/docker-compose.yml",
            "code",
            vec![service("mariadb", &[3309])],
        );
        let running_owner = PortOwner::new(
            "abc123",
            "api_mobile-mariadb-1",
            OwnerKind::RunningContainer,
            vec![binding(3309)],
        );
        let conflicts = all_conflicts(vec![running_owner], &[stopped]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].host_port, 3309);
        assert!(
            !conflicts[0].active,
            "a declared-vs-running collision is potential, not active"
        );
        assert!(conflicts[0].owners.iter().any(|o| o == "stack code"));
    }

    #[test]
    fn a_running_stack_declares_no_owner_its_containers_already_do() {
        let mut up = stack("/a/docker-compose.yml", "a", vec![service("web", &[8080])]);
        up.state = StackState::Running;
        assert!(
            declared_owners(&[up]).is_empty(),
            "counting a running stack's declared ports would make it conflict with itself"
        );
    }

    #[test]
    fn a_missing_stack_declares_no_owner() {
        let mut gone = stack("/a/docker-compose.yml", "a", vec![service("web", &[8080])]);
        gone.state = StackState::Missing;
        assert!(declared_owners(&[gone]).is_empty());
    }

    #[test]
    fn two_stopped_stacks_sharing_a_port_flag_each_other() {
        // The user's real case: `tasks` and `API_Mobile` both declare 3309.
        let a = stack(
            "/tasks/docker-compose.yml",
            "code",
            vec![service("mariadb", &[3309])],
        );
        let b = stack(
            "/API_Mobile/docker-compose.yml",
            "api_mobile",
            vec![service("mariadb", &[3309])],
        );
        let conflicts = all_conflicts(Vec::new(), &[a, b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].owners.len(), 2);
        assert!(!conflicts[0].active);
    }

    // --- argv builders -------------------------------------------------------

    #[test]
    fn argv_puts_the_project_flag_before_the_subcommand() {
        let target = StackTarget {
            file: "/a/docker-compose.yml".to_string(),
            project: Some("lab".to_string()),
        };
        assert_eq!(
            stop_args(&target),
            vec![
                "compose",
                "-f",
                "/a/docker-compose.yml",
                "-p",
                "lab",
                "stop"
            ]
        );
    }

    #[test]
    fn argv_omits_the_project_flag_when_none_is_given() {
        let target = StackTarget {
            file: "/a/docker-compose.yml".to_string(),
            project: None,
        };
        assert_eq!(
            up_args(&target),
            vec!["compose", "-f", "/a/docker-compose.yml", "up", "-d"]
        );
    }

    #[test]
    fn down_never_carries_the_volume_flag() {
        for project in [None, Some("lab".to_string())] {
            let target = StackTarget {
                file: "/a/docker-compose.yml".to_string(),
                project,
            };
            let args = down_args(&target);
            assert!(
                !args.iter().any(|arg| arg == "-v" || arg == "--volumes"),
                "down must never delete volumes: {args:?}"
            );
            assert_eq!(args.last().map(String::as_str), Some("down"));
        }
    }

    // --- rendering -----------------------------------------------------------

    fn harness_state<'a>(
        stacks: &'a [StackEntry],
        conflicts: &'a [PortConflict],
    ) -> ComposeViewState<'a> {
        ComposeViewState {
            stacks,
            conflicts,
            plugin_available: true,
            scanning: false,
            busy: false,
            log: &[],
            log_target: None,
            scan_warning: None,
        }
    }

    #[test]
    fn render_without_the_plugin_says_so_and_offers_no_scan() {
        let state = ComposeViewState {
            plugin_available: false,
            ..harness_state(&[], &[])
        };
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            render(ui, &state);
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        assert!(harness
            .query_by_label("plugin « docker compose » introuvable")
            .is_some());
    }

    #[test]
    fn render_shows_the_state_label_and_the_declared_ports() {
        let stacks = vec![stack(
            "/home/tnn/lab/docker-compose.yml",
            "smartlockers-lab",
            vec![service("web", &[8080]), service("mariadb", &[3308])],
        )];
        let state = harness_state(&stacks, &[]);
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            render(ui, &state);
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        assert!(harness.query_by_label("arrêtée").is_some());
        assert!(harness
            .query_by_label("2 services · ports 8080/tcp, 3308/tcp")
            .is_some());
    }

    #[test]
    fn render_flags_a_host_network_service_as_incomparable() {
        let mut host_service = service("proxy", &[]);
        host_service.host_network = true;
        let stacks = vec![stack(
            "/qt/docker-compose.yml",
            "qtproxy",
            vec![host_service],
        )];
        let state = harness_state(&stacks, &[]);
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            render(ui, &state);
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        assert!(harness
            .query_by_label("ports non comparables (network_mode: host)")
            .is_some());
    }

    #[test]
    fn clicking_scan_emits_the_scan_action() {
        let state = harness_state(&[], &[]);
        let mut actions = Vec::new();
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            // Extend, never assign: `Harness::run` may lay out several
            // frames, and the frames after the click would otherwise wipe the
            // action the click just emitted.
            actions.extend(render(ui, &state));
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        harness.get_by_label("Scanner").click();
        harness.run();
        // Releases the closure's mutable borrow of `actions` so the assert
        // below can read it.
        drop(harness);
        assert_eq!(actions, vec![StackAction::Scan]);
    }

    // --- log panel -----------------------------------------------------------

    #[test]
    fn the_log_panel_stays_out_of_the_way_until_there_is_something_to_show() {
        let stacks = vec![stack("/a/docker-compose.yml", "a", Vec::new())];
        // No target, no lines: nothing is anchored and the tab keeps its full
        // height. This is the state the tab spends most of its life in.
        let state = harness_state(&stacks, &[]);
        let mut actions = Vec::new();
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            actions.extend(render_log_panel(ui, &state));
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        assert!(harness.query_by_label("Journal — a").is_none());
        drop(harness);
        assert!(actions.is_empty());

        // A target whose command printed nothing yet is still not worth a
        // panel — an empty anchored strip is just lost height.
        let state = ComposeViewState {
            log_target: Some("/a/docker-compose.yml"),
            ..harness_state(&stacks, &[])
        };
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            render_log_panel(ui, &state);
        });
        harness.run();
        assert!(harness.query_by_label("Journal — a").is_none());
    }

    #[test]
    fn the_log_panel_titles_itself_with_the_project_it_belongs_to() {
        let stacks = vec![
            stack("/a/docker-compose.yml", "a", Vec::new()),
            stack("/b/docker-compose.yml", "pilotphone", Vec::new()),
        ];
        let lines = vec!["Container pilotphone-1 Removing".to_string()];
        let state = ComposeViewState {
            log: &lines,
            log_target: Some("/b/docker-compose.yml"),
            ..harness_state(&stacks, &[])
        };
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            render_log_panel(ui, &state);
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        assert!(harness.query_by_label("Journal — pilotphone").is_some());
        assert!(harness
            .query_by_label("Container pilotphone-1 Removing")
            .is_some());
    }

    #[test]
    fn the_log_panel_closes_on_demand_but_never_mid_command() {
        let stacks = vec![stack("/a/docker-compose.yml", "a", Vec::new())];
        let lines = vec!["Container a-1 Removing".to_string()];

        // While the command runs the close button is there but refuses: the
        // lines printed after a close would be lost with the buffer.
        let running = ComposeViewState {
            log: &lines,
            log_target: Some("/a/docker-compose.yml"),
            busy: true,
            ..harness_state(&stacks, &[])
        };
        let mut actions = Vec::new();
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            // Extend, never assign: `Harness::run` may lay out several
            // frames, and the frames after the click would otherwise wipe the
            // action the click just emitted.
            actions.extend(render_log_panel(ui, &running));
        });
        // `run_steps`, not `run`: the busy spinner requests a repaint every
        // frame, so `run`'s settle loop never converges and trips its own
        // step ceiling. The panel is anchored, so unlike a modal its rect is
        // final on the first frame and a queued click lands where it looks.
        harness.run_steps(2);
        use egui_kittest::kittest::Queryable;
        harness.get_by_label("✕").click();
        harness.run_steps(2);
        drop(harness);
        assert!(
            actions.is_empty(),
            "a disabled close button must emit nothing: {actions:?}"
        );

        // Once it has settled, the same click dismisses the panel.
        let settled = ComposeViewState {
            log: &lines,
            log_target: Some("/a/docker-compose.yml"),
            ..harness_state(&stacks, &[])
        };
        let mut actions = Vec::new();
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            actions.extend(render_log_panel(ui, &settled));
        });
        harness.run();
        harness.get_by_label("✕").click();
        harness.run();
        drop(harness);
        assert_eq!(actions, vec![StackAction::CloseLog]);
    }

    #[test]
    fn the_grid_drops_a_column_rather_than_squeeze_a_card() {
        let spacing = 8.0;
        // 721 px — the window of the screenshot that reported the bug: two
        // cards fit, the third one used to be clipped at the edge.
        let (columns, width) = grid_metrics(721.0, spacing);
        assert_eq!(columns, 2);
        assert!((width * 2.0 + spacing - 721.0).abs() < 0.01, "{width}");
        assert!(width >= MIN_CARD_WIDTH);

        // Maximized: more columns, and still no gutter left over.
        let (columns, width) = grid_metrics(1900.0, spacing);
        assert_eq!(columns, 7);
        assert!(
            (width * 7.0 + spacing * 6.0 - 1900.0).abs() < 0.01,
            "{width}"
        );

        // Narrower than one card: one column, never zero, never a division by
        // zero further down.
        let (columns, width) = grid_metrics(120.0, spacing);
        assert_eq!(columns, 1);
        assert_eq!(width, MIN_CARD_WIDTH);
    }

    #[test]
    fn a_missing_row_offers_forget_and_refuses_to_launch() {
        let mut gone = stack("/gone/docker-compose.yml", "gone", Vec::new());
        gone.state = StackState::Missing;
        let stacks = vec![gone];
        let state = harness_state(&stacks, &[]);
        let mut actions = Vec::new();
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            // Extend, never assign: `Harness::run` may lay out several
            // frames, and the frames after the click would otherwise wipe the
            // action the click just emitted.
            actions.extend(render(ui, &state));
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        assert!(harness.query_by_label("fichier introuvable").is_some());
        harness.get_by_label("Oublier").click();
        harness.run();
        drop(harness);
        assert_eq!(
            actions,
            vec![StackAction::Forget("/gone/docker-compose.yml".to_string())]
        );
    }

    #[test]
    fn a_run_row_targets_its_own_project_name() {
        let file = "/home/tnn/lab/docker-compose.yml";
        let mut entry = stack(file, "lab", Vec::new());
        entry.state = StackState::Running;
        entry.runs = vec![StackRun {
            project: "lab-bis".to_string(),
            running: 2,
            failing: 0,
            total: 2,
        }];
        let stacks = vec![entry];
        let state = harness_state(&stacks, &[]);
        let mut actions = Vec::new();
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            // Extend, never assign: `Harness::run` may lay out several
            // frames, and the frames after the click would otherwise wipe the
            // action the click just emitted.
            actions.extend(render(ui, &state));
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        harness.get_by_label("Arrêter").click();
        harness.run();
        drop(harness);
        assert_eq!(
            actions,
            vec![StackAction::Stop(StackTarget {
                file: file.to_string(),
                project: Some("lab-bis".to_string()),
            })]
        );
    }

    #[test]
    fn render_shows_the_potential_conflict_badge() {
        let stacks = vec![stack(
            "/tasks/docker-compose.yml",
            "code",
            vec![service("mariadb", &[3309])],
        )];
        let conflicts = all_conflicts(
            vec![PortOwner::new(
                "abc",
                "api_mobile-mariadb-1",
                OwnerKind::RunningContainer,
                vec![binding(3309)],
            )],
            &stacks,
        );
        let state = harness_state(&stacks, &conflicts);
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            render(ui, &state);
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        assert!(harness.query_by_label("⚠ conflit potentiel").is_some());
    }

    #[test]
    fn a_busy_frame_disables_every_action() {
        let stacks = vec![stack("/a/docker-compose.yml", "a", Vec::new())];
        let state = ComposeViewState {
            busy: true,
            ..harness_state(&stacks, &[])
        };
        let mut actions = Vec::new();
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            // Extend, never assign: `Harness::run` may lay out several
            // frames, and the frames after the click would otherwise wipe the
            // action the click just emitted.
            actions.extend(render(ui, &state));
        });
        harness.run();
        use egui_kittest::kittest::Queryable;
        harness.get_by_label("Lancer").click();
        harness.run();
        drop(harness);
        assert!(actions.is_empty(), "a disabled button emits nothing");
    }
}
