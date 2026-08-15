//! `eframe::App` — Part 2 Phase 2 card grid + favorites + category CRUD.
//!
//! Ports the user-facing behavior of the legacy Win32 host (`src/ui/app.rs`,
//! `src/ui/card.rs` — `cfg(windows)`-only, kept for reference until Phase 4
//! deletes them) onto `egui`. See the interaction checklist below (also
//! appended to the Part 2 plan's `## Amendments` section) for what ported,
//! what deferred to Phase 3, and what was flagged as a plan gap.
//!
//! ```text
//! Interaction checklist (enumerated from app.rs/card.rs before any deletion)
//! ─────────────────────────────────────────────────────────────────────────
//! Item                                   | Disposition (Phase 2)
//! ---------------------------------------|-----------------------------------
//! Card grid, flat mode (favorites only)  | PORTED — build_display_groups()
//! Card grid, grouped mode (by category,  | PORTED — build_display_groups()
//!   incl. synthetic "Sans catégorie")    |
//! Favorite toggle                        | PORTED (NEW UI — app.rs never
//!                                         |   exposed this; storage::
//!                                         |   toggle_favorite existed
//!                                         |   backend-only until now)
//! Category add / rename / remove         | PORTED (NEW UI — same as above)
//! Icon rendering (bitmap or emoji)        | PORTED — routed through
//!                                         |   IconBackend/EguiIconBackend,
//!                                         |   not a hardcoded test icon
//! Card hover/pressed/focus visual states | DROPPED (MVP) — egui's default
//!                                         |   button hover/press styling is
//!                                         |   used instead of porting the
//!                                         |   Fluent color-constant recipe
//!                                         |   from card.rs pixel-for-pixel
//! Direct command launch (click → run)    | STILL DEFERRED — Phase 3 built
//!                                         |   the Terminal view (see below)
//!                                         |   but wiring a card click to
//!                                         |   auto-launch into it was not
//!                                         |   part of Phase 3's explicit
//!                                         |   scope; left for a future phase
//! Variant-group popup menu (click on a   | STILL DEFERRED — depends on the
//!   grouped action → choose a variant)   |   direct-launch wiring above
//! Nav bar (Actions / Terminal /          | PORTED (Phase 3) — see
//!   Automations view switch)             |   `ActiveView`/nav row in
//!                                         |   `ui_content`
//! Native menu bar (Fichier/Affichage/    | STILL DEFERRED — the nav row
//!   Aide: reload config, quit, view      |   above covers the view-switch
//!   switch, About)                       |   part; Reload/Quit menu items
//!                                         |   were not in Phase 3's explicit
//!                                         |   task list
//! Terminal view (PowerShell output       | PORTED (Phase 3) — see
//!   streaming, VecDeque line buffer)     |   `render_terminal_view` /
//!                                         |   `src/ui/terminal_view.rs`
//!                                         |   (cross-platform, plain
//!                                         |   `std::process::Command`
//!                                         |   rather than the Windows-only
//!                                         |   `windows::process` pipeline)
//! Automations view (PowerShell           | PLAN GAP RESOLVED (Phase 3,
//!   Get-ScheduledTask/Stop/Enable/Set)   |   folded in per plan Amendment
//!                                         |   🤖 2026-08-05) — shell built:
//!                                         |   see `render_automations_view` /
//!                                         |   `src/ui/automations_view.rs`.
//!                                         |   Windows fetch (Get-ScheduledTask)
//!                                         |   implemented; Linux fetch is a
//!                                         |   documented `Ok(vec![])` stub —
//!                                         |   Part 3 Phase 2 wires a real
//!                                         |   systemd data source in later.
//!                                         |   Stop/Enable/Set actions are
//!                                         |   NOT in this shell (read-only
//!                                         |   list only); out of the gap's
//!                                         |   scope as folded into Phase 3.
//! MessageBoxW-based dialogs (About, ...) | PORTED (Phase 3) — see
//!                                         |   `src/ui/dialogs.rs`
//!                                         |   (`dialogs::{info,warn,confirm}`)
//! Right-click context menus              | N/A — none exist in app.rs/card.rs
//! Custom keyboard shortcuts              | N/A — `Command.shortcut` field is
//!                                         |   unused dead data in app.rs;
//!                                         |   no accelerator table, no
//!                                         |   WM_KEYDOWN handling found
//! Search / filter                        | N/A — no such feature in app.rs
//! ```

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use eframe::egui;

use crate::applications::{
    self, RecommendationReport, ReportEvent, SystemProcessProvider, UsageService,
};
use crate::icons::backend::IconBackend;
use crate::icons::egui_backend::EguiIconBackend;
#[cfg(not(target_os = "linux"))]
use crate::icons::resolve_icon;
use crate::icons::{decode_resize_file, icons_dirs, IconResolution};
use crate::storage::{self, CommandResolution, Config, MachineCommands, StorageError};
use crate::ui::applications_view::{self, ApplicationFilters};
use crate::ui::automations_view::{self, AutomationRow};
use crate::ui::dialogs::{self, DialogKind, DialogOutcome};
use crate::ui::terminal_view::{self, TerminalEvent};

/// A resolved, cached representation of a command/category `icon` field,
/// ready to draw without re-decoding every frame.
#[derive(Clone)]
enum IconVisual {
    Texture(egui::TextureHandle),
    Emoji(String),
}

/// Flattened, owned per-card data used purely for rendering — decoupled
/// from `storage::Config` so the grid-building step doesn't hold a live
/// borrow of `self.config` while `self` is mutated by click handlers.
#[derive(Clone, Debug, PartialEq)]
struct CardData {
    command_id: String,
    name: String,
    icon: String,
    is_favorite: bool,
    /// `false` only for a `machine_specific: true` command with no matching
    /// entry in the per-machine mapping for the current machine (see
    /// `storage::resolve_command`'s `CommandResolution::Unconfigured`).
    /// Always `true` for non-machine-specific commands, so existing cards
    /// render exactly as before this lot.
    is_configured: bool,
    /// `Some` iff `is_configured` is `false` — an inline message naming the
    /// current machine id and the mapping file path, shown on the disabled
    /// card.
    disabled_message: Option<String>,
    /// The resolved shell command to run on click — empty when
    /// `is_configured` is `false` (an unconfigured card is never clickable,
    /// so there is nothing meaningful to launch).
    command: String,
    /// `Some` for a variant-grouped card (mirrors `storage::Command::group_name`
    /// of its variants) — used as the card's displayed title instead of
    /// `name`, and as the key into `EguiApp::selected_variant`. `None` for a
    /// simple (non-grouped) card.
    group_name: Option<String>,
    /// Empty for a simple card. For a grouped card, every variant sharing
    /// the `variant_group`, in `config.commands` order — including
    /// non-favorite ones in favorites mode, so a card made visible by one
    /// favorite variant still offers its siblings in the dropdown. When
    /// non-empty, the root `command`/`is_configured`/`disabled_message`
    /// fields above are meaningless — read them from the currently selected
    /// `VariantCardData` instead.
    variants: Vec<VariantCardData>,
}

/// One entry in a variant-grouped card's dropdown — the per-variant
/// counterpart of the resolution-derived fields on [`CardData`].
#[derive(Clone, Debug, PartialEq)]
struct VariantCardData {
    command_id: String,
    /// `storage::Command::variant_label`, falling back to the command's
    /// `name` if unset.
    label: String,
    is_favorite: bool,
    is_configured: bool,
    disabled_message: Option<String>,
    command: String,
}

#[derive(Clone, Debug, PartialEq)]
struct DisplayGroup {
    /// `None` in flat mode (no header rendered). `Some("Sans catégorie")` is
    /// the synthetic bucket for orphaned commands in grouped mode.
    header: Option<String>,
    cards: Vec<CardData>,
}

/// Build a [`CardData`]'s resolution-derived fields (`is_configured` +
/// `disabled_message`) from a [`storage::resolve_command`] outcome.
/// Non-machine-specific commands always resolve to `Resolved`, so this is
/// unconditionally `(true, None)` for them — the existing-card no-regression
/// guarantee this lot's acceptance criteria call for.
fn resolution_fields(
    command: &storage::Command,
    overrides: &MachineCommands,
    machine_id: &str,
) -> (bool, Option<String>, String) {
    match storage::resolve_command(command, overrides, machine_id) {
        CommandResolution::Resolved(resolved) => (true, None, resolved),
        CommandResolution::Unconfigured {
            command_id,
            machine_id,
        } => {
            let mapping_path = crate::platform::machine_commands_path();
            (
                false,
                Some(format!(
                    "Non configuré pour la machine « {machine_id} » — ajoutez '{command_id}' dans {}",
                    mapping_path.display()
                )),
                String::new(),
            )
        }
    }
}

/// Pure click-handling guard for a card body click: a card can only launch
/// if it is configured and no other card launch is already in flight (one
/// card launch at a time — see the `action_running` field doc). Kept as a
/// free function, independent of `EguiApp`, so the concurrency guard is
/// directly unit-testable without building a full app/harness.
fn can_launch_card(is_configured: bool, action_running: &Option<String>) -> bool {
    is_configured && action_running.is_none()
}

/// Groups commands for [`partition_by_variant_group`]: every command with a
/// `variant_group` merges with its siblings sharing that same group id;
/// every command without one gets its own bucket keyed by its own `id`, so
/// distinct ungrouped commands never collapse into each other (all sharing
/// the same `None` `variant_group` would otherwise hash identically).
#[derive(Clone, PartialEq, Eq, Hash)]
enum PartitionKey {
    Single(String),
    Group(String),
}

