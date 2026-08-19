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

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use eframe::egui;

use crate::applications::{
    self, RecommendationReport, ReportEvent, SystemProcessProvider, UsageService,
};
use crate::cleanup::{self, CleanupEvent, ModuleRow, Payload};
use crate::icons::backend::IconBackend;
use crate::icons::egui_backend::EguiIconBackend;
#[cfg(not(target_os = "linux"))]
use crate::icons::resolve_icon;
use crate::icons::{decode_resize_file, icons_dirs, IconResolution};
use crate::storage::{self, CommandResolution, Config, MachineCommands, StorageError};
use crate::ui::applications_view::{self, ApplicationFilters};
use crate::ui::automations_view::{self, AutomationRow};
use crate::ui::cleanup_view::{self, CleanupAction, CleanupViewState, LastRun};
use crate::ui::command_form::CommandFormWidget;
use crate::ui::dialogs::{self, DialogKind, DialogOutcome};
use crate::ui::icon_picker;
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
/// if it is configured and no other command is already in flight — card
/// launch, Terminal command or cleanup run alike (one command at a time,
/// see [`EguiApp::command_busy`]). Kept as a free function, independent of
/// `EguiApp`, so the concurrency guard is directly unit-testable without
/// building a full app/harness.
fn can_launch_card(is_configured: bool, command_busy: bool) -> bool {
    is_configured && !command_busy
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
                    let group_name = first
                        .group_name
                        .clone()
                        .unwrap_or_else(|| first.name.clone());
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
        let cards = partition_by_variant_group(&commands, overrides, machine_id, |c| c.is_favorite);
        vec![DisplayGroup {
            header: None,
            cards,
        }]
    }
}

/// Owned, per-category snapshot of a category (or the synthetic "Sans
/// catégorie" bucket, `category: None`) plus its commands, used purely for
/// rendering the unified Préférences view (Part 3 Phase 1) — decoupled from
/// `storage::Config` for the same reason as `build_display_groups`/
/// `CardData`: the render loop opens edit/delete requests via click
/// handlers on `&mut self`, so it must never hold a live borrow of
/// `self.config` while doing so.
#[derive(Clone, Debug, PartialEq)]
struct PreferencesGroup {
    category: Option<storage::Category>,
    rows: Vec<PreferencesRow>,
}

/// A single renderable row inside a [`PreferencesGroup`]: either one
/// ungrouped command, or a whole variant group collapsed into one row —
/// mirroring how [`partition_by_variant_group`] collapses the same
/// commands into a single card in the Actions view. Préférences must show
/// exactly one line per app (per the user's request), not one per variant;
/// per-variant editing happens by expanding the row (see
/// `EguiApp::expanded_groups`).
#[derive(Clone, Debug, PartialEq)]
enum PreferencesRow {
    Single(storage::Command),
    Group {
        key: String,
        group_name: String,
        icon: String,
        variants: Vec<storage::Command>,
    },
}

/// Partitions `commands` into [`PreferencesRow`]s, preserving first-seen
/// order — same [`PartitionKey`] bucketing as `partition_by_variant_group`,
/// adapted to owned `storage::Command` data instead of `CardData`.
fn partition_preferences_rows(commands: Vec<storage::Command>) -> Vec<PreferencesRow> {
    let mut order: Vec<PartitionKey> = Vec::new();
    let mut buckets: HashMap<PartitionKey, Vec<storage::Command>> = HashMap::new();
    for c in commands {
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
            let members = buckets.remove(&key)?;
            match key {
                PartitionKey::Single(_) => members.into_iter().next().map(PreferencesRow::Single),
                PartitionKey::Group(group_key) => {
                    let first = members.first()?;
                    let group_name = first
                        .group_name
                        .clone()
                        .unwrap_or_else(|| first.name.clone());
                    let icon = first.icon.clone();
                    Some(PreferencesRow::Group {
                        key: group_key,
                        group_name,
                        icon,
                        variants: members,
                    })
                }
            }
        })
        .collect()
}

/// Every declared category (in `config.categories` order) plus a trailing
/// synthetic "Sans catégorie" bucket when orphan commands exist — mirrors
/// `storage::group_commands_by_category` exactly, just with owned data,
/// then collapses variant groups into single rows via
/// [`partition_preferences_rows`].
fn preferences_groups(config: &Config) -> Vec<PreferencesGroup> {
    storage::group_commands_by_category(config)
        .into_iter()
        .map(|group| PreferencesGroup {
            category: group.category.cloned(),
            rows: partition_preferences_rows(group.commands.into_iter().cloned().collect()),
        })
        .collect()
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

/// Action-form scratch state (Part 3 Phase 1 task 1) — one form shared by
/// both creation and editing. `editing_id` is `None` while composing a new
/// action and `Some(command_id)` while editing an existing one (the id
/// itself is immutable across an edit — see `EguiApp::try_submit_action_form`).
/// Covers every editable field: `name`, executable+arguments (via Part 2's
/// `command_form` widget), `category` (dropdown id, `""` = "Sans
/// catégorie"), `icon` (via Part 2's `icon_picker` widget), `is_favorite`,
/// and `shortcut`.
struct ActionForm {
    editing_id: Option<String>,
    name: String,
    command_widget: CommandFormWidget,
    category: String,
    icon: String,
    is_favorite: bool,
    shortcut: String,
}

impl ActionForm {
    fn new() -> Self {
        Self {
            editing_id: None,
            name: String::new(),
            command_widget: CommandFormWidget::new(),
            category: String::new(),
            icon: String::new(),
            is_favorite: false,
            shortcut: String::new(),
        }
    }

    /// Prefill from an existing command (Phase 2 task 2 — "Modifier").
    fn from_command(command: &storage::Command) -> Self {
        Self {
            editing_id: Some(command.id.clone()),
            name: command.name.clone(),
            command_widget: CommandFormWidget::from_command(&command.command),
            category: command.category.clone(),
            icon: command.icon.clone(),
            is_favorite: command.is_favorite,
            shortcut: command.shortcut.clone().unwrap_or_default(),
        }
    }
}

enum CategoryAction {
    Add,
    Rename {
        id: String,
        new_name: String,
    },
    Remove {
        id: String,
    },
    Move {
        id: String,
        direction: storage::MoveDirection,
    },
}

/// Which top-level view the nav row has selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ActiveView {
    #[default]
    Actions,
    Terminal,
    Automations,
    Cleanup,
    Preferences,
}

/// What the background `clean.py` thread is currently doing, if anything —
/// `Analyze` drives the Bibliothèques spinner, `Clean` names the module so
/// a failure message can cite it. Either state makes [`EguiApp::command_busy`]
/// true.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CleanupJob {
    Analyze,
    Clean(String),
}