/// Partitions `commands` into [`CardData`]s: commands sharing a
/// `variant_group` consolidate into a single card carrying all of them as
/// `variants` (in `commands` order); commands without one keep today's
/// one-command-one-card behavior. `is_visible` additionally gates whether a
/// candidate card is emitted at all — e.g. the favorites branch only emits a
/// grouped card if at least one of its variants passes `is_visible`, but
/// every variant is still attached to `variants` regardless, so a favorited
/// variant's card still shows its non-favorited siblings in the dropdown.
fn partition_by_variant_group(
    commands: &[&storage::Command],
    overrides: &MachineCommands,
    machine_id: &str,
    is_visible: impl Fn(&storage::Command) -> bool,
) -> Vec<CardData> {
    let mut order: Vec<PartitionKey> = Vec::new();
    let mut buckets: HashMap<PartitionKey, Vec<&storage::Command>> = HashMap::new();
    for &c in commands {
        let key = match &c.variant_group {
            Some(g) => PartitionKey::Group(g.clone()),
            None => PartitionKey::Single(c.id.clone()),
        };
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().push(c);
    }

    order
        .into_iter()
        .filter_map(|key| {
            let members = buckets.get(&key)?;
            if !members.iter().copied().any(&is_visible) {
                return None;
            }
            let first = members[0];

            match key {
                PartitionKey::Single(_) => {
                    let (is_configured, disabled_message, command) =
                        resolution_fields(first, overrides, machine_id);
                    Some(CardData {
                        command_id: first.id.clone(),
                        name: first.name.clone(),
                        icon: first.icon.clone(),
                        is_favorite: first.is_favorite,
                        is_configured,
                        disabled_message,
                        command,
                        group_name: None,
                        variants: Vec::new(),
                    })
                }
                PartitionKey::Group(_) => {
                    let variants: Vec<VariantCardData> = members
                        .iter()
                        .copied()
                        .map(|c| {
                            let (is_configured, disabled_message, command) =
                                resolution_fields(c, overrides, machine_id);
                            VariantCardData {
                                command_id: c.id.clone(),
                                label: c.variant_label.clone().unwrap_or_else(|| c.name.clone()),
                                is_favorite: c.is_favorite,
                                is_configured,
                                disabled_message,
                                command,
                            }
                        })
                        .collect();
                    let group_name = first.group_name.clone().unwrap_or_else(|| first.name.clone());
                    Some(CardData {
                        command_id: first.id.clone(),
                        name: group_name.clone(),
                        icon: first.icon.clone(),
                        is_favorite: members.iter().any(|c| c.is_favorite),
                        is_configured: false,
                        disabled_message: None,
                        command: String::new(),
                        group_name: Some(group_name),
                        variants,
                    })
                }
            }
        })
        .collect()
}

/// Pure grouping/flattening step — mirrors `app.rs`'s
/// `build_from_config`'s branch on `config.default_settings.show_categories`
/// (grouped path uses `group_commands_by_category`; flat path filters to
/// favorites only). Takes an owned `&Config` and returns fully owned data so
/// callers never hold a borrow of `config` while later mutating it. Each
/// card's `is_configured`/`disabled_message` is computed via
/// `storage::resolve_command` against `overrides`/`machine_id` (Part 3).
/// Commands sharing a `variant_group` are consolidated by
/// `partition_by_variant_group` in both branches below.
fn build_display_groups(
    config: &Config,
    overrides: &MachineCommands,
    machine_id: &str,
) -> Vec<DisplayGroup> {
    if config.default_settings.show_categories {
        storage::group_commands_by_category(config)
            .into_iter()
            .map(|group| {
                let header = match group.category {
                    Some(cat) => cat.name.clone(),
                    None => "Sans catégorie".to_string(),
                };
                let cards =
                    partition_by_variant_group(&group.commands, overrides, machine_id, |_| true);
                DisplayGroup {
                    header: Some(header),
                    cards,
                }
            })
            .collect()
    } else {
        let commands: Vec<&storage::Command> = config.commands.iter().collect();
        let cards = partition_by_variant_group(&commands, overrides, machine_id, |c| {
            c.is_favorite
        });
        vec![DisplayGroup {
            header: None,
            cards,
        }]
    }
}

/// Same built-in fallback used by `app.rs::UiHost::new` when `storage::load`
/// fails (kept in parity so a broken/missing config.json still boots into a
/// usable window instead of an empty grid).
fn fallback_config() -> Config {
    use storage::{Command, Settings};
    Config {
        version: "0.1.0".to_string(),
        default_settings: Settings {
            show_categories: true,
            icon_size: 56,
            theme: "light".to_string(),
            launch_at_startup: false,
            show_descriptions: true,
        },
        categories: Vec::new(),
        commands: vec![
            Command {
                id: "notepad".into(),
                name: "Bloc-notes".into(),
                command: "notepad.exe".into(),
                category: "system".into(),
                icon: "📝".into(),
                is_favorite: true,
                shortcut: None,
                variant_group: None,
                group_name: None,
                variant_label: None,
                machine_specific: false,
            },
            Command {
                id: "cmd".into(),
                name: "Invite de commandes".into(),
                command: "cmd.exe".into(),
                category: "system".into(),
                icon: "💻".into(),
                is_favorite: true,
                shortcut: None,
                variant_group: None,
                group_name: None,
                variant_label: None,
                machine_specific: false,
            },
            Command {
                id: "ipconfig".into(),
                name: "Adresse IP".into(),
                command: "ipconfig /all".into(),
                category: "system".into(),
                icon: "🌐".into(),
                is_favorite: true,
                shortcut: None,
                variant_group: None,
                group_name: None,
                variant_label: None,
                machine_specific: false,
            },
        ],
        // Suppresses an "unused struct field" concern for readers: this
        // mirrors app.rs's fallback exactly — `categories` stays empty, so
        // all three fallback commands land in the synthetic "Sans
        // catégorie" bucket when grouped.
    }
}

/// Status bar message (success/error feedback for the last CRUD/toggle
/// action), analogous to `app.rs`'s `log::info!`/`log::warn!` calls but
/// surfaced in the UI since there is no console visible to an end user.
struct StatusMessage {
    text: String,
    is_error: bool,
}

/// Category-creation form scratch buffers (id/name/icon text fields).
#[derive(Default)]
struct CategoryForm {
    id: String,
    name: String,
    icon: String,
}

enum CategoryAction {
    Add,
    Rename { id: String, new_name: String },
    Remove { id: String },
}

/// Which top-level view the nav row has selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ActiveView {
    #[default]
    Actions,
    Terminal,
    Automations,
    Applications,
    Preferences,
}

/// An action deferred behind a confirm dialog — applied by
/// [`EguiApp::resolve_pending_action`] only once the user picks "Oui".
enum PendingAction {
    RemoveCategory(String),
}

/// The dialog currently blocking the rest of the UI (see `ui_content`'s
/// short-circuit and `src/ui/dialogs.rs`'s module docs on why this has to be
/// checked first, before any other widget is built for the frame).
struct ActiveDialog {
    kind: DialogKind,
    /// `Some` for a destructive action awaiting confirmation; `None` for a
    /// plain info/warn dialog with nothing to do on "OK".
    on_confirm: Option<PendingAction>,
}

pub struct EguiApp {
    config: Config,
    /// `None` uses `storage::save`'s default resolution
    /// (`platform::config_path()`); `Some(path)` overrides it — used by the
    /// `#[cfg(test)]` harness constructor so interaction tests never touch
    /// the real user config file.
    config_path: Option<PathBuf>,
    icon_backend: EguiIconBackend,
    icon_cache: HashMap<String, IconVisual>,
    category_form: CategoryForm,
    rename_buffers: HashMap<String, String>,
    status: Option<StatusMessage>,
    active_view: ActiveView,
    /// `Some` blocks the rest of the UI for the frame — see `ui_content`.
    active_dialog: Option<ActiveDialog>,
    terminal_input: String,
    terminal_lines: VecDeque<String>,
    terminal_rx: Option<Receiver<TerminalEvent>>,
    terminal_running: bool,
    /// Events for a command launched by clicking an Actions card — a
    /// dedicated slot separate from `terminal_rx`/`terminal_running` so a
    /// card launch never interferes with (or is interfered with by) a
    /// command running in the Terminal view.
    action_rx: Option<Receiver<TerminalEvent>>,
    /// The `command_id` of the card currently launching/running, if any —
    /// used both to gate the concurrency guard (`can_launch_card`) and to
    /// name the command in the status message once it settles.
    action_running: Option<String>,
    /// Session-only, never persisted to `config.json`: `variant_group` →
    /// currently selected `command_id` for a grouped card's dropdown.
    /// Lazily defaulted to the group's first variant on first render if
    /// absent.
    selected_variant: HashMap<String, String>,
    /// `None` until the Automations view has fetched at least once.
    automations: Option<Result<Vec<AutomationRow>, String>>,
    application_report: Option<RecommendationReport>,
    application_error: Option<String>,
    application_loading: bool,
    application_generation: u64,
    application_tx: Sender<ReportEvent>,
    application_rx: Receiver<ReportEvent>,
    application_filters: ApplicationFilters,
    application_selected: Option<String>,
    usage_service: Option<UsageService>,
    report_spawning_enabled: bool,
    /// Per-machine command overrides, loaded once at startup (Part 3) via
    /// `storage::load_machine_commands_from(platform::machine_commands_path())`.
    /// A missing file or load error both fall back to an empty map — this
    /// must never crash the app on startup.
    machine_commands: MachineCommands,
    /// This machine's id, resolved once at startup via `platform::machine_id()`
    /// and reused on every frame instead of re-resolving per `build_display_groups` call.
    machine_id: String,
}

impl EguiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let config = storage::load().unwrap_or_else(|err| {
            log::warn!("storage::load failed ({err}); falling back to built-in defaults");
            fallback_config()
        });
        // A missing file or a load error both fall back to an empty mapping
        // — this must never crash the app on startup (Part 3 acceptance
        // criteria).
        let machine_commands =
            storage::load_machine_commands_from(&crate::platform::machine_commands_path())
                .unwrap_or_else(|err| {
                    log::warn!(
                        "storage::load_machine_commands_from failed ({err}); falling back to an empty per-machine mapping"
                    );
                    MachineCommands::default()
                });
        let machine_id = crate::platform::machine_id();
        let usage_service = UsageService::start(
            crate::platform::application_usage_path(),
            Arc::new(SystemProcessProvider),
        );
        let mut app = Self::from_parts(
            config,
            None,
            EguiIconBackend::new(cc.egui_ctx.clone()),
            machine_commands,
            machine_id,
            Some(usage_service),
            true,
        );
        app.refresh_applications();
        app
    }

    /// Test-only constructor: builds an `EguiApp` from an explicit `Config`
    /// and a temp-file `config_path` (see `config_path` field doc), using a
    /// headless `egui::Context` for the icon backend — `egui::Context`
    /// texture handles aren't tied to any particular render surface, so a
    /// context independent from `egui_kittest::Harness`'s own context is
    /// fine for interaction-only (non-pixel-snapshot) tests. Uses a fixed,
    /// deterministic empty `MachineCommands`/machine id rather than reading
    /// the real `platform::machine_commands_path()` file, so the test suite
    /// never depends on whatever happens to be on the machine it runs on;
    /// none of the existing harness tests use `machine_specific: true`
    /// commands, so this has no effect on them.
    #[cfg(test)]
    fn new_for_test(config: Config, config_path: PathBuf) -> Self {
        Self::from_parts(
            config,
            Some(config_path),
            EguiIconBackend::new(egui::Context::default()),
            MachineCommands::default(),
            "test-machine".to_string(),
            None,
            false,
        )
    }

    fn from_parts(
        config: Config,
        config_path: Option<PathBuf>,
        icon_backend: EguiIconBackend,
        machine_commands: MachineCommands,
        machine_id: String,
        usage_service: Option<UsageService>,
        report_spawning_enabled: bool,
    ) -> Self {
        let (application_tx, application_rx) = std::sync::mpsc::channel();
        Self {
            config,
            config_path,
            icon_backend,
            icon_cache: HashMap::new(),
            category_form: CategoryForm::default(),
            rename_buffers: HashMap::new(),
            status: None,
            active_view: ActiveView::default(),
            active_dialog: None,
            terminal_input: String::new(),
            terminal_lines: VecDeque::new(),
            terminal_rx: None,
            terminal_running: false,
            action_rx: None,
            action_running: None,
            selected_variant: HashMap::new(),
            automations: None,
            application_report: None,
            application_error: None,
            application_loading: false,
            application_generation: 0,
            application_tx,
            application_rx,
            application_filters: ApplicationFilters::default(),
            application_selected: None,
            usage_service,
            report_spawning_enabled,
            machine_commands,
            machine_id,
        }
    }

    fn set_status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = Some(StatusMessage {
            text: text.into(),
            is_error,
        });
    }

    fn refresh_applications(&mut self) {
        self.application_generation = self.application_generation.saturating_add(1);
        self.application_loading = true;
        if self.report_spawning_enabled {
            applications::spawn_report(
                self.application_generation,
                crate::platform::application_usage_path(),
                self.application_tx.clone(),
            );
        }
    }

    fn drain_application_events(&mut self) {
        let mut events = Vec::new();
        while let Ok(event) = self.application_rx.try_recv() {
            events.push(event);
        }
        for event in events {
            if event.generation != self.application_generation {
                continue;
            }
            self.application_loading = false;
            match event.result {
                Ok(report) => {
                    if let Some(service) = &self.usage_service {
                        if let Err(error) = service.replace_targets(report.usage_targets()) {
                            log::warn!("application usage targets unavailable: {error}");
                        }
                    }
                    if self.application_selected.as_ref().is_none_or(|selected| {
                        !report
                            .candidates
                            .iter()
                            .any(|candidate| &candidate.app_id == selected)
                    }) {
                        self.application_selected = report
                            .candidates
                            .iter()
                            .find(|candidate| !candidate.protection.protected)
                            .map(|candidate| candidate.app_id.clone());
                    }
                    self.application_error = None;
                    self.application_report = Some(report);
                }
                Err(error) => self.application_error = Some(error),
            }
        }
    }

    fn persist(&mut self) -> Result<(), StorageError> {
        match &self.config_path {
            Some(path) => storage::json::save_to(&self.config, path),
            None => storage::save(&self.config),
        }
    }

    /// Resolve and cache the visual for a raw `icon` field value. Routes
    /// every icon through `resolve_icon` (or, on Linux since Part 3 Phase 2,
    /// the theme-aware `resolve_icon_for_platform` wrapper) ->
    /// `decode_resize_file` -> `IconBackend::load` (the Phase 1 pipeline) —
    /// no hardcoded test icon.
    fn icon_visual(&mut self, icon: &str) -> IconVisual {
        if let Some(cached) = self.icon_cache.get(icon) {
            return cached.clone();
        }

        let dirs = icons_dirs();
        let size = self.config.default_settings.icon_size.max(1);
        let visual = match Self::resolve_icon_for_platform(icon, &dirs, size) {
            IconResolution::EmojiFallback(text) => IconVisual::Emoji(text),
            IconResolution::Image(path) => match decode_resize_file(&path, size) {
                Ok(decoded) => {
                    let texture = self.icon_backend.load(icon, &decoded);
                    IconVisual::Texture(texture)
                }
                Err(err) => {
                    log::warn!("icon_visual: decode failed for {path:?}: {err}");
                    IconVisual::Emoji("🔧".to_string())
                }
            },
        };

        self.icon_cache.insert(icon.to_string(), visual.clone());
        visual
    }

    /// Icon resolution, OS-dispatched. On Linux (since Part 3 Phase 2) this
    /// goes through `crate::linux::icon_theme::resolve_icon_with_theme`,
    /// which composes over the OS-neutral `resolve_icon` (tried first, so
    /// direct paths / `.svg` descoping / bundled overrides under
    /// `platform::data_dir()/icons` keep their existing precedence) and
    /// only falls through to a real freedesktop icon-theme lookup for a
    /// bare freedesktop-style name that `resolve_icon` couldn't place.
    /// Every other OS keeps the plain Phase 1 `resolve_icon` behavior
    /// unchanged.
    #[cfg(target_os = "linux")]
    fn resolve_icon_for_platform(icon: &str, dirs: &[PathBuf], size: u32) -> IconResolution {
        crate::linux::icon_theme::resolve_icon_with_theme(icon, dirs, size)
    }

    #[cfg(not(target_os = "linux"))]
    fn resolve_icon_for_platform(icon: &str, dirs: &[PathBuf], size: u32) -> IconResolution {
        let _ = size;
        resolve_icon(icon, dirs)
    }

    /// Renders one card. The clickable/launch-relevant body (icon + name +
    /// disabled-state message) is scoped inside `ui.add_enabled_ui`, keyed
    /// on `card.is_configured` — `false` only for a `machine_specific: true`
    /// command with no matching per-machine mapping entry (Part 3). The
    /// favorite-toggle button is added OUTSIDE that scope so it stays
    /// clickable regardless of resolution state — toggling favorite status
    /// is a config edit, not a launch (see this lot's risk register).
    fn render_card(&mut self, ui: &mut egui::Ui, card: &CardData, toggles: &mut Vec<String>) {
        if card.variants.is_empty() {
            self.render_simple_card(ui, card, toggles);
        } else {
            self.render_grouped_card(ui, card, toggles);
        }
    }

    fn render_simple_card(&mut self, ui: &mut egui::Ui, card: &CardData, toggles: &mut Vec<String>) {
        let visual = self.icon_visual(&card.icon);
        let mut body_clicked = false;
        ui.group(|ui| {
            ui.set_width(96.0);
            ui.vertical_centered(|ui| {
                ui.add_enabled_ui(card.is_configured, |ui| {
                    // The whole icon+name block is the click target, not
                    // just the name text — a text-only target left most of
                    // the card (icon, padding) dead to clicks in real
                    // mouse use, even though it passed kittest (which
                    // clicks by accessibility label regardless of hit
                    // size). `.interact(Sense::click())` upgrades the
                    // container response's existing rect/id, and an
                    // explicit `widget_info` keeps it discoverable via
                    // `egui_kittest::Queryable::get_by_label(&card.name)`.
                    let body_response = ui
                        .vertical_centered(|ui| {
                            match visual {
                                IconVisual::Texture(texture) => {
                                    let size = texture.size_vec2();
                                    let display_size = egui::vec2(48.0, 48.0)
                                        .min(size.max(egui::vec2(1.0, 1.0)));
                                    ui.add(egui::Image::new((texture.id(), display_size)));
                                }
                                IconVisual::Emoji(text) => {
                                    // `selectable(false)`: a plain `ui.label`
                                    // is selectable text by default, which
                                    // gives it its own click+drag sense —
                                    // that widget then wins hit-testing over
                                    // the card's own `.interact(click)`
                                    // below, silently eating every click.
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(text).size(28.0))
                                            .selectable(false),
                                    );
                                }
                            }
                            ui.add(
                                egui::Label::new(egui::RichText::new(&card.name).strong())
                                    .selectable(false),
                            );
                        })
                        .response
                        .interact(egui::Sense::click());
                    body_response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &card.name)
                    });
                    if body_response.clicked() {
                        body_clicked = true;
                    }

                    if let Some(message) = &card.disabled_message {
                        ui.label(egui::RichText::new(message).small().italics())
                            .on_hover_text(message.clone());
                    }
                });

                let star_label = if card.is_favorite {
                    "★ Favori"
                } else {
                    "☆ Favori"
                };
                if ui.small_button(star_label).clicked() {
                    toggles.push(card.command_id.clone());
                }
            });
        });

        // `add_enabled_ui` already suppresses `clicked()` for an
        // unconfigured card, so `card.is_configured` is redundant with
        // `body_clicked` here — kept explicit as the same guard
        // `can_launch_card` exposes for its pure unit test.
        if body_clicked && can_launch_card(card.is_configured, &self.action_running) {
            self.launch_command(&card.command_id, &card.command);
        }
    }

    /// Grouped card: a `ComboBox` selects among `card.variants` (session
    /// state in `self.selected_variant`, keyed by `card.group_name`) and an
    /// explicit "Lancer" button launches the selected variant's resolved
    /// command — unlike a simple card, the body itself is not clickable, so
    /// interacting with the dropdown never risks an accidental launch.
    fn render_grouped_card(&mut self, ui: &mut egui::Ui, card: &CardData, toggles: &mut Vec<String>) {
        let visual = self.icon_visual(&card.icon);
        let group_key = card.group_name.clone().unwrap_or_else(|| card.command_id.clone());
        let selected_id = self
            .selected_variant
            .get(&group_key)
            .cloned()
            .unwrap_or_else(|| card.variants[0].command_id.clone());
        let selected = card
            .variants
            .iter()
            .find(|v| v.command_id == selected_id)
            .unwrap_or(&card.variants[0]);
        let selected_command_id = selected.command_id.clone();
        let selected_command = selected.command.clone();
        let selected_label = selected.label.clone();
        let selected_is_configured = selected.is_configured;
        let selected_is_favorite = selected.is_favorite;
        let selected_disabled_message = selected.disabled_message.clone();

        let mut requested_variant: Option<String> = None;
        let mut launch_clicked = false;
        let mut favorite_clicked = false;

        ui.group(|ui| {
            ui.set_width(96.0);
            ui.vertical_centered(|ui| {
                match visual {
                    IconVisual::Texture(texture) => {
                        let size = texture.size_vec2();
                        let display_size =
                            egui::vec2(48.0, 48.0).min(size.max(egui::vec2(1.0, 1.0)));
                        ui.add(egui::Image::new((texture.id(), display_size)));
                    }
                    IconVisual::Emoji(text) => {
                        ui.label(egui::RichText::new(text).size(28.0));
                    }
                }
                ui.label(egui::RichText::new(&card.name).strong());

                egui::ComboBox::from_id_salt(&group_key)
                    .selected_text(&selected_label)
                    .show_ui(ui, |ui| {
                        for variant in &card.variants {
                            let is_selected = variant.command_id == selected_command_id;
                            if ui.selectable_label(is_selected, &variant.label).clicked() {
                                requested_variant = Some(variant.command_id.clone());
                            }
                        }
                    });

                if let Some(message) = &selected_disabled_message {
                    ui.label(egui::RichText::new(message).small().italics())
                        .on_hover_text(message.clone());
                }

                ui.add_enabled_ui(
                    can_launch_card(selected_is_configured, &self.action_running),
                    |ui| {
                        if ui.button("Lancer").clicked() {
                            launch_clicked = true;
                        }
                    },
                );

                let star_label = if selected_is_favorite {
                    "★ Favori"
                } else {
                    "☆ Favori"
                };
                if ui.small_button(star_label).clicked() {
                    favorite_clicked = true;
                }
            });
        });

        if let Some(variant_id) = requested_variant {
            self.selected_variant.insert(group_key, variant_id);
        }
        if launch_clicked && can_launch_card(selected_is_configured, &self.action_running) {
            self.launch_command(&selected_command_id, &selected_command);
        }
        if favorite_clicked {
            toggles.push(selected_command_id);
        }
    }

    /// Launch a card's resolved command in the background, through the
    /// dedicated `action_rx` slot so it never interferes with a command
    /// running from the Terminal view's own input field — but its output
    /// still streams into `terminal_lines` and switches `active_view` to
    /// Terminal, so the user actually sees what a diagnostic command (e.g.
    /// `ipconfig`) printed instead of just a "launched successfully"
    /// status with no visible result.
    fn launch_command(&mut self, command_id: &str, command: &str) {
        let (tx, rx) = std::sync::mpsc::channel();
        match terminal_view::launch_captured(command, tx) {
            Ok(_pid) => {
                self.action_rx = Some(rx);
                self.action_running = Some(command_id.to_string());
                self.terminal_lines.push_back(format!("$ {command}"));
                terminal_view::trim_lines(&mut self.terminal_lines);
                self.active_view = ActiveView::Terminal;
            }
            Err(err) => {
                self.set_status(format!("Échec du lancement: {err}"), true);
            }
        }
    }

    fn apply_favorite_toggles(&mut self, toggles: Vec<String>) {
        for command_id in toggles {
            match storage::toggle_favorite(&mut self.config, &command_id) {
                Ok(new_state) => match self.persist() {
                    Ok(()) => {
                        let word = if new_state {
                            "ajouté aux"
                        } else {
                            "retiré des"
                        };
                        self.set_status(format!("'{command_id}' {word} favoris."), false);
                    }
                    Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
                },
                Err(err) => self.set_status(err.to_string(), true),
            }
        }
    }

    fn apply_category_action(&mut self, action: CategoryAction) {
        // Removal is destructive (irreversible for the caller — commands in
        // the category get silently re-bucketed as "Sans catégorie" rather
        // than deleted, but the category itself is gone) and must go through
        // a blocking confirm dialog rather than acting immediately (Part 2
        // plan Risk register item 3 / this phase's acceptance criterion).
        if let CategoryAction::Remove { id } = &action {
            let label = self
                .config
                .categories
                .iter()
                .find(|c| &c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| id.clone());
            self.active_dialog = Some(ActiveDialog {
                kind: dialogs::confirm(
                    "Supprimer la catégorie",
                    format!(
                        "Supprimer la catégorie « {label} » ? Les commandes qu'elle contient \
                         seront reclassées dans « Sans catégorie »."
                    ),
                ),
                on_confirm: Some(PendingAction::RemoveCategory(id.clone())),
            });
            return;
        }

        let result = match &action {
            CategoryAction::Add => {
                let id = self.category_form.id.trim().to_string();
                let name = self.category_form.name.trim().to_string();
                let icon = self.category_form.icon.trim().to_string();
                if id.is_empty() || name.is_empty() {
                    self.set_status("L'id et le nom de la catégorie sont requis.", true);
                    return;
                }
                storage::add_category(&mut self.config, id, name, icon)
            }
            CategoryAction::Rename { id, new_name } => {
                storage::rename_category(&mut self.config, id, new_name.clone())
            }
            CategoryAction::Remove { .. } => unreachable!("handled by the early return above"),
        };

        match result {
            Ok(()) => {
                if matches!(action, CategoryAction::Add) {
                    self.category_form = CategoryForm::default();
                }
                match self.persist() {
                    Ok(()) => self.set_status("Catégorie mise à jour.", false),
                    Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
                }
            }
            Err(err) => self.set_status(err.to_string(), true),
        }
    }

    /// Perform the action a confirm dialog was guarding, once the user
    /// picked "Oui" — see [`ActiveDialog::on_confirm`].
    fn resolve_pending_action(&mut self, action: PendingAction) {
        match action {
            PendingAction::RemoveCategory(id) => {
                match storage::remove_category(&mut self.config, &id) {
                    Ok(()) => match self.persist() {
                        Ok(()) => self.set_status("Catégorie supprimée.", false),
                        Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
                    },
                    Err(err) => self.set_status(err.to_string(), true),
                }
            }
        }
    }

    /// Dedicated Préférences view — moved out of `render_actions_view` (was
    /// a `CollapsingHeader` at the top of the Actions grid) so category
    /// management no longer eats into the vertical space available for
    /// action cards.
    fn render_preferences_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Préférences");
        ui.separator();

        let mut actions: Vec<CategoryAction> = Vec::new();
        let categories_snapshot = self.config.categories.clone();

        ui.label("Catégories");
        for category in &categories_snapshot {
            ui.horizontal(|ui| {
                ui.label(&category.icon);
                ui.label(&category.name);
                let buffer = self
                    .rename_buffers
                    .entry(category.id.clone())
                    .or_insert_with(|| category.name.clone());
                ui.text_edit_singleline(buffer);
                if ui.button("Renommer").clicked() && !buffer.trim().is_empty() {
                    actions.push(CategoryAction::Rename {
                        id: category.id.clone(),
                        new_name: buffer.clone(),
                    });
                }
                if ui.button("Supprimer").clicked() {
                    actions.push(CategoryAction::Remove {
                        id: category.id.clone(),
                    });
                }
            });
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Nouvelle catégorie —");

            let id_label = ui.label("id");
            ui.text_edit_singleline(&mut self.category_form.id)
                .labelled_by(id_label.id)
                .on_hover_text("identifiant unique");

            let name_label = ui.label("nom");
            ui.text_edit_singleline(&mut self.category_form.name)
                .labelled_by(name_label.id)
                .on_hover_text("nom affiché");

            let icon_label = ui.label("icône");
            ui.text_edit_singleline(&mut self.category_form.icon)
                .labelled_by(icon_label.id)
                .on_hover_text("emoji");

            if ui.button("Ajouter").clicked() {
                actions.push(CategoryAction::Add);
            }
        });

        for action in actions {
            self.apply_category_action(action);
        }
    }

    /// The actual UI content, factored out of `eframe::App::ui` so it can be
    /// driven directly by an `egui_kittest::Harness` in tests (no
    /// `eframe::Frame` required).
    ///
    /// A blocking [`ActiveDialog`], if any, is checked *first* and
    /// short-circuits everything else — nothing behind it (nav row,
    /// Actions/Terminal/Automations views) is even built for this frame, on
    /// top of `egui::Modal`'s own backdrop-click blocking. See
    /// `src/ui/dialogs.rs`'s module docs for the full rationale.
    fn ui_content(&mut self, ui: &mut egui::Ui) {
        if let Some(dialog) = self.active_dialog.take() {
            match dialogs::show(ui.ctx(), &dialog.kind) {
                DialogOutcome::Pending => {
                    self.active_dialog = Some(dialog);
                }
                DialogOutcome::Accepted => {
                    if let Some(action) = dialog.on_confirm {
                        self.resolve_pending_action(action);
                    }
                }
                DialogOutcome::Dismissed => {
                    // Cancel — the pending action (if any) is simply dropped;
                    // nothing was mutated by opening the dialog.
                }
            }
            return;
        }

        self.drain_terminal_events();
        self.drain_action_events();
        self.drain_application_events();
        if self.terminal_running {
            // Keep polling for output while a command is in flight, even if
            // the user has switched away from the Terminal view.
            ui.ctx().request_repaint();
        }
        if self.action_running.is_some() {
            // Same rationale as above, for a command launched from an
            // Actions card.
            ui.ctx().request_repaint();
        }
        if self.application_loading {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }

        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.active_view, ActiveView::Actions, "Actions");
            ui.selectable_value(&mut self.active_view, ActiveView::Terminal, "Terminal");
            ui.selectable_value(
                &mut self.active_view,
                ActiveView::Automations,
                "Automatisations",
            );
            ui.selectable_value(
                &mut self.active_view,
                ActiveView::Applications,
                "Applications",
            );
            ui.selectable_value(
                &mut self.active_view,
                ActiveView::Preferences,
                "Préférences",
            );
            if ui.button("À propos").clicked() {
                self.active_dialog = Some(ActiveDialog {
                    kind: dialogs::info(
                        "À propos de DevToolBox",
                        "DevToolBox\nLanceur de scripts et d'outils.",
                    ),
                    on_confirm: None,
                });
            }
        });
        ui.separator();

        match self.active_view {
            ActiveView::Actions => self.render_actions_view(ui),
            ActiveView::Terminal => self.render_terminal_view(ui),
            ActiveView::Automations => self.render_automations_view(ui),
            ActiveView::Applications => self.render_applications_view(ui),
            ActiveView::Preferences => self.render_preferences_view(ui),
        }
    }

    fn render_actions_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("DevToolBox — Actions");

        if let Some(status) = &self.status {
            let color = if status.is_error {
                egui::Color32::from_rgb(0xC4, 0x2B, 0x1C)
            } else {
                egui::Color32::from_rgb(0x1B, 0x5E, 0x20)
            };
            ui.colored_label(color, &status.text);
        }

        let mut show_categories = self.config.default_settings.show_categories;
        if ui
            .checkbox(&mut show_categories, "Afficher par catégories")
            .changed()
        {
            self.config.default_settings.show_categories = show_categories;
            if let Err(err) = self.persist() {
                self.set_status(format!("Échec de sauvegarde: {err}"), true);
            }
        }

        let groups = build_display_groups(&self.config, &self.machine_commands, &self.machine_id);
        let mut toggles: Vec<String> = Vec::new();

        egui::ScrollArea::vertical().show(ui, |ui| {
            // `ScrollArea::vertical()` does not clamp its content's width by
            // default, so `horizontal_wrapped` below would see an unbounded
            // wrap width and cards would overflow past the window edge
            // instead of flowing onto a new row. Pinning the width to what's
            // actually available re-derives it every frame, so the grid
            // reflows as the window is resized.
            ui.set_width(ui.available_width());
            for group in &groups {
                if let Some(header) = &group.header {
                    ui.add_space(8.0);
                    ui.heading(header);
                }
                if group.cards.is_empty() {
                    ui.label("(aucune commande)");
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    for card in &group.cards {
                        self.render_card(ui, card, &mut toggles);
                    }
                });
            }
        });

        self.apply_favorite_toggles(toggles);
    }

    /// Launch `self.terminal_input` in the background — cross-platform port
    /// of `app.rs`'s Terminal panel launch button, using
    /// `crate::ui::terminal_view::launch_captured` (plain
    /// `std::process::Command`, not the Windows-only `windows::process`
    /// pipeline).
    fn launch_terminal_command(&mut self) {
        let command = self.terminal_input.trim().to_string();
        if command.is_empty() {
            self.set_status("La commande est vide.", true);
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        match terminal_view::launch_captured(&command, tx) {
            Ok(_pid) => {
                self.terminal_lines.push_back(format!("$ {command}"));
                terminal_view::trim_lines(&mut self.terminal_lines);
                self.terminal_rx = Some(rx);
                self.terminal_running = true;
            }
            Err(err) => {
                self.set_status(format!("Échec du lancement: {err}"), true);
            }
        }
    }

    /// Drain any pending [`TerminalEvent`]s from the running command, if
    /// any, appending complete lines to `terminal_lines` (already
    /// newline-split by `terminal_view::stream_output`, so no `feed_text`
    /// buffering step is needed here — unlike `app.rs`'s original raw
    /// character-stream source, this event stream already yields whole
    /// lines).
    fn drain_terminal_events(&mut self) {
        let Some(rx) = &self.terminal_rx else {
            return;
        };

        let mut settled = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                TerminalEvent::Started { .. } => {}
                TerminalEvent::Output(line) => {
                    self.terminal_lines.push_back(line);
                    terminal_view::trim_lines(&mut self.terminal_lines);
                }
                TerminalEvent::Finished { code } => {
                    self.terminal_lines
                        .push_back(format!("(terminé — code {code:?})"));
                    settled = true;
                }
                TerminalEvent::Failed(err) => {
                    self.terminal_lines.push_back(format!("(erreur: {err})"));
                    settled = true;
                }
            }
        }

        if settled {
            self.terminal_running = false;
            self.terminal_rx = None;
        }
    }

    /// Mirror of `drain_terminal_events` for a card launch: drains
    /// `action_rx`, feeding both `self.status` (success/error) and
    /// `terminal_lines` (so the command's actual output is visible in the
    /// Terminal view, which `launch_command` already switched to), and
    /// clears `action_running` once the command settles — freeing the
    /// concurrency guard for the next card click.
    fn drain_action_events(&mut self) {
        let Some(rx) = &self.action_rx else {
            return;
        };

        let mut settled = false;
        let mut outcome: Option<Result<(), String>> = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                TerminalEvent::Started { .. } => {}
                TerminalEvent::Output(line) => {
                    self.terminal_lines.push_back(line);
                    terminal_view::trim_lines(&mut self.terminal_lines);
                }
                TerminalEvent::Finished { code } => {
                    self.terminal_lines
                        .push_back(format!("(terminé — code {code:?})"));
                    outcome = Some(Ok(()));
                    settled = true;
                }
                TerminalEvent::Failed(err) => {
                    self.terminal_lines.push_back(format!("(erreur: {err})"));
                    outcome = Some(Err(err));
                    settled = true;
                }
            }
        }

        if settled {
            let command_id = self.action_running.take().unwrap_or_default();
            self.action_rx = None;
            self.status = Some(match outcome.expect("settled implies a terminal event was seen") {
                Ok(()) => StatusMessage {
                    text: format!("'{command_id}' lancé avec succès."),
                    is_error: false,
                },
                Err(err) => StatusMessage {
                    text: format!("Échec du lancement de '{command_id}': {err}"),
                    is_error: true,
                },
            });
        }
    }

    fn render_terminal_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("DevToolBox — Terminal");

        ui.horizontal(|ui| {
            let command_label = ui.label("commande");
            ui.text_edit_singleline(&mut self.terminal_input)
                .labelled_by(command_label.id);

            if ui
                .add_enabled(!self.terminal_running, egui::Button::new("Lancer"))
                .clicked()
            {
                self.launch_terminal_command();
            }

            if self.terminal_running {
                ui.label("(en cours…)");
            }
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.terminal_lines.is_empty() {
                    ui.label("(aucune sortie)");
                }
                for line in &self.terminal_lines {
                    ui.label(line);
                }
            });
    }

    fn render_automations_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("DevToolBox — Automatisations");

        if self.automations.is_none() {
            self.automations = Some(automations_view::fetch());
        }

        ui.horizontal(|ui| {
            if ui.button("Rafraîchir").clicked() {
                self.automations = Some(automations_view::fetch());
            }
        });
        ui.separator();

        match &self.automations {
            None => unreachable!("populated just above if it was None"),
            Some(Err(err)) => {
                ui.colored_label(egui::Color32::from_rgb(0xC4, 0x2B, 0x1C), err);
            }
            Some(Ok(rows)) if rows.is_empty() => {
                ui.label(automations_placeholder_message());
            }
            Some(Ok(rows)) => {
                egui::Grid::new("automations-grid")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Nom");
                        ui.strong("Catégorie");
                        ui.strong("Prochaine exécution");
                        ui.strong("État");
                        ui.strong("Auteur");
                        ui.end_row();
                        for row in rows {
                            ui.label(&row.name);
                            ui.label(&row.category);
                            ui.label(&row.next_run);
                            ui.label(&row.state);
                            ui.label(&row.author);
                            ui.end_row();
                        }
                    });
            }
        }
    }

    fn render_applications_view(&mut self, ui: &mut egui::Ui) {
        let refresh = applications_view::render(
            ui,
            self.application_report.as_ref(),
            self.application_error.as_deref(),
            self.application_loading,
            &mut self.application_filters,
            &mut self.application_selected,
        );
        if refresh {
            self.refresh_applications();
        }
    }
}