/// An action deferred behind a confirm dialog — applied by
/// [`EguiApp::resolve_pending_action`] only once the user picks "Oui". Every
/// variant is destructive by design (this enum only ever gates destructive
/// actions behind a blocking confirm): the `Remove*` variants delete config
/// entries, `CleanModule` deletes a cache from disk.
enum PendingAction {
    RemoveCategory(String),
    RemoveCommand(String),
    RemoveCommandGroup(String),
    CleanModule(String),
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
    /// `None` when no action create/edit form is open — the Préférences
    /// view then shows only a "+ Nouvelle action" button (Part 3 Phase 1/2).
    action_form: Option<ActionForm>,
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
    /// `variant_group` keys currently expanded in the Préférences view (Part
    /// 3 Phase 3) — a collapsed group row shows one line per app; expanding
    /// it reveals each variant so its options/arguments can be edited
    /// individually. Session-only, never persisted.
    expanded_groups: HashSet<String>,
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
    /// `None` when no `clean.py` run is in flight; see [`CleanupJob`].
    cleanup_job: Option<CleanupJob>,
    /// `None` before the first successful analysis.
    cleanup_rows: Option<Vec<ModuleRow>>,
    cleanup_error: Option<String>,
    /// Rows survive a failed re-analysis but are flagged as coming from the
    /// last successful plan.
    cleanup_stale: bool,
    /// Guards against a stale thread's event overwriting a newer run's
    /// state — same pattern as `application_generation`.
    cleanup_generation: u64,
    cleanup_tx: Sender<CleanupEvent>,
    cleanup_rx: Receiver<CleanupEvent>,
    /// Last `--apply` outcome per module name, feeding the row badges and
    /// the measured-size refresh (no full re-analysis after a clean).
    cleanup_last_runs: HashMap<String, LastRun>,
    /// Same test-gating pattern as `report_spawning_enabled`: kittest
    /// harness tests never spawn a real python process.
    cleanup_spawning_enabled: bool,
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
        let (cleanup_tx, cleanup_rx) = std::sync::mpsc::channel();
        Self {
            config,
            config_path,
            icon_backend,
            icon_cache: HashMap::new(),
            category_form: CategoryForm::default(),
            action_form: None,
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
            expanded_groups: HashSet::new(),
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
            cleanup_job: None,
            cleanup_rows: None,
            cleanup_error: None,
            cleanup_stale: false,
            cleanup_generation: 0,
            cleanup_tx,
            cleanup_rx,
            cleanup_last_runs: HashMap::new(),
            cleanup_spawning_enabled: report_spawning_enabled,
            machine_commands,
            machine_id,
        }
    }

    /// The single-command-slot guard (brief decision: one external command
    /// at a time, whatever launched it — Actions card, Terminal, or a
    /// `clean.py` run).
    fn command_busy(&self) -> bool {
        self.action_running.is_some() || self.terminal_running || self.cleanup_job.is_some()
    }

    fn start_cleanup_analysis(&mut self) {
        self.cleanup_generation = self.cleanup_generation.saturating_add(1);
        self.cleanup_job = Some(CleanupJob::Analyze);
        if self.cleanup_spawning_enabled {
            cleanup::spawn_analyze(self.cleanup_generation, self.cleanup_tx.clone());
        }
    }

    /// Opens the blocking confirm dialog for `Nettoyer` on a module row —
    /// the spawn itself only happens in `resolve_pending_action` on "Oui".
    fn request_clean_module(&mut self, module: String) {
        let Some(row) = self
            .cleanup_rows
            .as_ref()
            .and_then(|rows| rows.iter().find(|row| row.module == module))
        else {
            return;
        };
        let size = cleanup_view::display_size(
            row,
            self.cleanup_last_runs.get(&module).map(|last| &last.result),
        );
        let mut message = format!("Supprimer le cache « {} » ({size}) ?", row.module);
        const MAX_PATHS: usize = 3;
        for path in row.paths.iter().take(MAX_PATHS) {
            message.push_str("\n• ");
            message.push_str(path);
        }
        if row.paths.len() > MAX_PATHS {
            message.push_str(&format!(
                "\n… et {} autres chemins",
                row.paths.len() - MAX_PATHS
            ));
        }
        if row.needs_network {
            message.push_str("\nRe-téléchargement requis pour reconstituer ce cache.");
        }
        self.active_dialog = Some(ActiveDialog {
            kind: dialogs::confirm("Nettoyer ce module ?", message),
            on_confirm: Some(PendingAction::CleanModule(module)),
        });
    }

    fn drain_cleanup_events(&mut self) {
        let mut events = Vec::new();
        while let Ok(event) = self.cleanup_rx.try_recv() {
            events.push(event);
        }
        for event in events {
            if event.generation != self.cleanup_generation {
                continue;
            }
            let finished_job = self.cleanup_job.take();
            match event.result {
                Ok(Payload::Plan(plan)) => {
                    self.cleanup_rows = Some(cleanup::module_rows(&plan));
                    self.cleanup_error = None;
                    self.cleanup_stale = false;
                }
                Ok(Payload::Applied { run, .. }) => {
                    let interrupted = run.is_interrupted();
                    for result in run.results {
                        self.cleanup_last_runs.insert(
                            result.module.clone(),
                            LastRun {
                                result,
                                interrupted,
                            },
                        );
                    }
                    if interrupted {
                        self.set_status("Nettoyage interrompu avant la fin.", true);
                    }
                }
                Err(error) => match finished_job {
                    Some(CleanupJob::Clean(module)) => {
                        self.set_status(format!("Échec du nettoyage de {module} : {error}"), true);
                    }
                    _ => {
                        self.cleanup_error = Some(error);
                        if self.cleanup_rows.is_some() {
                            self.cleanup_stale = true;
                        }
                    }
                },
            }
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
    /// command with no matching per-machine mapping entry (Part 3). Favorite
    /// management lives exclusively in Préférences now, not here — see
    /// `render_preferences_view`'s per-action favorite toggle.
    fn render_card(&mut self, ui: &mut egui::Ui, card: &CardData) {
        if card.variants.is_empty() {
            self.render_simple_card(ui, card);
        } else {
            self.render_grouped_card(ui, card);
        }
    }

    fn render_simple_card(&mut self, ui: &mut egui::Ui, card: &CardData) {
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
                                    let display_size =
                                        egui::vec2(48.0, 48.0).min(size.max(egui::vec2(1.0, 1.0)));
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
            });
        });

        // `add_enabled_ui` already suppresses `clicked()` for an
        // unconfigured card, so `card.is_configured` is redundant with
        // `body_clicked` here — kept explicit as the same guard
        // `can_launch_card` exposes for its pure unit test.
        if body_clicked && can_launch_card(card.is_configured, self.command_busy()) {
            self.launch_command(&card.command_id, &card.command);
        }
    }

    /// Grouped card: a `ComboBox` selects among `card.variants` (session
    /// state in `self.selected_variant`, keyed by `card.group_name`) and an
    /// explicit "Lancer" button launches the selected variant's resolved
    /// command — unlike a simple card, the body itself is not clickable, so
    /// interacting with the dropdown never risks an accidental launch.
    fn render_grouped_card(&mut self, ui: &mut egui::Ui, card: &CardData) {
        let visual = self.icon_visual(&card.icon);
        let group_key = card
            .group_name
            .clone()
            .unwrap_or_else(|| card.command_id.clone());
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
        let selected_disabled_message = selected.disabled_message.clone();

        let mut requested_variant: Option<String> = None;
        let mut launch_clicked = false;

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
                    can_launch_card(selected_is_configured, self.command_busy()),
                    |ui| {
                        if ui.button("Lancer").clicked() {
                            launch_clicked = true;
                        }
                    },
                );
            });
        });

        if let Some(variant_id) = requested_variant {
            self.selected_variant.insert(group_key, variant_id);
        }
        if launch_clicked && can_launch_card(selected_is_configured, self.command_busy()) {
            self.launch_command(&selected_command_id, &selected_command);
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

    /// Favorite management lives in Préférences (moved off the Actions
    /// view's cards per user request) — this is its sole entry point now.
    fn apply_toggle_favorite(&mut self, command_id: &str) {
        match storage::toggle_favorite(&mut self.config, command_id) {
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

    fn apply_category_action(&mut self, action: CategoryAction) {
        // Removal must go through a blocking confirm dialog rather than
        // acting immediately (Part 2 plan Risk register item 3 / this
        // phase's acceptance criterion). It's only reachable on an empty
        // category — the "Supprimer" button is disabled otherwise (manual
        // click-through feedback: no orphan-rebucketing fallback in the UI)
        // — but this is re-checked here defensively since the UI gate alone
        // doesn't guarantee every caller of this method goes through it.
        if let CategoryAction::Remove { id } = &action {
            let has_commands = self.config.commands.iter().any(|c| c.category == *id);
            if has_commands {
                self.set_status(
                    "Impossible de supprimer une catégorie qui contient encore des actions.",
                    true,
                );
                return;
            }
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
                    format!("Supprimer la catégorie « {label} » ?"),
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
            CategoryAction::Move { id, direction } => {
                storage::move_category(&mut self.config, id, *direction)
            }
            CategoryAction::Remove { .. } => unreachable!("handled by the early return above"),
        };

        match result {
            Ok(()) => {
                if matches!(action, CategoryAction::Add) {
                    self.category_form = CategoryForm::default();
                }
                match self.persist() {
                    Ok(()) => {
                        let message = if matches!(action, CategoryAction::Move { .. }) {
                            "Ordre des catégories mis à jour."
                        } else {
                            "Catégorie mise à jour."
                        };
                        self.set_status(message, false)
                    }
                    Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
                }
            }
            Err(err) => self.set_status(err.to_string(), true),
        }
    }

    /// Reorder an action within its category bucket — mirrors
    /// `apply_category_action`'s `CategoryAction::Move` arm, but there is no
    /// confirm dialog to guard (moves are always reversible/non-destructive).
    fn apply_move_command(&mut self, id: &str, direction: storage::MoveDirection) {
        match storage::move_command(&mut self.config, id, direction) {
            Ok(()) => match self.persist() {
                Ok(()) => self.set_status("Ordre des actions mis à jour.", false),
                Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
            },
            Err(err) => self.set_status(err.to_string(), true),
        }
    }

    /// Move a whole variant group's block (all its variants together)
    /// relative to sibling categories/groups/singles — mirrors
    /// `apply_move_command`, delegating to `storage::move_command_group`.
    fn apply_move_command_group(&mut self, key: &str, direction: storage::MoveDirection) {
        match storage::move_command_group(&mut self.config, key, direction) {
            Ok(()) => match self.persist() {
                Ok(()) => self.set_status("Ordre des actions mis à jour.", false),
                Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
            },
            Err(err) => self.set_status(err.to_string(), true),
        }
    }

    /// Move a single variant within its own group — mirrors
    /// `apply_move_command`, delegating to `storage::move_variant`.
    fn apply_move_variant(&mut self, id: &str, direction: storage::MoveDirection) {
        match storage::move_variant(&mut self.config, id, direction) {
            Ok(()) => match self.persist() {
                Ok(()) => self.set_status("Ordre des variantes mis à jour.", false),
                Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
            },
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
            PendingAction::RemoveCommand(id) => {
                match storage::remove_command(&mut self.config, &id) {
                    Ok(()) => match self.persist() {
                        Ok(()) => {
                            self.set_status("Action supprimée.", false);
                            // If the deleted action was open in the edit
                            // form, close the form rather than leaving it
                            // pointed at a now-nonexistent id.
                            if self
                                .action_form
                                .as_ref()
                                .and_then(|form| form.editing_id.as_deref())
                                == Some(id.as_str())
                            {
                                self.action_form = None;
                            }
                        }
                        Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
                    },
                    Err(err) => self.set_status(err.to_string(), true),
                }
            }
            PendingAction::RemoveCommandGroup(key) => {
                match storage::remove_command_group(&mut self.config, &key) {
                    Ok(_removed) => match self.persist() {
                        Ok(()) => {
                            self.set_status("Application supprimée.", false);
                            self.expanded_groups.remove(&key);
                            // If the deleted group's edit form was open on
                            // one of its now-removed variants, close it
                            // rather than leaving it pointed at a
                            // nonexistent id.
                            if let Some(editing_id) = self
                                .action_form
                                .as_ref()
                                .and_then(|form| form.editing_id.as_deref())
                            {
                                if !self.config.commands.iter().any(|c| c.id == editing_id) {
                                    self.action_form = None;
                                }
                            }
                        }
                        Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
                    },
                    Err(err) => self.set_status(err.to_string(), true),
                }
            }
            PendingAction::CleanModule(module) => {
                // Re-check the single-command slot: another command may have
                // started while the dialog was open (risk register, part 3).
                if self.command_busy() {
                    self.set_status("Une autre commande est en cours — nettoyage annulé.", true);
                    return;
                }
                self.cleanup_generation = self.cleanup_generation.saturating_add(1);
                self.cleanup_job = Some(CleanupJob::Clean(module.clone()));
                if self.cleanup_spawning_enabled {
                    cleanup::spawn_clean(&module, self.cleanup_generation, self.cleanup_tx.clone());
                }
            }
        }
    }

    /// Open the blocking confirm dialog guarding an action deletion (Phase 2
    /// task 3) — mirrors `apply_category_action`'s `CategoryAction::Remove`
    /// branch. The actual `storage::remove_command` call only happens in
    /// `resolve_pending_action` once the user picks "Oui".
    fn request_remove_command(&mut self, id: String, name: String) {
        self.active_dialog = Some(ActiveDialog {
            kind: dialogs::confirm(
                "Supprimer l'action",
                format!("Supprimer l'action « {name} » ? Cette opération est irréversible."),
            ),
            on_confirm: Some(PendingAction::RemoveCommand(id)),
        });
    }

    /// Open the blocking confirm dialog guarding a whole variant group's
    /// deletion — mirrors `request_remove_command`, but removes every
    /// variant in the group at once via `storage::remove_command_group`.
    fn request_remove_command_group(&mut self, key: String, group_name: String, count: usize) {
        self.active_dialog = Some(ActiveDialog {
            kind: dialogs::confirm(
                "Supprimer l'application",
                format!(
                    "Supprimer « {group_name} » et ses {count} variantes ? Cette opération est irréversible."
                ),
            ),
            on_confirm: Some(PendingAction::RemoveCommandGroup(key)),
        });
    }

    /// Validate and submit the current action form (Phase 2 tasks 1/2).
    ///
    /// Hard gate (non-optional per plan): `command_widget.is_valid()` is
    /// checked *first* — if it fails, no `add_command`/`update_command`
    /// call is made and nothing is written to disk; the widget's own inline
    /// error (rendered by `command_form::CommandFormWidget::show`) stays
    /// visible because `form` is simply put back into `self.action_form`
    /// unchanged. The same "put the form back, never discard in-progress
    /// input" rule applies to every other failure path below (empty name,
    /// storage error, persist error) — see this lot's risk register item 2.
    fn try_submit_action_form(&mut self, form: ActionForm) {
        if !form.command_widget.is_valid() {
            self.action_form = Some(form);
            return;
        }
        let Some(command_str) = form.command_widget.recomposed() else {
            self.action_form = Some(form);
            return;
        };

        let name = form.name.trim().to_string();
        if name.is_empty() {
            self.set_status("Le nom de l'action est requis.", true);
            self.action_form = Some(form);
            return;
        }

        let shortcut = {
            let trimmed = form.shortcut.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        };

        match &form.editing_id {
            None => {
                let existing_ids: Vec<String> =
                    self.config.commands.iter().map(|c| c.id.clone()).collect();
                let id = storage::generate_slug(&name, &existing_ids);
                let command = storage::Command {
                    id,
                    name,
                    command: command_str,
                    category: form.category.clone(),
                    icon: form.icon.clone(),
                    is_favorite: form.is_favorite,
                    shortcut,
                    variant_group: None,
                    group_name: None,
                    variant_label: None,
                    machine_specific: false,
                };
                match storage::add_command(&mut self.config, command) {
                    Ok(()) => match self.persist() {
                        Ok(()) => {
                            self.set_status("Action créée.", false);
                            self.action_form = None;
                        }
                        Err(err) => {
                            self.set_status(format!("Échec de sauvegarde: {err}"), true);
                            self.action_form = Some(form);
                        }
                    },
                    Err(err) => {
                        self.set_status(err.to_string(), true);
                        self.action_form = Some(form);
                    }
                }
            }
            Some(id) => {
                // `update_command` replaces the whole `Command` struct, so
                // fields not exposed by the form (variant_group/group_name/
                // variant_label/machine_specific) must be carried over from
                // the existing command rather than reset to defaults.
                let Some(existing) = self.config.commands.iter().find(|c| &c.id == id).cloned()
                else {
                    self.set_status("Action introuvable.", true);
                    self.action_form = Some(form);
                    return;
                };
                let updated = storage::Command {
                    id: id.clone(),
                    name,
                    command: command_str,
                    category: form.category.clone(),
                    icon: form.icon.clone(),
                    is_favorite: form.is_favorite,
                    shortcut,
                    variant_group: existing.variant_group,
                    group_name: existing.group_name,
                    variant_label: existing.variant_label,
                    machine_specific: existing.machine_specific,
                };
                match storage::update_command(&mut self.config, id, updated) {
                    Ok(()) => match self.persist() {
                        Ok(()) => {
                            self.set_status("Action mise à jour.", false);
                            self.action_form = None;
                        }
                        Err(err) => {
                            self.set_status(format!("Échec de sauvegarde: {err}"), true);
                            self.action_form = Some(form);
                        }
                    },
                    Err(err) => {
                        self.set_status(err.to_string(), true);
                        self.action_form = Some(form);
                    }
                }
            }
        }
    }

    /// Renders the action create/edit form (Phase 1 task 1, Phase 2 tasks
    /// 1/2). Takes full ownership of `self.action_form` for the duration of
    /// rendering (`Option::take`) so the ui closures below never need to
    /// borrow `self` as a whole — only `self.config` (read-only, for the
    /// category dropdown) — leaving `self.try_submit_action_form`/
    /// `self.action_form = ...` free to run afterwards without a borrow
    /// conflict.
    fn render_action_form(&mut self, ui: &mut egui::Ui) {
        let category_options: Vec<(String, String)> = self
            .config
            .categories
            .iter()
            .map(|c| (c.id.clone(), c.name.clone()))
            .collect();

        let Some(mut form) = self.action_form.take() else {
            if ui.button("+ Nouvelle action").clicked() {
                self.action_form = Some(ActionForm::new());
            }
            return;
        };

        // Self-healing category dropdown (risk register item 3): if the
        // category currently referenced by the form was deleted while the
        // form was open, fall back to "Sans catégorie" instead of showing
        // (or silently keeping) a dangling id.
        if !form.category.is_empty() && !category_options.iter().any(|(id, _)| id == &form.category)
        {
            form.category = String::new();
        }

        ui.label(if form.editing_id.is_some() {
            format!("Modifier l'action « {} »", form.name)
        } else {
            "Nouvelle action".to_string()
        });

        let name_label = ui.label("nom de l'action");
        ui.text_edit_singleline(&mut form.name)
            .labelled_by(name_label.id);

        ui.label("Commande");
        form.command_widget.show(ui);

        ui.horizontal(|ui| {
            ui.label("Catégorie");
            let selected_text = category_options
                .iter()
                .find(|(id, _)| id == &form.category)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| "Sans catégorie".to_string());
            egui::ComboBox::from_id_salt("action-form-category")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut form.category, String::new(), "Sans catégorie");
                    for (id, name) in &category_options {
                        ui.selectable_value(&mut form.category, id.clone(), name);
                    }
                });
        });

        ui.label("Icône");
        ui.push_id("action-icon-picker", |ui| {
            icon_picker::show(ui, &mut form.icon);
        });

        ui.checkbox(&mut form.is_favorite, "Favori");

        let shortcut_label = ui.label("raccourci");
        ui.text_edit_singleline(&mut form.shortcut)
            .labelled_by(shortcut_label.id);

        let mut submit_clicked = false;
        let mut cancel_clicked = false;
        ui.horizontal(|ui| {
            if ui.button("Enregistrer").clicked() {
                submit_clicked = true;
            }
            if ui.button("Annuler").clicked() {
                cancel_clicked = true;
            }
        });

        if cancel_clicked {
            self.action_form = None;
        } else if submit_clicked {
            self.try_submit_action_form(form);
        } else {
            self.action_form = Some(form);
        }
    }

    /// Dedicated Préférences view — moved out of `render_actions_view` (was
    /// a `CollapsingHeader` at the top of the Actions grid) so category
    /// management no longer eats into the vertical space available for
    /// action cards. Since Part 3 Phase 1: a unified view listing every
    /// category (plus the synthetic "Sans catégorie" bucket) with its
    /// actions inline, each carrying edit/delete controls, followed by the
    /// existing category-creation form and the action create/edit form.
    fn render_preferences_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Préférences");
        ui.separator();

        if let Some(status) = &self.status {
            let color = if status.is_error {
                egui::Color32::from_rgb(0xC4, 0x2B, 0x1C)
            } else {
                egui::Color32::from_rgb(0x1B, 0x5E, 0x20)
            };
            ui.colored_label(color, &status.text);
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut category_actions: Vec<CategoryAction> = Vec::new();
            let mut remove_command_request: Option<(String, String)> = None;
            let mut edit_command_request: Option<storage::Command> = None;
            let mut move_command_request: Option<(String, storage::MoveDirection)> = None;
            let mut toggle_favorite_request: Option<String> = None;
            let mut move_group_request: Option<(String, storage::MoveDirection)> = None;
            let mut remove_group_request: Option<(String, String, usize)> = None;
            let mut move_variant_request: Option<(String, storage::MoveDirection)> = None;
            let mut toggle_group_expand_request: Option<String> = None;

            let groups = preferences_groups(&self.config);

            ui.label("Catégories");
            for group in &groups {
                ui.separator();
                match &group.category {
                    Some(category) => {
                        let is_empty = group.rows.is_empty();
                        // Display order is exactly `self.config.categories`'s
                        // order (there is no dedicated ordering field) — find
                        // this category's position to gate the move buttons
                        // at the boundaries.
                        let position = self
                            .config
                            .categories
                            .iter()
                            .position(|c| c.id == category.id);
                        let is_first = position == Some(0);
                        let is_last = position == Some(self.config.categories.len() - 1);
                        ui.horizontal(|ui| {
                            ui.label(&category.icon);
                            ui.strong(&category.name);
                            let buffer = self
                                .rename_buffers
                                .entry(category.id.clone())
                                .or_insert_with(|| category.name.clone());
                            ui.text_edit_singleline(buffer);
                            if ui.button("Renommer").clicked() && !buffer.trim().is_empty() {
                                category_actions.push(CategoryAction::Rename {
                                    id: category.id.clone(),
                                    new_name: buffer.clone(),
                                });
                            }
                            // Compact single-glyph move buttons rather than
                            // full-width "Monter"/"Descendre" text (reported
                            // as too massive/unreadable). ▲/▼ (U+25B2/25BC)
                            // aren't covered by egui's Proportional font
                            // fallback chain (Ubuntu-Light + NotoEmoji +
                            // emoji-icon-font) and rendered as empty boxes;
                            // ⬆/⬇ (U+2B06/2B07) are covered by NotoEmoji and
                            // do render. Hover text keeps the action
                            // discoverable/accessible.
                            if ui
                                .add_enabled(!is_first, egui::Button::new("⬆"))
                                .on_hover_text("Monter")
                                .clicked()
                            {
                                category_actions.push(CategoryAction::Move {
                                    id: category.id.clone(),
                                    direction: storage::MoveDirection::Up,
                                });
                            }
                            if ui
                                .add_enabled(!is_last, egui::Button::new("⬇"))
                                .on_hover_text("Descendre")
                                .clicked()
                            {
                                category_actions.push(CategoryAction::Move {
                                    id: category.id.clone(),
                                    direction: storage::MoveDirection::Down,
                                });
                            }
                            // Deleting a category is only offered once it holds
                            // no actions — the user asked to remove the
                            // orphan-rebucketing path entirely from the UI
                            // rather than delete-with-fallback (deviation from
                            // the originally approved spec, confirmed via
                            // manual click-through).
                            let remove_button =
                                ui.add_enabled(is_empty, egui::Button::new("Supprimer"));
                            let remove_button = if is_empty {
                                remove_button
                            } else {
                                remove_button.on_disabled_hover_text(
                                    "Retirez ou déplacez d'abord les actions de cette catégorie",
                                )
                            };
                            if remove_button.clicked() {
                                category_actions.push(CategoryAction::Remove {
                                    id: category.id.clone(),
                                });
                            }
                        });
                    }
                    None => {
                        // Synthetic "Sans catégorie" bucket: no rename/delete
                        // controls at the category level (Phase 1 task 3), but
                        // its individual actions below keep the normal controls.
                        ui.horizontal(|ui| {
                            ui.label("📂");
                            ui.strong("Sans catégorie");
                        });
                    }
                }

                if group.rows.is_empty() {
                    ui.label("  (aucune action)");
                }
                let last_row_index = group.rows.len().saturating_sub(1);
                for (row_index, row) in group.rows.iter().enumerate() {
                    match row {
                        PreferencesRow::Single(command) => {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.label(&command.icon);
                                ui.label(&command.name);
                                // Favorite management lives here now (moved
                                // off the Actions view's cards per user
                                // request) — the star itself is the toggle,
                                // filled when favorite.
                                let star_label = if command.is_favorite { "★" } else { "☆" };
                                if ui.button(star_label).on_hover_text("Favori").clicked() {
                                    toggle_favorite_request = Some(command.id.clone());
                                }
                                if ui.button("Modifier").clicked() {
                                    edit_command_request = Some(command.clone());
                                }
                                // Compact single-glyph move buttons, same
                                // rationale as the category Monter/Descendre
                                // buttons above.
                                if ui
                                    .add_enabled(row_index != 0, egui::Button::new("⬆"))
                                    .on_hover_text("Monter")
                                    .clicked()
                                {
                                    move_command_request =
                                        Some((command.id.clone(), storage::MoveDirection::Up));
                                }
                                if ui
                                    .add_enabled(
                                        row_index != last_row_index,
                                        egui::Button::new("⬇"),
                                    )
                                    .on_hover_text("Descendre")
                                    .clicked()
                                {
                                    move_command_request =
                                        Some((command.id.clone(), storage::MoveDirection::Down));
                                }
                                if ui.button("Supprimer").clicked() {
                                    remove_command_request =
                                        Some((command.id.clone(), command.name.clone()));
                                }
                            });
                        }
                        PreferencesRow::Group {
                            key,
                            group_name,
                            icon,
                            variants,
                        } => {
                            let is_expanded = self.expanded_groups.contains(key);
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                let toggle_label = if is_expanded { "▾" } else { "▸" };
                                if ui
                                    .button(toggle_label)
                                    .on_hover_text("Afficher/masquer les variantes")
                                    .clicked()
                                {
                                    toggle_group_expand_request = Some(key.clone());
                                }
                                ui.label(icon.as_str());
                                ui.strong(group_name.as_str());
                                if ui
                                    .add_enabled(row_index != 0, egui::Button::new("⬆"))
                                    .on_hover_text("Monter")
                                    .clicked()
                                {
                                    move_group_request =
                                        Some((key.clone(), storage::MoveDirection::Up));
                                }
                                if ui
                                    .add_enabled(
                                        row_index != last_row_index,
                                        egui::Button::new("⬇"),
                                    )
                                    .on_hover_text("Descendre")
                                    .clicked()
                                {
                                    move_group_request =
                                        Some((key.clone(), storage::MoveDirection::Down));
                                }
                                if ui.button("Supprimer").clicked() {
                                    remove_group_request =
                                        Some((key.clone(), group_name.clone(), variants.len()));
                                }
                            });
                            if is_expanded {
                                let last_variant_index = variants.len().saturating_sub(1);
                                for (variant_index, variant) in variants.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.add_space(32.0);
                                        ui.label(&variant.icon);
                                        let label = variant
                                            .variant_label
                                            .clone()
                                            .unwrap_or_else(|| variant.name.clone());
                                        ui.label(label);
                                        let star_label =
                                            if variant.is_favorite { "★" } else { "☆" };
                                        if ui.button(star_label).on_hover_text("Favori").clicked() {
                                            toggle_favorite_request = Some(variant.id.clone());
                                        }
                                        if ui.button("Modifier").clicked() {
                                            edit_command_request = Some(variant.clone());
                                        }
                                        if ui
                                            .add_enabled(variant_index != 0, egui::Button::new("⬆"))
                                            .on_hover_text("Monter")
                                            .clicked()
                                        {
                                            move_variant_request = Some((
                                                variant.id.clone(),
                                                storage::MoveDirection::Up,
                                            ));
                                        }
                                        if ui
                                            .add_enabled(
                                                variant_index != last_variant_index,
                                                egui::Button::new("⬇"),
                                            )
                                            .on_hover_text("Descendre")
                                            .clicked()
                                        {
                                            move_variant_request = Some((
                                                variant.id.clone(),
                                                storage::MoveDirection::Down,
                                            ));
                                        }
                                        if ui.button("Supprimer").clicked() {
                                            remove_command_request =
                                                Some((variant.id.clone(), variant.name.clone()));
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }

            for action in category_actions {
                self.apply_category_action(action);
            }
            if let Some((id, name)) = remove_command_request {
                self.request_remove_command(id, name);
            }
            if let Some(command) = edit_command_request {
                self.action_form = Some(ActionForm::from_command(&command));
            }
            if let Some((id, direction)) = move_command_request {
                self.apply_move_command(&id, direction);
            }
            if let Some(id) = toggle_favorite_request {
                self.apply_toggle_favorite(&id);
            }
            if let Some((key, direction)) = move_group_request {
                self.apply_move_command_group(&key, direction);
            }
            if let Some((key, group_name, count)) = remove_group_request {
                self.request_remove_command_group(key, group_name, count);
            }
            if let Some((id, direction)) = move_variant_request {
                self.apply_move_variant(&id, direction);
            }
            if let Some(key) = toggle_group_expand_request {
                if !self.expanded_groups.remove(&key) {
                    self.expanded_groups.insert(key);
                }
            }

            ui.separator();
            let mut add_category_clicked = false;
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

                ui.label("icône");
                ui.push_id("category-icon-picker", |ui| {
                    icon_picker::show(ui, &mut self.category_form.icon);
                });
                if ui.button("Ajouter").clicked() {
                    add_category_clicked = true;
                }
            });
            if add_category_clicked {
                self.apply_category_action(CategoryAction::Add);
            }

            ui.separator();
            self.render_action_form(ui);
        });
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
        self.drain_cleanup_events();
        if self.cleanup_job.is_some() {
            // Same polling rationale as `terminal_running` below: the
            // cleanup thread's events only land on a repaint.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
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
            ui.selectable_value(&mut self.active_view, ActiveView::Cleanup, "Nettoyage");
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
            ActiveView::Cleanup => self.render_cleanup_view(ui),
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
                        self.render_card(ui, card);
                    }
                });
            }
        });
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
            self.status = Some(
                match outcome.expect("settled implies a terminal event was seen") {
                    Ok(()) => StatusMessage {
                        text: format!("'{command_id}' lancé avec succès."),
                        is_error: false,
                    },
                    Err(err) => StatusMessage {
                        text: format!("Échec du lancement de '{command_id}': {err}"),
                        is_error: true,
                    },
                },
            );
        }
    }

    fn render_terminal_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("DevToolBox — Terminal");

        ui.horizontal(|ui| {
            let command_label = ui.label("commande");
            ui.text_edit_singleline(&mut self.terminal_input)
                .labelled_by(command_label.id);

            if ui
                .add_enabled(!self.command_busy(), egui::Button::new("Lancer"))
                .clicked()
            {
                self.launch_terminal_command();
            }
        });

        // Busy indicator on its own line, in the same green as success
        // status messages so it stands out from the form. Both launch paths
        // land their output here, so both drive it: `terminal_running`
        // (input field above) and `action_running` (Actions card).
        if self.terminal_running || self.action_running.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.colored_label(
                    egui::Color32::from_rgb(0x1B, 0x5E, 0x20),
                    "commande en cours d'exécution…",
                );
            });
        }
        ui.separator();

        // `both()`: command output lines don't wrap, so long lines would be
        // clipped past the right edge without a horizontal scrollbar.
        egui::ScrollArea::both()
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
        ui.label("Automatisations créées par l'utilisateur ou un logiciel tiers (les tâches/timers fournis par l'OS sont masqués).");

        if let Some(status) = &self.status {
            let color = if status.is_error {
                egui::Color32::from_rgb(0xC4, 0x2B, 0x1C)
            } else {
                egui::Color32::from_rgb(0x1B, 0x5E, 0x20)
            };
            ui.colored_label(color, &status.text);
        }

        if self.automations.is_none() {
            self.automations = Some(automations_view::fetch());
        }

        ui.horizontal(|ui| {
            if ui.button("Rafraîchir").clicked() {
                self.automations = Some(automations_view::fetch());
            }
            if ui.button("Ouvrir l'outil natif").clicked() {
                if let Err(err) = automations_view::open_native_tool() {
                    self.set_status(err, true);
                }
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
                // `both()`: the row count and the TaskPath-style category
                // column can both exceed the window; grid cells don't wrap,
                // so without scrollbars the overflow is clipped invisibly.
                egui::ScrollArea::both()
                    .id_salt("automations-scroll")
                    .show(ui, |ui| {
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
                    });
            }
        }
    }

    /// « Nettoyage » view: the « Bibliothèques » section on top — its rows
    /// only exist after an explicit Analyser click, so they must own the
    /// first screenful — the installed-apps report below.
    fn render_cleanup_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("DevToolBox — Nettoyage");

        let state = CleanupViewState {
            rows: self.cleanup_rows.as_deref(),
            error: self.cleanup_error.as_deref(),
            analyzing: matches!(self.cleanup_job, Some(CleanupJob::Analyze)),
            busy: self.command_busy(),
            last_runs: &self.cleanup_last_runs,
            stale: self.cleanup_stale,
        };
        let actions = cleanup_view::render(ui, &state);
        for action in actions {
            match action {
                CleanupAction::Analyze => self.start_cleanup_analysis(),
                CleanupAction::Clean(module) => self.request_clean_module(module),
            }
        }

        ui.separator();
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
    use egui_kittest::{
        kittest::{NodeT, Queryable},
        Harness,
    };
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
            variant_command(
                "sftp-pro",
                "sftp-sync",
                "Synchroniser",
                "Pro",
                "sync.sh pro",
                "system",
                true,
            ),
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
            variant_command(
                "sftp-tout",
                "sftp-sync",
                "Synchroniser",
                "Tout",
                "sync.sh tout",
                "system",
                true,
            ),
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

        // Favorite management now lives in Préférences (moved off the
        // Actions view's cards). "cmd" is the only non-favorite command in
        // `sample_config`, so its toggle is the sole "☆" button on screen.
        harness.run();
        harness.get_by_label("Préférences").click();
        harness.run();
        harness.get_by_label("☆").click();
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

        // The icon field is no longer free text (Part 3 Phase 1 task 4):
        // it's the curated `icon_picker` widget, now a popup opened via its
        // trigger button (shows "Choisir…" while unset) — open it, then
        // pick a tile by its glyph label.
        harness.get_by_label("Choisir…").click();
        harness.run();
        harness.get_by_label("🌐").click();
        harness.run();

        harness.get_by_label("Ajouter").click_accesskit();
        harness.run();

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        assert!(
            reloaded
                .categories
                .iter()
                .any(|c| c.id == "net" && c.name == "Réseau" && c.icon == "🌐"),
            "category added via simulated UI did not persist to disk; categories={:?}",
            reloaded.categories
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_descendre_on_a_category_swaps_it_with_its_next_neighbor_and_persists() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "cat-reorder"
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

        // Inject a second, empty category so there's a neighbor to swap
        // with — `sample_config()` only ships "system" on its own.
        harness.state_mut().config.categories.push(Category {
            id: "temp".into(),
            name: "Temporaire".into(),
            icon: "🗂️".into(),
        });
        harness.run();

        assert_eq!(
            harness
                .state()
                .config
                .categories
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["system", "temp"],
            "sanity check: system should render before temp"
        );

        // "system" renders first, so its "⬇" is the first one in the tree.
        harness
            .get_all_by_label("⬇")
            .next()
            .expect("system category's ⬇ button should be present")
            .click();
        harness.run();

        assert_eq!(
            harness
                .state()
                .config
                .categories
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["temp", "system"],
            "clicking Descendre on system must swap it past temp"
        );

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        assert_eq!(
            reloaded
                .categories
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["temp", "system"],
            "reordering must persist to disk"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_descendre_on_an_action_swaps_it_with_its_next_neighbor_in_the_same_category_and_persists(
    ) {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "cmd-reorder"
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

        assert_eq!(
            harness
                .state()
                .config
                .commands
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["notepad", "cmd"],
            "sanity check: notepad should render before cmd"
        );

        // Render order for "⬇" labels: [0] the "system" category's own
        // (disabled, sole category so is_last), [1] notepad's (enabled,
        // first of two commands), [2] cmd's (disabled, last in its bucket)
        // — click index 1.
        harness
            .get_all_by_label("⬇")
            .nth(1)
            .expect("notepad action's ⬇ button should be present")
            .click();
        harness.run();

        assert_eq!(
            harness
                .state()
                .config
                .commands
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["cmd", "notepad"],
            "clicking Descendre on notepad must swap it past cmd"
        );

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        assert_eq!(
            reloaded
                .commands
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["cmd", "notepad"],
            "reordering must persist to disk"
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

        // sample_config's only category ("system") holds two commands, so
        // its "Supprimer" button is disabled (category delete is now
        // UI-gated to empty categories only — manual click-through
        // feedback). Push a second, empty category directly into state to
        // exercise the confirm-dialog flow on a deletable category.
        harness.state_mut().config.categories.push(Category {
            id: "temp".into(),
            name: "Temporaire".into(),
            icon: "🗂️".into(),
        });

        harness.run();
        harness.get_by_label("Préférences").click();
        harness.run();

        // Render order is: "system" category row (disabled Supprimer),
        // then its two command rows (each with their own enabled
        // Supprimer), then the empty "temp" category row (enabled
        // Supprimer) — so the fourth "Supprimer" label is "temp"'s.
        harness
            .get_all_by_label("Supprimer")
            .nth(3)
            .expect("the empty 'Temporaire' category's Supprimer button should be present")
            .click();
        harness.run();

        // The confirm dialog is now up and should block everything behind
        // it: the Préférences view's "Catégories" heading, its "Renommer"
        // buttons — nothing but the dialog itself is rendered this frame.
        assert!(
            harness.query_by_label("Oui").is_some(),
            "confirm dialog should be showing an Oui button"
        );
        assert!(
            harness.query_by_label("Non").is_some(),
            "confirm dialog should be showing a Non button"
        );
        assert!(
            harness.query_by_label("Renommer").is_none(),
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
            2,
            "canceling the confirm dialog must leave categories unchanged in memory"
        );
        assert!(
            !config_path.exists(),
            "canceling must never write to disk (nothing was persisted before the cancel)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_a_non_empty_category_is_blocked_and_the_supprimer_button_is_disabled() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "cat-remove-non-empty"
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

        // "system" holds notepad + cmd — its Supprimer button must be
        // disabled and clicking it must not open the confirm dialog.
        let system_delete = harness
            .get_all_by_label("Supprimer")
            .next()
            .expect("system category's Supprimer button should be present");
        assert!(
            system_delete.accesskit_node().is_disabled(),
            "Supprimer must be disabled for a category that still has actions"
        );
        system_delete.click();
        harness.run();

        assert!(
            harness.query_by_label("Oui").is_none(),
            "clicking a disabled Supprimer must not open the confirm dialog"
        );
        assert_eq!(
            harness.state().config.categories.len(),
            1,
            "the category must still be present"
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

        // "system" holds commands, so its Supprimer is disabled — push an
        // empty category directly into state to exercise a real deletion
        // (see the sibling cancel test's comment for the render-order
        // rationale behind `.nth(3)`).
        harness.state_mut().config.categories.push(Category {
            id: "temp".into(),
            name: "Temporaire".into(),
            icon: "🗂️".into(),
        });

        harness.run();
        harness.get_by_label("Préférences").click();
        harness.run();
        harness
            .get_all_by_label("Supprimer")
            .nth(3)
            .expect("the empty 'Temporaire' category's Supprimer button should be present")
            .click();
        harness.run();
        harness.get_by_label("Oui").click();
        harness.run();

        assert_eq!(
            harness.state().config.categories.len(),
            1,
            "confirming removal must update in-memory state, leaving only 'system'"
        );
        assert!(
            harness
                .state()
                .config
                .categories
                .iter()
                .any(|c| c.id == "system"),
            "the non-empty 'system' category must remain untouched"
        );

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        assert_eq!(
            reloaded.categories.len(),
            1,
            "confirmed removal via the real UI did not persist to disk; categories={:?}",
            reloaded.categories
        );
        assert!(!reloaded.categories.iter().any(|c| c.id == "temp"));
        assert_eq!(
            reloaded.commands.len(),
            2,
            "removing an empty category must not affect commands in other categories"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Action CRUD tests (Part 3 Phase 2 acceptance criteria) ---------------
    //
    // Drive the REAL unified Préférences view end-to-end: create/edit/delete
    // an action via the real form widgets (`command_form`/`icon_picker`), the
    // real confirm-dialog gate on delete, and the real validity gate on
    // submit. `egui::ComboBox` popups are opened via `Role::ComboBox` (not
    // `get_by_value`) because the category dropdown's current-value text can
    // collide with the rename text field's own current value, which is also
    // exposed as a `value` accesskit attribute.

    #[test]
    fn creating_an_action_via_the_real_ui_persists_and_appears_immediately_in_actions_view() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "action-create"
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
        harness.get_by_label("+ Nouvelle action").click();
        harness.run();

        harness.get_by_label("nom de l'action").focus();
        harness.run();
        harness
            .get_by_label("nom de l'action")
            .type_text("Calculatrice");
        harness.run();

        harness.get_by_label("Exécutable").focus();
        harness.run();
        harness.get_by_label("Exécutable").type_text("calc.exe");
        harness.run();

        // A space-containing argument, per the Phase 2 manual acceptance
        // criterion ("a space-containing path argument").
        harness.get_by_label("+ Argument").click();
        harness.run();
        harness.get_by_label("Argument").focus();
        harness.run();
        harness.get_by_label("Argument").type_text("start now");
        harness.run();

        // Reassign from the default "Sans catégorie" to the sample
        // config's one real category.
        harness.get_by_role(egui::accesskit::Role::ComboBox).click();
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Système")
            .click();
        harness.run();

        // Two icon_picker widgets exist here (the category-add form's and
        // the open action form's), both still unset ("Choisir…") — the
        // action form's trigger button renders second (see
        // `render_preferences_view` ordering), so it's the second matching
        // node in tree order. Only one popup is open at a time, so once
        // it's opened its own "🚀" tile is unambiguous.
        harness
            .get_all_by_label("Choisir…")
            .nth(1)
            .expect(
                "action form's icon_picker trigger button should be the second 'Choisir…' button",
            )
            .click();
        harness.run();
        harness.get_by_label("🚀").click();
        harness.run();

        harness.get_by_label("Favori").click();
        harness.run();

        harness.get_by_label("raccourci").focus();
        harness.run();
        harness.get_by_label("raccourci").type_text("Ctrl+K");
        harness.run();

        harness.get_by_label("Enregistrer").click_accesskit();
        harness.run();

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        let created = reloaded
            .commands
            .iter()
            .find(|c| c.name == "Calculatrice")
            .expect("new action persisted to disk");
        assert_eq!(created.command, "calc.exe \"start now\"");
        assert_eq!(created.category, "system");
        assert_eq!(created.icon, "🚀");
        assert!(created.is_favorite);
        assert_eq!(created.shortcut.as_deref(), Some("Ctrl+K"));

        // Appears immediately in the Actions view, no restart.
        harness.get_by_label("Actions").click();
        harness.run();
        assert!(
            harness.query_by_label("Calculatrice").is_some(),
            "newly created action should render immediately in the Actions view"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editing_an_action_via_the_real_ui_prefills_the_form_and_persists_changes() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "action-edit"
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
        // Action buttons are plain "Modifier"/"Supprimer" (Part 3 follow-up
        // — the item name to their left is already visible, so the name
        // suffix was redundant). "system" lists notepad then cmd, so
        // notepad's "Modifier" is the first match.
        harness
            .get_all_by_label("Modifier")
            .next()
            .expect("notepad's Modifier button should be present")
            .click();
        harness.run();

        {
            let form = harness
                .state()
                .action_form
                .as_ref()
                .expect("edit form should be open after clicking Modifier");
            assert_eq!(form.editing_id.as_deref(), Some("notepad"));
            assert_eq!(form.name, "Bloc-notes");
            assert_eq!(form.command_widget.rows(), &["notepad.exe".to_string()]);
            assert_eq!(form.category, "system");
            assert_eq!(form.icon, "📝");
            assert!(form.is_favorite);
            assert_eq!(form.shortcut, "");
        }

        harness.get_by_label("raccourci").focus();
        harness.run();
        harness.get_by_label("raccourci").type_text("Ctrl+Shift+N");
        harness.run();

        // Reassign to "Sans catégorie" (risk register item 3's flip side —
        // the dropdown must let a currently-categorized action move out).
        harness.get_by_role(egui::accesskit::Role::ComboBox).click();
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Sans catégorie")
            .click();
        harness.run();

        // Toggle is_favorite off (it started as true — sample_config's
        // "notepad").
        harness.get_by_label("Favori").click();
        harness.run();

        harness.get_by_label("Enregistrer").click_accesskit();
        harness.run();

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        let updated = reloaded
            .commands
            .iter()
            .find(|c| c.id == "notepad")
            .expect("edited action still present after save");
        assert_eq!(updated.name, "Bloc-notes");
        assert_eq!(updated.command, "notepad.exe");
        assert_eq!(
            updated.category, "",
            "reassigning to Sans catégorie must persist as an empty category field"
        );
        assert_eq!(updated.shortcut.as_deref(), Some("Ctrl+Shift+N"));
        assert!(
            !updated.is_favorite,
            "toggling favori off via the action form must persist"
        );

        // Reopening the edit form after persisting must show the just-saved
        // values, not the stale pre-edit ones. notepad moved to "Sans
        // catégorie" (rendered after "system"), so cmd's "Modifier" is now
        // first and notepad's is second.
        harness
            .get_all_by_label("Modifier")
            .nth(1)
            .expect("notepad's Modifier button should be present after reassignment")
            .click();
        harness.run();
        let reopened = harness
            .state()
            .action_form
            .as_ref()
            .expect("edit form should reopen");
        assert_eq!(reopened.shortcut, "Ctrl+Shift+N");
        assert!(!reopened.is_favorite);
        assert_eq!(reopened.category, "");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_an_action_is_gated_by_a_blocking_confirm_dialog_and_persists_once_confirmed() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "action-remove"
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
        // Action buttons are plain "Modifier"/"Supprimer" (Part 3
        // follow-up). "system" lists notepad then cmd; the category's own
        // (disabled) "Supprimer" is first, notepad's second, cmd's third.
        harness
            .get_all_by_label("Supprimer")
            .nth(2)
            .expect("cmd's Supprimer button should be present")
            .click();
        harness.run();

        assert!(
            harness.query_by_label("Oui").is_some(),
            "confirm dialog should be showing an Oui button"
        );
        assert!(
            harness.query_by_label("Non").is_some(),
            "confirm dialog should be showing a Non button"
        );
        assert_eq!(
            harness.query_all_by_label("Supprimer").count(),
            0,
            "background action list must not render while the confirm dialog is active"
        );

        harness.get_by_label("Non").click();
        harness.run();

        assert_eq!(
            harness.state().config.commands.len(),
            2,
            "canceling the confirm dialog must leave commands unchanged in memory"
        );
        assert!(
            !config_path.exists(),
            "canceling must never write to disk (nothing was persisted before the cancel)"
        );

        harness
            .get_all_by_label("Supprimer")
            .nth(2)
            .expect("cmd's Supprimer button should be present")
            .click();
        harness.run();
        harness.get_by_label("Oui").click();
        harness.run();

        assert_eq!(
            harness.state().config.commands.len(),
            1,
            "confirming removal must update in-memory state"
        );
        assert!(!harness
            .state()
            .config
            .commands
            .iter()
            .any(|c| c.id == "cmd"));

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        assert!(
            !reloaded.commands.iter().any(|c| c.id == "cmd"),
            "confirmed removal via the real UI did not persist to disk; commands={:?}",
            reloaded.commands
        );
        assert_eq!(reloaded.commands.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn submitting_an_invalid_command_blocks_the_storage_write_and_preserves_form_input() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "action-invalid"
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
        harness.get_by_label("+ Nouvelle action").click();
        harness.run();

        harness.get_by_label("nom de l'action").focus();
        harness.run();
        harness.get_by_label("nom de l'action").type_text("Cassée");
        harness.run();

        harness.get_by_label("Exécutable").focus();
        harness.run();
        harness.get_by_label("Exécutable").type_text("echo");
        harness.run();

        // A row containing a literal `"` can never round-trip through
        // `terminal_view::tokenize` (see `command_form`'s module docs) —
        // this must block the storage write entirely.
        harness.get_by_label("+ Argument").click();
        harness.run();
        harness.get_by_label("Argument").focus();
        harness.run();
        harness.get_by_label("Argument").type_text("say\"hi");
        harness.run();

        assert!(
            !harness
                .state()
                .action_form
                .as_ref()
                .unwrap()
                .command_widget
                .is_valid(),
            "precondition: the row containing a literal quote must be invalid"
        );

        harness.get_by_label("Enregistrer").click_accesskit();
        harness.run();

        assert_eq!(
            harness.state().config.commands.len(),
            2,
            "an invalid command must not be added to the in-memory config"
        );
        assert!(
            !config_path.exists(),
            "an invalid command must never be written to disk"
        );

        let form = harness
            .state()
            .action_form
            .as_ref()
            .expect("form must stay open (not be discarded) on validation failure");
        assert_eq!(
            form.name, "Cassée",
            "in-progress name must be preserved on validation failure"
        );
        assert_eq!(
            form.command_widget.rows(),
            &["echo".to_string(), "say\"hi".to_string()]
        );

        // The widget's own inline error stays visible.
        assert!(harness.query_by_label_contains("guillemet").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn category_forms_icon_picker_preserves_an_out_of_set_value_when_the_view_is_open() {
        // Phase 1 acceptance criterion 3: an out-of-curated-set icon value
        // must display unchanged, not be silently reset, when the
        // category-add form (which now uses `icon_picker` rather than free
        // text) is rendered.
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "cat-icon-out-of-set"
        ));
        let config_path = dir.join("config.json");

        let config = sample_config();
        let mut app = EguiApp::new_for_test(config, config_path);
        app.category_form.icon = "🦄".to_string();
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

        assert_eq!(
            harness.state().category_form.icon,
            "🦄",
            "an out-of-curated-set icon value must not be silently reset when the \
             Préférences view (and its icon_picker) renders"
        );

        // The icon_picker is now a popup (fix #2): the "hors liste" note
        // only renders once the trigger button (labeled with the current
        // out-of-set value itself) is clicked open.
        harness.get_by_label("🦄").click();
        harness.run();
        assert!(
            harness
                .query_by_label_contains("Icône actuelle (hors liste)")
                .is_some(),
            "the out-of-set value should still be displayed, per icon_picker's own contract"
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
    fn can_launch_card_blocks_any_click_while_another_command_is_in_flight() {
        assert!(
            !can_launch_card(true, true),
            "a configured card must not be launchable while another command is running"
        );
        assert!(
            can_launch_card(true, false),
            "a configured card must be launchable when no command is in flight"
        );
        assert!(
            !can_launch_card(false, false),
            "an unconfigured card must never be launchable, even with no command in flight"
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
            saw_success =
                saw_success
                    || harness.state().status.as_ref().is_some_and(|status| {
                        !status.is_error && status.text.contains("echo-card")
                    });
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
            saw_success =
                saw_success
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

        // Favorite management now lives in Préférences (moved off the
        // Actions view's cards). A variant group collapses into a single
        // row (Phase 3), so its variants — and their favorite stars — are
        // only reachable once the row is expanded. "sync-perso" starts as
        // the only favorite, so its row is the sole "★" button once expanded.
        harness.run();
        harness.get_by_label("Préférences").click();
        harness.run();
        harness.get_by_label("▸").click();
        harness.run();
        harness.get_by_label("★").click();
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

    // -- Préférences variant-group consolidation (Phase 3): a command's
    // `variant_group` is what makes the Actions view collapse it into one
    // card (`partition_by_variant_group`) — Préférences must mirror that
    // exactly, showing one row per app rather than one per variant, per the
    // user's screenshot request: "pour une même application, il faudrait
    // que ce soit une seule ligne […] et ensuite dans modifier, pouvoir
    // gérer les options/arguments." --------------------------------------

    #[test]
    fn preferences_view_collapses_a_variant_group_into_a_single_row() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "prefs-group-collapse"
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
                false,
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
        harness.get_by_label("Préférences").click();
        harness.run();

        // A single "Synchroniser" (group_name) row, not one per variant —
        // the per-variant labels ("Pro"/"Perso") must not appear until the
        // row is expanded.
        harness.get_by_label("Synchroniser");
        assert!(
            harness.query_by_label("Pro").is_none(),
            "variant rows must stay hidden until the group row is expanded"
        );
        assert!(harness.query_by_label("Perso").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expanding_a_group_row_reveals_each_variant_for_editing() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "prefs-group-expand-edit"
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
                false,
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
        harness.get_by_label("Préférences").click();
        harness.run();
        harness.get_by_label("▸").click();
        harness.run();

        // Both variants are now visible as their own sub-rows, each with
        // its own "Modifier" — clicking "Perso"'s must open the edit form
        // pointed at "sync-perso" specifically (options/arguments editing
        // per variant, the user's actual ask), not at the group as a whole.
        harness.get_by_label("Pro");
        harness.get_by_label("Perso");
        harness
            .get_all_by_label("Modifier")
            .nth(1)
            .expect("the second Modifier button belongs to the 'Perso' variant row")
            .click();
        harness.run();

        assert_eq!(
            harness
                .state()
                .action_form
                .as_ref()
                .and_then(|form| form.editing_id.clone()),
            Some("sync-perso".to_string()),
            "Modifier on a variant sub-row must open the edit form for that exact variant"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_group_row_removes_every_variant_after_confirmation() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "prefs-group-delete"
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
                false,
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
        harness.get_by_label("Préférences").click();
        harness.run();
        // "system" (the category holding the group) is non-empty, so its
        // own "Supprimer" is disabled and renders first; the group row's
        // "Supprimer" is the second match.
        harness
            .get_all_by_label("Supprimer")
            .nth(1)
            .expect("the group row's Supprimer button should be present")
            .click();
        harness.run();
        harness.get_by_label("Oui").click();
        harness.run();

        assert!(
            harness.state().config.commands.is_empty(),
            "confirming a group deletion must remove every variant, not just one"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Automations view smoke test (Phase 3 acceptance criterion, updated
    // for Part 3 Phase 2's real `crate::linux::automations::fetch()` data
    // source, and again for the user-scope filter added afterwards) ------
    //
    // "renders without panicking on Linux" — this drives the real nav
    // switch and asserts the real `automations_view::fetch()` -> render
    // path completes without panicking. This reference machine's real
    // systemd timers are all package-provided (see
    // `crate::linux::automations`'s own real-machine tests), so after the
    // user-scope filter the *correct* result here is an empty, filtered
    // list — this asserts the empty-state placeholder renders instead of
    // requiring a populated grid, but still exercises the populated-row
    // render path when rows are present (e.g. on a machine with a real
    // local timer under `/etc/systemd/system`), so the assertion stays
    // meaningful either way.
    //
    // Deliberately does not click "Ouvrir l'outil natif": on Linux that
    // spawns a real `gnome-terminal` window (see
    // `crate::linux::automations::open_native_tool`) — a real side effect
    // this smoke test shouldn't trigger on every run. Its presence in the
    // rendered view is asserted instead.

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

        if rows.is_empty() {
            assert!(
                harness
                    .query_by_label_contains("Aucune automatisation trouvée")
                    .is_some(),
                "expected the empty-state placeholder when the user-scope filter leaves zero rows"
            );
        } else {
            let first_name = rows[0].name.clone();
            assert!(
                harness.query_by_label_contains(&first_name).is_some(),
                "expected the first real timer's name ({first_name:?}) to appear in the rendered grid"
            );
        }

        assert!(
            harness
                .query_by_label_contains("Ouvrir l'outil natif")
                .is_some(),
            "expected the native-tool link button to render"
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
        harness.get_by_label("Nettoyage").click();
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

    // -- Part 3: cleanup wiring ---------------------------------------------

    fn sample_cleanup_plan() -> crate::cleanup::CleanupPlan {
        crate::cleanup::CleanupPlan {
            level: "moderate".into(),
            apply: false,
            candidates: vec![
                crate::cleanup::Candidate {
                    module: "npm".into(),
                    path: Some("C:/Users/x/AppData/Local/npm-cache".into()),
                    label: "cache npm".into(),
                    estimated_bytes: Some(4_600_000_000),
                    level: "safe".into(),
                    needs_network: true,
                },
                crate::cleanup::Candidate {
                    module: "recycle".into(),
                    path: None,
                    label: "corbeille".into(),
                    estimated_bytes: Some(1_000_000),
                    level: "moderate".into(),
                    needs_network: false,
                },
            ],
            total_estimated_bytes: Some(4_601_000_000),
            unpriced_modules: vec![],
            warnings: vec![],
        }
    }

    fn cleanup_test_app(name: &str) -> (EguiApp, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("devtoolbox-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let app = EguiApp::new_for_test(sample_config(), dir.join("config.json"));
        (app, dir)
    }

    #[test]
    fn analyser_sets_job_and_drains_plan_event_into_rows() {
        let (app, dir) = cleanup_test_app("cleanup-analyze");
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Nettoyage").click();
        harness.run();
        harness.get_by_label("Analyser").click();
        harness.run_steps(1);
        assert_eq!(harness.state().cleanup_job, Some(CleanupJob::Analyze));
        assert_eq!(harness.state().cleanup_generation, 1);

        // A stale event from a previous generation must be ignored.
        harness
            .state()
            .cleanup_tx
            .send(CleanupEvent {
                generation: 0,
                result: Err("stale".into()),
            })
            .unwrap();
        harness.run_steps(1);
        assert_eq!(harness.state().cleanup_job, Some(CleanupJob::Analyze));
        assert!(harness.state().cleanup_error.is_none());

        harness
            .state()
            .cleanup_tx
            .send(CleanupEvent {
                generation: 1,
                result: Ok(Payload::Plan(sample_cleanup_plan())),
            })
            .unwrap();
        harness.run();
        assert!(harness.state().cleanup_job.is_none());
        let rows = harness.state().cleanup_rows.as_ref().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(harness.query_by_label("npm").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clean_opens_dialog_and_spawns_only_on_oui() {
        let (mut app, dir) = cleanup_test_app("cleanup-dialog");
        app.cleanup_rows = Some(cleanup::module_rows(&sample_cleanup_plan()));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Nettoyage").click();
        harness.run();
        // One Nettoyer button: only the safe npm row gets one, the moderate
        // recycle row is greyed without a button (part 2 contract).
        harness.get_by_label("Nettoyer").click();
        harness.run_steps(2);
        assert!(matches!(
            harness.state().active_dialog,
            Some(ActiveDialog {
                on_confirm: Some(PendingAction::CleanModule(_)),
                ..
            })
        ));
        assert!(harness.state().cleanup_job.is_none());

        // « Non » closes the dialog without starting anything.
        harness.get_by_label("Non").click();
        harness.run_steps(2);
        assert!(harness.state().active_dialog.is_none());
        assert!(harness.state().cleanup_job.is_none());
        assert_eq!(harness.state().cleanup_generation, 0);

        // « Oui » starts the clean job (spawning gated off in tests).
        harness.get_by_label("Nettoyer").click();
        harness.run_steps(2);
        harness.get_by_label("Oui").click();
        harness.run_steps(1);
        assert_eq!(
            harness.state().cleanup_job,
            Some(CleanupJob::Clean("npm".to_string()))
        );
        assert_eq!(harness.state().cleanup_generation, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn command_busy_gates_all_launch_paths_mutually() {
        let (mut app, dir) = cleanup_test_app("cleanup-busy");
        assert!(!app.command_busy());
        app.terminal_running = true;
        assert!(app.command_busy());
        app.terminal_running = false;
        app.action_running = Some("cmd".into());
        assert!(app.command_busy());
        app.action_running = None;
        app.cleanup_job = Some(CleanupJob::Analyze);
        assert!(app.command_busy());

        // Dialog confirm re-checks the slot: a command that started while
        // the dialog was open refuses the clean instead of double-running.
        app.cleanup_rows = Some(cleanup::module_rows(&sample_cleanup_plan()));
        app.cleanup_job = None;
        app.terminal_running = true;
        app.resolve_pending_action(PendingAction::CleanModule("npm".into()));
        assert!(app.cleanup_job.is_none());
        assert!(app.status.as_ref().is_some_and(|s| s.is_error));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_failure_apply_event_lands_in_last_runs_with_measured_size() {
        let (mut app, dir) = cleanup_test_app("cleanup-partial");
        app.cleanup_rows = Some(cleanup::module_rows(&sample_cleanup_plan()));
        app.cleanup_generation = 2;
        app.cleanup_job = Some(CleanupJob::Clean("npm".into()));
        app.cleanup_tx
            .send(CleanupEvent {
                generation: 2,
                result: Ok(Payload::Applied {
                    plan: sample_cleanup_plan(),
                    run: crate::cleanup::RunPayload {
                        status: "completed".into(),
                        results: vec![crate::cleanup::ModuleResult {
                            module: "npm".into(),
                            estimated: Some(4_600_000_000),
                            freed: Some(4_000_000_000),
                            failed: Some(600_000_000),
                            measured: Some(600_000_000),
                            locked_paths: vec!["C:/locked".into()],
                            operation_failures: vec![],
                        }],
                    },
                }),
            })
            .unwrap();

        app.drain_cleanup_events();

        assert!(app.cleanup_job.is_none());
        let last = app.cleanup_last_runs.get("npm").unwrap();
        assert!(!last.interrupted);
        assert!(!last.result.is_success());
        assert_eq!(last.result.failure_count(), 1);
        // The refreshed size shown for the row comes from `measured`.
        let row = app
            .cleanup_rows
            .as_ref()
            .unwrap()
            .iter()
            .find(|row| row.module == "npm")
            .unwrap();
        assert_eq!(
            cleanup_view::display_size(row, Some(&last.result)),
            "572.2 Mio"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn interrupted_run_is_never_a_success() {
        let (mut app, dir) = cleanup_test_app("cleanup-interrupted");
        app.cleanup_generation = 1;
        app.cleanup_job = Some(CleanupJob::Clean("npm".into()));
        app.cleanup_tx
            .send(CleanupEvent {
                generation: 1,
                result: Ok(Payload::Applied {
                    plan: sample_cleanup_plan(),
                    run: crate::cleanup::RunPayload {
                        status: "interrupted".into(),
                        results: vec![crate::cleanup::ModuleResult {
                            module: "npm".into(),
                            estimated: Some(1),
                            freed: Some(1),
                            failed: None,
                            measured: Some(0),
                            locked_paths: vec![],
                            operation_failures: vec![],
                        }],
                    },
                }),
            })
            .unwrap();

        app.drain_cleanup_events();

        let last = app.cleanup_last_runs.get("npm").unwrap();
        assert!(last.interrupted);
        assert!(app.status.as_ref().is_some_and(|s| s.is_error));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_reanalysis_keeps_rows_and_marks_them_stale() {
        let (mut app, dir) = cleanup_test_app("cleanup-stale");
        app.cleanup_rows = Some(cleanup::module_rows(&sample_cleanup_plan()));
        app.cleanup_generation = 3;
        app.cleanup_job = Some(CleanupJob::Analyze);
        app.cleanup_tx
            .send(CleanupEvent {
                generation: 3,
                result: Err("python introuvable".into()),
            })
            .unwrap();

        app.drain_cleanup_events();

        assert!(app.cleanup_rows.is_some());
        assert!(app.cleanup_stale);
        assert_eq!(app.cleanup_error.as_deref(), Some("python introuvable"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