/// Empty-state message for the Automations view. Both Windows
/// (`Get-ScheduledTask`) and Linux (`systemctl list-timers`, since Part 3
/// Phase 2) now have a real data source wired, so an empty result
/// genuinely means "zero automations found on this system" on either OS —
/// no OS-specific "not implemented yet" wording is needed anymore.
fn automations_placeholder_message() -> &'static str {
    "Aucune automatisation trouvée."
}

impl eframe::App for EguiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            self.ui_content(ui);
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};
    use storage::{Category, Command, Settings};

    fn sample_config() -> Config {
        Config {
            version: "0.1.0".to_string(),
            default_settings: Settings {
                show_categories: true,
                icon_size: 32,
                theme: "light".to_string(),
                launch_at_startup: false,
                show_descriptions: true,
            },
            categories: vec![Category {
                id: "system".into(),
                name: "Système".into(),
                icon: "🖥️".into(),
            }],
            commands: vec![
                Command {
                    id: "notepad".into(),
                    name: "Bloc-notes".into(),
                    command: "notepad.exe".into(),
                    category: "system".into(),
                    icon: "📝".into(),
                    is_favorite: true,
                    shortcut: None,
                    variant_group: None,
                    group_name: None,
                    variant_label: None,
                    machine_specific: false,
                },
                Command {
                    id: "cmd".into(),
                    name: "Invite de commandes".into(),
                    command: "cmd.exe".into(),
                    category: "system".into(),
                    icon: "💻".into(),
                    is_favorite: false,
                    shortcut: None,
                    variant_group: None,
                    group_name: None,
                    variant_label: None,
                    machine_specific: false,
                },
            ],
        }
    }

    /// Builds one variant of a variant-grouped command — the Phase 2
    /// counterpart of `sample_config`'s plain `Command` literals.
    #[allow(clippy::too_many_arguments)]
    fn variant_command(
        id: &str,
        variant_group: &str,
        group_name: &str,
        variant_label: &str,
        command: &str,
        category: &str,
        is_favorite: bool,
    ) -> Command {
        Command {
            id: id.into(),
            name: variant_label.into(),
            command: command.into(),
            category: category.into(),
            icon: "🔧".into(),
            is_favorite,
            shortcut: None,
            variant_group: Some(variant_group.into()),
            group_name: Some(group_name.into()),
            variant_label: Some(variant_label.into()),
            machine_specific: false,
        }
    }

    /// `sample_config()` with its `commands` replaced wholesale — new
    /// grouped-card tests need fixtures with no widget-label overlap with
    /// `sample_config`'s own "Bloc-notes"/"Invite de commandes" cards
    /// (`kittest`'s `get()`/`query()` panics on more than one match), so
    /// they build their command list from scratch rather than appending.
    fn config_with_commands(commands: Vec<Command>) -> Config {
        let mut config = sample_config();
        config.commands = commands;
        config
    }

    fn sample_application_report() -> RecommendationReport {
        serde_json::from_str(
            r#"{
              "schema_version":1,"generated_at":"2026-08-14T12:00:00Z","platform":"linux",
              "candidates":[{
                "app_id":"apt:editor","source":"apt","name":"Editor",
                "size":{"installed_bytes":1073741824,"reclaimable_bytes":null,"method":"dpkg","scope":"paquet","confidence":"high"},
                "executable_hints":["/usr/bin/editor"],
                "usage":{"kind":"not_observed","last_seen":null,"tracked_since":"2026-01-01T00:00:00Z","covered_days":90,"confidence":"medium"},
                "protection":{"protected":false,"reasons":[]},
                "command":{"value":"sudo apt-get remove -- editor","origin":"manager_verified"},
                "score":50,"confidence":"medium","reasons":["Empreinte disque : +25"],"metadata":{}
              }],"source_errors":[],"warnings":[]
            }"#,
        )
        .unwrap()
    }

    // -- Pure build_display_groups tests -------------------------------------

    #[test]
    fn flat_mode_returns_only_favorites_in_a_single_headerless_group() {
        let mut config = sample_config();
        config.default_settings.show_categories = false;

        let groups = build_display_groups(&config, &MachineCommands::default(), "test-machine");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].header, None);
        assert_eq!(groups[0].cards.len(), 1);
        assert_eq!(groups[0].cards[0].command_id, "notepad");
    }

    #[test]
    fn grouped_mode_labels_orphan_commands_as_sans_categorie() {
        let mut config = sample_config();
        config.default_settings.show_categories = true;
        config.commands.push(Command {
            id: "orphan".into(),
            name: "Orphelin".into(),
            command: "true".into(),
            category: "does-not-exist".into(),
            icon: "❓".into(),
            is_favorite: false,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: false,
        });

        let groups = build_display_groups(&config, &MachineCommands::default(), "test-machine");

        assert!(groups
            .iter()
            .any(|g| g.header.as_deref() == Some("Système") && g.cards.len() == 2));
        assert!(groups
            .iter()
            .any(|g| g.header.as_deref() == Some("Sans catégorie")
                && g.cards.iter().any(|c| c.command_id == "orphan")));
    }

    // -- Phase 2: variant-group consolidation --------------------------------

    #[test]
    fn variant_group_commands_consolidate_into_one_card_with_all_variants() {
        let commands = vec![
            variant_command("sftp-pro", "sftp-sync", "Synchroniser", "Pro", "sync.sh pro", "system", true),
            variant_command(
                "sftp-perso",
                "sftp-sync",
                "Synchroniser",
                "Perso",
                "sync.sh perso",
                "system",
                true,
            ),
            variant_command(
                "sftp-hermes",
                "sftp-sync",
                "Synchroniser",
                "Hermes",
                "sync.sh hermes",
                "system",
                true,
            ),
            variant_command("sftp-tout", "sftp-sync", "Synchroniser", "Tout", "sync.sh tout", "system", true),
        ];

        for show_categories in [true, false] {
            let mut config = config_with_commands(commands.clone());
            config.default_settings.show_categories = show_categories;

            let groups = build_display_groups(&config, &MachineCommands::default(), "test-machine");
            let cards: Vec<&CardData> = groups.iter().flat_map(|g| &g.cards).collect();

            assert_eq!(
                cards.len(),
                1,
                "show_categories={show_categories}: expected exactly one grouped card"
            );
            assert_eq!(cards[0].variants.len(), 4);
            assert_eq!(cards[0].group_name.as_deref(), Some("Synchroniser"));
        }
    }

    #[test]
    fn a_single_favorite_variant_still_pulls_in_all_its_siblings_in_favorites_mode() {
        let commands = vec![
            variant_command(
                "e2m-tray",
                "email-to-markdown",
                "Email to Markdown",
                "Tray auto",
                "e2m.sh tray",
                "system",
                true,
            ),
            variant_command(
                "e2m-release",
                "email-to-markdown",
                "Email to Markdown",
                "Tray release",
                "e2m.sh release",
                "system",
                false,
            ),
            variant_command(
                "e2m-debug",
                "email-to-markdown",
                "Email to Markdown",
                "Tray debug",
                "e2m.sh debug",
                "system",
                false,
            ),
            variant_command(
                "e2m-build-release",
                "email-to-markdown",
                "Email to Markdown",
                "Build+launch release",
                "e2m.sh build-release",
                "system",
                false,
            ),
            variant_command(
                "e2m-build-debug",
                "email-to-markdown",
                "Email to Markdown",
                "Build+launch debug",
                "e2m.sh build-debug",
                "system",
                false,
            ),
        ];
        let mut config = config_with_commands(commands);
        config.default_settings.show_categories = false;

        let groups = build_display_groups(&config, &MachineCommands::default(), "test-machine");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].cards.len(), 1);
        assert_eq!(
            groups[0].cards[0].variants.len(),
            5,
            "non-favorite siblings must still ride along in the dropdown"
        );
    }

    #[test]
    fn an_ungrouped_command_still_produces_exactly_one_plain_card() {
        for show_categories in [true, false] {
            let mut config = sample_config();
            config.default_settings.show_categories = show_categories;

            let groups = build_display_groups(&config, &MachineCommands::default(), "test-machine");
            let cards: Vec<&CardData> = groups.iter().flat_map(|g| &g.cards).collect();

            let notepad = cards
                .iter()
                .find(|c| c.command_id == "notepad")
                .expect("notepad card present");
            assert!(notepad.variants.is_empty());
            assert_eq!(notepad.group_name, None);
        }
    }

    // -- Part 3: per-machine resolution wiring into CardData -----------------

    #[test]
    fn non_machine_specific_cards_are_always_configured_regardless_of_overrides() {
        // No card in `sample_config()` opts into `machine_specific: true`, so
        // every card must resolve as configured even against a completely
        // empty mapping — the no-visual-regression guarantee for existing
        // (non-machine-specific) cards.
        let config = sample_config();

        let groups = build_display_groups(&config, &MachineCommands::default(), "some-machine");

        for group in &groups {
            for card in &group.cards {
                assert!(
                    card.is_configured,
                    "non-machine-specific card {:?} must always be configured",
                    card.command_id
                );
                assert!(
                    card.disabled_message.is_none(),
                    "non-machine-specific card {:?} must never carry a disabled message",
                    card.command_id
                );
            }
        }
    }

    #[test]
    fn machine_specific_command_with_no_matching_mapping_entry_renders_disabled_with_message() {
        let mut config = sample_config();
        config.default_settings.show_categories = false;
        config.commands.push(Command {
            id: "deploy".into(),
            name: "Déploiement".into(),
            command: "deploy.sh".into(),
            category: "system".into(),
            icon: "🚀".into(),
            is_favorite: true,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: true,
        });

        // Empty mapping: "deploy" has no override for "laptop-x".
        let groups = build_display_groups(&config, &MachineCommands::default(), "laptop-x");

        let card = groups
            .iter()
            .flat_map(|g| &g.cards)
            .find(|c| c.command_id == "deploy")
            .expect("the machine-specific 'deploy' card must still be present, just disabled");

        assert!(
            !card.is_configured,
            "a machine_specific command with no matching mapping entry must render disabled"
        );
        let message = card
            .disabled_message
            .as_ref()
            .expect("an unconfigured card must carry an inline disabled message");
        assert!(
            message.contains("laptop-x"),
            "disabled message must name the current machine id; got: {message:?}"
        );
        let mapping_path = crate::platform::machine_commands_path();
        assert!(
            message.contains(&mapping_path.display().to_string()),
            "disabled message must name the mapping file path; got: {message:?}"
        );
    }

    #[test]
    fn machine_specific_command_with_a_matching_mapping_entry_renders_configured() {
        let mut config = sample_config();
        config.default_settings.show_categories = false;
        config.commands.push(Command {
            id: "deploy".into(),
            name: "Déploiement".into(),
            command: "deploy.sh".into(),
            category: "system".into(),
            icon: "🚀".into(),
            is_favorite: true,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: true,
        });

        let mut per_command = std::collections::BTreeMap::new();
        per_command.insert("deploy".to_string(), "deploy.sh --prod".to_string());
        let mut machines = std::collections::BTreeMap::new();
        machines.insert("laptop-x".to_string(), per_command);
        let overrides = MachineCommands { machines };

        let groups = build_display_groups(&config, &overrides, "laptop-x");

        let card = groups
            .iter()
            .flat_map(|g| &g.cards)
            .find(|c| c.command_id == "deploy")
            .expect("the 'deploy' card must be present");

        assert!(
            card.is_configured,
            "a machine_specific command with a matching mapping entry must render configured"
        );
        assert!(
            card.disabled_message.is_none(),
            "a configured card must not carry a disabled message"
        );
    }

    // -- Persistence-across-restart integration tests -------------------------
    //
    // These drive the REAL `EguiApp::ui_content` widget tree via
    // `egui_kittest::Harness` (simulated clicks/typing dispatched through
    // egui's normal event pipeline — not a direct call into `storage::`),
    // then reload the saved file from disk to confirm the on-disk state
    // actually round-trips. `config_path` is a tempfile, so the real user
    // config is never touched.

    #[test]
    fn toggling_favorite_via_the_real_ui_persists_across_a_simulated_restart() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "fav-toggle"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let config = sample_config();
        assert!(
            !config
                .commands
                .iter()
                .find(|c| c.id == "cmd")
                .unwrap()
                .is_favorite,
            "precondition: 'cmd' starts as not-favorite"
        );

        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        // First pass lays out the grid; the "cmd" card's favorite toggle is
        // rendered with label "☆ Favori" (not yet a favorite).
        harness.run();
        harness.get_by_label("☆ Favori").click();
        harness.run();

        // Reload from disk exactly like a fresh app boot would.
        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        let reloaded_cmd = reloaded
            .commands
            .iter()
            .find(|c| c.id == "cmd")
            .expect("'cmd' still present after reload");
        assert!(
            reloaded_cmd.is_favorite,
            "favorite toggle performed via simulated UI click did not persist to disk"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adding_a_category_via_the_real_ui_persists_across_a_simulated_restart() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "cat-add"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let config = sample_config();
        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        // Category management now lives on the dedicated Préférences view.
        harness.get_by_label("Préférences").click();
        harness.run();

        // Each `TextEdit` only consumes `Event::Text` while it holds egui's
        // memory focus, so each field needs an explicit focus (processed via
        // an intervening `run()`) before typing into it.
        harness.get_by_label("id").focus();
        harness.run();
        harness.get_by_label("id").type_text("net");
        harness.run();

        harness.get_by_label("nom").focus();
        harness.run();
        harness.get_by_label("nom").type_text("Réseau");
        harness.run();

        harness.get_by_label("icône").focus();
        harness.run();
        harness.get_by_label("icône").type_text("🌐");
        harness.run();

        harness.get_by_label("Ajouter").click_accesskit();
        harness.run();

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        assert!(
            reloaded
                .categories
                .iter()
                .any(|c| c.id == "net" && c.name == "Réseau"),
            "category added via simulated UI did not persist to disk; categories={:?}",
            reloaded.categories
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Confirm-dialog blocking tests (Phase 3 acceptance criterion) ---------
    //
    // Removing a category is destructive (Risk register item 3): these
    // drive the REAL nav row + categories panel + dialog through
    // `egui_kittest`, confirming (a) a confirm dialog blocks the background
    // UI (nothing behind it renders while it's up) and (b) canceling leaves
    // both in-memory and on-disk state untouched, while confirming performs
    // and persists the removal.

    #[test]
    fn removing_a_category_shows_a_blocking_confirm_dialog_and_cancel_leaves_state_unchanged() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "cat-remove-cancel"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let config = sample_config();
        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        harness.get_by_label("Préférences").click();
        harness.run();
        harness.get_by_label("Supprimer").click();
        harness.run();

        // The confirm dialog is now up and should block everything behind
        // it: the category row's own "Supprimer" button, the Préférences
        // view's "Catégories" heading — nothing but the dialog itself is
        // rendered this frame.
        assert!(
            harness.query_by_label("Oui").is_some(),
            "confirm dialog should be showing an Oui button"
        );
        assert!(
            harness.query_by_label("Non").is_some(),
            "confirm dialog should be showing a Non button"
        );
        assert!(
            harness.query_by_label("Supprimer").is_none(),
            "background category list must not render while the confirm dialog is active"
        );
        assert!(
            harness.query_by_label("Catégories").is_none(),
            "background nav/categories panel must not render while the confirm dialog is active"
        );

        harness.get_by_label("Non").click();
        harness.run();

        assert!(
            harness.query_by_label("Non").is_none(),
            "dialog should be closed after Non"
        );
        assert_eq!(
            harness.state().config.categories.len(),
            1,
            "canceling the confirm dialog must leave categories unchanged in memory"
        );
        assert!(
            !config_path.exists(),
            "canceling must never write to disk (nothing was persisted before the cancel)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_a_category_persists_once_the_confirm_dialog_is_accepted() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "cat-remove-confirm"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let config = sample_config();
        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        harness.get_by_label("Préférences").click();
        harness.run();
        harness.get_by_label("Supprimer").click();
        harness.run();
        harness.get_by_label("Oui").click();
        harness.run();

        assert!(
            harness.state().config.categories.is_empty(),
            "confirming removal must update in-memory state"
        );

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        assert!(
            reloaded.categories.is_empty(),
            "confirmed removal via the real UI did not persist to disk; categories={:?}",
            reloaded.categories
        );
        // remove_category re-buckets rather than deletes commands (see
        // storage::categories::remove_category_does_not_delete_commands).
        assert_eq!(
            reloaded.commands.len(),
            2,
            "removing a category must not delete the commands that were in it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Terminal view test (Phase 3 acceptance criterion) --------------------
    //
    // Drives the REAL Terminal view: types a command into the real text
    // field, clicks the real "Lancer" button (through egui's actual input
    // pipeline), and polls the harness (which drives real frames, draining
    // the real mpsc channel fed by a REAL spawned OS process) until the
    // command's real output appears in the rendered state.

    #[test]
    fn terminal_view_launches_a_real_command_and_displays_its_output() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "terminal-run"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let config = sample_config();
        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        harness.get_by_label("Terminal").click();
        harness.run();

        harness.get_by_label("commande").focus();
        harness.run();
        harness
            .get_by_label("commande")
            .type_text("echo hello-from-kittest-terminal-view");
        harness.run();

        harness.get_by_label("Lancer").click();
        // While the command is in flight, `ui_content` calls
        // `ui.ctx().request_repaint()` every frame to keep polling the
        // mpsc channel (see the `request_repaint` call near the top of the
        // update loop), so `Harness::run()`'s "run until no more repaints
        // are requested" contract never stabilizes here. Drive single
        // frames with `run_steps(1)` instead — each step still drains
        // whatever the real child process has produced so far.
        harness.run_steps(1);

        // The spawned real `echo` process streams asynchronously from two
        // independent unsynchronized threads (the stdout reader and the
        // `child.wait()` reaper, see `launch_captured`), so the `Finished`
        // event can land a poll or two after the `Output` line does. Poll
        // real frames until both have shown up, or fail after a generous
        // bound rather than hanging forever.
        let mut saw_output = false;
        for _ in 0..200 {
            saw_output = saw_output
                || harness
                    .state()
                    .terminal_lines
                    .iter()
                    .any(|line| line.contains("hello-from-kittest-terminal-view"));
            if saw_output && !harness.state().terminal_running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            harness.run_steps(1);
        }

        assert!(
            saw_output,
            "expected the real echoed output to appear in the Terminal view's rendered lines; got {:?}",
            harness.state().terminal_lines
        );
        assert!(
            !harness.state().terminal_running,
            "the real echo process should have finished by now"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Card click-to-launch tests (this phase's acceptance criteria) -------

    #[test]
    fn can_launch_card_blocks_any_click_while_another_launch_is_in_flight() {
        let another_launch = Some("some-other-command".to_string());
        assert!(
            !can_launch_card(true, &another_launch),
            "a configured card must not be launchable while a different launch is running"
        );
        assert!(
            can_launch_card(true, &None),
            "a configured card must be launchable when no launch is in flight"
        );
        assert!(
            !can_launch_card(false, &None),
            "an unconfigured card must never be launchable, even with no launch in flight"
        );
    }

    #[test]
    fn clicking_a_configured_cards_body_launches_it_and_shows_a_success_status() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "card-click-launch"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let mut config = sample_config();
        config.commands.push(Command {
            id: "echo-card".into(),
            name: "Echo Carte".into(),
            command: "echo hello-from-card-click".into(),
            category: "system".into(),
            icon: "🔧".into(),
            is_favorite: false,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: false,
        });

        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        harness.get_by_label("Echo Carte").click();
        // Same rationale as the Terminal view test above: `ui_content`
        // requests a repaint every frame while `action_running` is set, so
        // `Harness::run()`'s "until no more repaints" contract never
        // stabilizes here — drive single frames instead.
        harness.run_steps(1);

        let mut saw_success = false;
        for _ in 0..200 {
            saw_success = saw_success
                || harness
                    .state()
                    .status
                    .as_ref()
                    .is_some_and(|status| !status.is_error && status.text.contains("echo-card"));
            if saw_success && harness.state().action_running.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            harness.run_steps(1);
        }

        assert!(
            saw_success,
            "expected a success status naming 'echo-card' after clicking its card body; got {:?}",
            harness.state().status.as_ref().map(|s| &s.text)
        );
        assert!(
            harness.state().action_running.is_none(),
            "action_running must be cleared once the launched command settles"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_the_body_of_an_unconfigured_card_does_not_launch_anything() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "card-click-unconfigured"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let mut config = sample_config();
        config.commands.push(Command {
            id: "deploy".into(),
            name: "Déploiement".into(),
            command: "deploy.sh".into(),
            category: "system".into(),
            icon: "🚀".into(),
            is_favorite: false,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: true,
        });

        // Empty mapping: "deploy" has no override for "test-machine" (the
        // fixed machine id used by `new_for_test`), so it renders disabled.
        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        harness.get_by_label("Déploiement").click();
        harness.run();

        assert!(
            harness.state().action_running.is_none(),
            "clicking an unconfigured card's body must not start a launch"
        );
        assert!(
            harness.state().status.is_none(),
            "clicking an unconfigured card's body must not produce a status message; got {:?}",
            harness.state().status.as_ref().map(|s| &s.text)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Grouped-card variant selector tests (this phase's acceptance criteria) --

    #[test]
    fn selecting_a_variant_then_clicking_lancer_launches_that_variants_command() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "variant-select-launch"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let commands = vec![
            variant_command(
                "sync-pro",
                "sync",
                "Synchroniser",
                "Pro",
                "echo hello-from-pro",
                "system",
                true,
            ),
            variant_command(
                "sync-perso",
                "sync",
                "Synchroniser",
                "Perso",
                "echo hello-from-perso",
                "system",
                true,
            ),
        ];
        let config = config_with_commands(commands);

        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        // "Pro" is the first variant in `config.commands` order, so it is
        // the lazily-defaulted initial selection — open the label-less
        // `ComboBox` via its current value (see `partition_by_variant_group`
        // doc comment / `VariantCardData`).
        harness.get_by_value("Pro").click();
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Perso")
            .click();
        harness.run();

        harness.get_by_label("Lancer").click();
        harness.run_steps(1);

        let mut saw_success = false;
        for _ in 0..200 {
            saw_success = saw_success
                || harness.state().status.as_ref().is_some_and(|status| {
                    !status.is_error && status.text.contains("sync-perso")
                });
            if saw_success && harness.state().action_running.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            harness.run_steps(1);
        }

        assert!(
            saw_success,
            "expected a success status naming the selected variant 'sync-perso', not the default 'sync-pro'; got {:?}",
            harness.state().status.as_ref().map(|s| &s.text)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn selecting_an_unconfigured_variant_disables_lancer() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "variant-select-unconfigured"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let mut commands = vec![variant_command(
            "sync-pro",
            "sync",
            "Synchroniser",
            "Pro",
            "echo hello-from-pro",
            "system",
            true,
        )];
        let mut perso = variant_command(
            "sync-perso",
            "sync",
            "Synchroniser",
            "Perso",
            "echo hello-from-perso",
            "system",
            true,
        );
        // No override exists for "test-machine" in the empty `MachineCommands`
        // used by `new_for_test`, so this variant resolves as unconfigured.
        perso.machine_specific = true;
        commands.push(perso);
        let config = config_with_commands(commands);

        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        harness.get_by_value("Pro").click();
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Perso")
            .click();
        harness.run();

        harness.get_by_label("Lancer").click();
        harness.run();

        assert!(
            harness.state().action_running.is_none(),
            "clicking 'Lancer' on an unconfigured selected variant must not start a launch"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn favorite_toggle_on_a_grouped_card_only_affects_the_selected_variant() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "variant-favorite-toggle"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let commands = vec![
            variant_command(
                "sync-pro",
                "sync",
                "Synchroniser",
                "Pro",
                "echo hello-from-pro",
                "system",
                false,
            ),
            variant_command(
                "sync-perso",
                "sync",
                "Synchroniser",
                "Perso",
                "echo hello-from-perso",
                "system",
                true,
            ),
        ];
        let config = config_with_commands(commands);

        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        // "Pro" (not favorite) is the initial selection; switch to "Perso"
        // (favorite) before toggling.
        harness.get_by_value("Pro").click();
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Perso")
            .click();
        harness.run();

        harness.get_by_label("★ Favori").click();
        harness.run();

        let commands = &harness.state().config.commands;
        let pro = commands.iter().find(|c| c.id == "sync-pro").unwrap();
        let perso = commands.iter().find(|c| c.id == "sync-perso").unwrap();
        assert!(
            !pro.is_favorite,
            "toggling the selected variant's favorite must not affect an unselected sibling"
        );
        assert!(
            !perso.is_favorite,
            "the selected variant's own favorite state must have flipped"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Automations view smoke test (Phase 3 acceptance criterion, updated
    // for Part 3 Phase 2's real `crate::linux::automations::fetch()` data
    // source) -----------------------------------------------------------
    //
    // "renders without panicking on Linux" — this drives the real nav
    // switch and asserts the real `automations_view::fetch()` -> render
    // path completes without panicking. This reference machine has real
    // systemd timers configured (see `crate::linux::automations`'s own
    // real-machine tests), so this asserts the populated-grid path (a
    // real row's name shows up in the rendered UI) rather than the old
    // "always empty, always show the not-yet-available placeholder" stub
    // behavior.

    #[test]
    fn automations_view_renders_real_systemd_rows_without_panicking_on_linux() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "automations-shell"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let config = sample_config();
        let app = EguiApp::new_for_test(config, config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.run();
        harness.get_by_label("Automatisations").click();
        harness.run();

        let rows = harness
            .state()
            .automations
            .as_ref()
            .expect("Automations view must have fetched on first render")
            .as_ref()
            .expect("fetch() must not error out on Linux — real systemctl call");
        assert!(
            !rows.is_empty(),
            "expected real systemd timers on this reference system"
        );

        let first_name = rows[0].name.clone();
        assert!(
            harness.query_by_label_contains(&first_name).is_some(),
            "expected the first real timer's name ({first_name:?}) to appear in the rendered grid"
        );

        // Rafraîchir must also not panic on a second real fetch.
        harness.get_by_label("Rafraîchir").click();
        harness.run();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_application_generation_is_ignored_and_current_one_settles() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-applications-generation",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = EguiApp::new_for_test(sample_config(), dir.join("config.json"));
        app.application_generation = 2;
        app.application_loading = true;
        app.application_tx
            .send(ReportEvent {
                generation: 1,
                result: Ok(sample_application_report()),
            })
            .unwrap();
        app.drain_application_events();
        assert!(app.application_report.is_none());
        assert!(app.application_loading);

        app.application_tx
            .send(ReportEvent {
                generation: 2,
                result: Ok(sample_application_report()),
            })
            .unwrap();
        app.drain_application_events();
        assert_eq!(app.application_report.as_ref().unwrap().candidates.len(), 1);
        assert!(!app.application_loading);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_refresh_keeps_the_last_application_report() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-applications-error",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = EguiApp::new_for_test(sample_config(), dir.join("config.json"));
        app.application_report = Some(sample_application_report());
        app.application_generation = 3;
        app.application_loading = true;
        app.application_tx
            .send(ReportEvent {
                generation: 3,
                result: Err("Python indisponible".to_string()),
            })
            .unwrap();

        app.drain_application_events();

        assert_eq!(app.application_report.as_ref().unwrap().candidates.len(), 1);
        assert_eq!(
            app.application_error.as_deref(),
            Some("Python indisponible")
        );
        assert!(!app.application_loading);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn applications_view_filters_selects_copies_and_refreshes() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-applications-view",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = EguiApp::new_for_test(sample_config(), dir.join("config.json"));
        app.application_report = Some(sample_application_report());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Applications").click();
        harness.run();
        assert!(harness.query_by_label_contains("Editor").is_some());

        harness.get_by_label("Recherche").focus();
        harness.run();
        harness.get_by_label("Recherche").type_text("absente");
        harness.run();
        assert!(harness.query_by_label_contains("Editor").is_none());

        harness.state_mut().application_filters.search.clear();
        harness.run();
        harness.get_by_label("Editor").click();
        harness.run();
        harness.get_by_label("Copier la commande").click();
        harness.run_steps(1);
        assert!(harness.output().platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == "sudo apt-get remove -- editor")
        }));

        harness.get_by_label("Rafraîchir").click();
        harness.run_steps(1);
        assert_eq!(harness.state().application_generation, 1);
        assert!(harness.state().application_loading);
        let _ = std::fs::remove_dir_all(dir);
    }
}
