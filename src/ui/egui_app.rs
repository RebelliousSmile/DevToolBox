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
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use eframe::egui;
use walkdir::{DirEntry, WalkDir};

use crate::applications::{
    self, RecommendationReport, ReportEvent, SystemProcessProvider, UsageService,
};
use crate::cleanup::{self, CleanupEvent, ModuleRow, Payload};
use crate::icons::backend::IconBackend;
use crate::icons::egui_backend::EguiIconBackend;
#[cfg(not(target_os = "linux"))]
use crate::icons::resolve_icon;
use crate::icons::{decode_resize_file, icons_dirs, IconResolution};
use crate::models::{
    self, AcquisitionOffer, CancelHandle, CatalogSnapshot, ModelSettings, ModelWorkerEvent,
    ProgressEvent,
};
use crate::storage::{self, CommandResolution, Config, MachineCommands, StorageError};
use crate::ui::applications_view::{self, ApplicationFilters};
use crate::ui::automations_view::{self, AutomationRow};
use crate::ui::cleanup_view::{self, CleanupAction, CleanupViewState, LastRun};
use crate::ui::command_form::CommandFormWidget;
use crate::ui::compose_view::{
    self, ComposeViewState, ScanOutcome, StackAction, StackEntry, StackState, StackTarget,
};
use crate::ui::dialogs::{self, DialogKind, DialogOutcome};
use crate::ui::docker_view::{
    self, BatchOutcome, BatchTarget, DockerAction, DockerList, DockerSnapshot, DockerViewState,
    SelectionKey,
};
use crate::ui::icon_picker;
use crate::ui::models_view::{self, ModelsAction, ModelsUiState, ModelsViewState};
use crate::ui::port_plan;
use crate::ui::terminal_view::{self, TerminalEvent};
use crate::ui::{components, native_window, theme};
use crate::update::service::UpdateState;

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
    /// The command's optional free-text note (`storage::Command::info`),
    /// surfaced in the card's "i" badge tooltip. Independent of
    /// `disabled_message`: both can be present, and the badge then shows
    /// them one after the other.
    info: Option<String>,
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
    info: Option<String>,
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

/// Human-readable tail line for a finished command. `code` is `None` when
/// the child was killed by a signal (Unix) rather than exiting on its own,
/// which `{code:?}` used to surface as the raw `Some(0)`/`None` debug form.
/// Kept as a free function so both drain loops share one wording and it is
/// directly unit-testable.
fn format_exit_line(code: Option<i32>) -> String {
    match code {
        Some(0) => "(terminé — succès)".to_string(),
        Some(code) => format!("(terminé — code {code})"),
        None => "(terminé — interrompu par un signal)".to_string(),
    }
}

/// Text shown in a card's "i" badge tooltip, or `None` when the card has
/// nothing to say. The unconfigured diagnostic comes first (it explains why
/// the card is greyed out), then the command's own free-text `info` note;
/// when both are present they are separated by a blank line. Kept as a free
/// function so the precedence is unit-testable without a harness.
fn badge_message(disabled_message: Option<&str>, info: Option<&str>) -> Option<String> {
    let info = info.map(str::trim).filter(|s| !s.is_empty());
    match (disabled_message, info) {
        (Some(disabled), Some(info)) => Some(format!("{disabled}\n\n{info}")),
        (Some(disabled), None) => Some(disabled.to_owned()),
        (None, Some(info)) => Some(info.to_owned()),
        (None, None) => None,
    }
}

/// Diameter of the circled "i" info badge, in points.
const BADGE_DIAMETER: f32 = 16.0;

/// Draws the small circled "i" badge that stands in for a card's inline
/// message: the text itself only shows as a hover tooltip, so a long
/// diagnostic (machine id + mapping file path) no longer stretches the card
/// into a wall of italics. Painted by hand rather than typeset as "ⓘ"/"ℹ"
/// because neither glyph is guaranteed to exist in egui's default font
/// stack, whereas an ASCII "i" drawn over a circle always renders.
///
/// Callers must place it *outside* any `add_enabled_ui(false)` scope: a
/// disabled response drops `on_hover_text`, which would silently swallow
/// the very message the badge exists to surface.
fn info_badge(ui: &mut egui::Ui, message: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(BADGE_DIAMETER, BADGE_DIAMETER),
        egui::Sense::hover(),
    );
    if ui.is_rect_visible(rect) {
        let color = ui.visuals().warn_fg_color;
        let painter = ui.painter();
        painter.circle_stroke(
            rect.center(),
            BADGE_DIAMETER / 2.0 - 1.0,
            egui::Stroke::new(1.0, color),
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "i",
            egui::FontId::proportional(11.0),
            color,
        );
    }
    // Keeps the badge queryable by its message in `egui_kittest` (and
    // readable by screen readers) even though nothing is typeset on screen.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, message));
    response.on_hover_text(message.to_owned())
}

/// Overlays [`info_badge`] on the top-right corner of an already-laid-out
/// card frame. Goes through `Ui::new_child` — which, unlike `scope_builder`,
/// does *not* advance the parent's cursor — so the badge costs zero layout
/// space: a card carrying a message keeps exactly the size it would have
/// without one, instead of gaining a row and shifting everything below it.
fn card_corner_badge(ui: &mut egui::Ui, card_rect: egui::Rect, message: &str) {
    const MARGIN: f32 = 3.0;
    let badge_rect = egui::Rect::from_min_size(
        egui::pos2(
            card_rect.right() - BADGE_DIAMETER - MARGIN,
            card_rect.top() + MARGIN,
        ),
        egui::vec2(BADGE_DIAMETER, BADGE_DIAMETER),
    );
    let mut badge_ui = ui.new_child(egui::UiBuilder::new().max_rect(badge_rect));
    info_badge(&mut badge_ui, message);
}

/// What [`render_card_shell`] reports back to its caller.
struct CardShell {
    /// Outer frame rect, for [`card_corner_badge`] to hang the info badge on.
    rect: egui::Rect,
    /// `true` when the icon+title block was clicked this frame. Always
    /// `false` while `body_enabled` is `false` — `add_enabled_ui` swallows
    /// clicks — but callers still re-check through [`can_launch_card`].
    body_clicked: bool,
}

/// The chrome every card shares: fixed-width frame, centered icon, title,
/// and the icon+title click target that launches the card. Simple and
/// variant-grouped cards differ only in what they add *below* the title, so
/// that difference is all `extra` carries — previously both bodies were
/// written out twice, which is why the grouped card silently missed the
/// clickable body the simple one had.
///
/// `extra` runs inside the frame but *outside* the `add_enabled_ui` scope,
/// so a grouped card's variant picker stays usable even when the selected
/// variant is unconfigured (you must be able to switch away from it).
fn render_card_shell(
    ui: &mut egui::Ui,
    visual: &IconVisual,
    title: &str,
    body_enabled: bool,
    extra: impl FnOnce(&mut egui::Ui),
) -> CardShell {
    let mut body_clicked = false;
    let frame = ui.group(|ui| {
        ui.set_width(96.0);
        ui.vertical_centered(|ui| {
            ui.add_enabled_ui(body_enabled, |ui| {
                // The whole icon+name block is the click target, not just
                // the name text — a text-only target left most of the card
                // (icon, padding) dead to clicks in real mouse use, even
                // though it passed kittest (which clicks the accessibility
                // node's rect, so a small target still gets hit).
                // `.interact(Sense::click())` upgrades the container
                // response's existing rect/id, and an explicit
                // `widget_info` keeps it discoverable via
                // `egui_kittest::Queryable::get_by_label(title)`.
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
                                // `selectable(false)`: a plain `ui.label` is
                                // selectable text by default, which gives it
                                // its own click+drag sense — that widget then
                                // wins hit-testing over the card's own
                                // `.interact(click)` below, silently eating
                                // every click.
                                ui.add(
                                    egui::Label::new(egui::RichText::new(text).size(28.0))
                                        .selectable(false),
                                );
                            }
                        }
                        ui.add(
                            egui::Label::new(egui::RichText::new(title).strong()).selectable(false),
                        );
                    })
                    .response
                    .interact(egui::Sense::click());
                body_response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title)
                });
                if body_response.clicked() {
                    body_clicked = true;
                }
            });

            extra(ui);
        });
    });

    CardShell {
        rect: frame.response.rect,
        body_clicked,
    }
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
                        info: first.info.clone(),
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
                                info: c.info.clone(),
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
                        info: None,
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
            dormant_after_days: 60,
            user_scripts_directory: String::new(),
            native_effects: true,
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
                info: None,
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
                info: None,
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
                info: None,
            },
        ],
        // Empty on purpose: a fallback config is what boots when the real one
        // could not be read, and inventing compose-file paths there would put
        // rows in the Docker tab that no scan ever produced.
        docker_stacks: Vec::new(),
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
/// `shortcut`, and `info` (free-text note surfaced as an "i" badge on the
/// card; empty means no badge).
struct ActionForm {
    editing_id: Option<String>,
    name: String,
    command_widget: CommandFormWidget,
    category: String,
    icon: String,
    is_favorite: bool,
    shortcut: String,
    info: String,
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
            info: String::new(),
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
            info: command.info.clone().unwrap_or_default(),
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
    Models,
    Docker,
    Preferences,
}

/// Sub-navigation within the Preferences workspace, mirroring the permanent
/// selectable sections used by the Models view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PreferencesSection {
    General,
    #[default]
    Actions,
    Terminal,
    Automations,
    Cleanup,
    Models,
    Docker,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UserScriptProposal {
    relative_path: PathBuf,
    selected: bool,
}

fn should_descend_into_script_entry(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() || entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !name.starts_with('.') && name != "__pycache__" && name != "node_modules"
}

fn scan_user_python_scripts(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.is_dir() {
        return Err(format!(
            "Dossier de scripts introuvable : {}",
            root.display()
        ));
    }
    let mut scripts = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_descend_into_script_entry)
    {
        let entry = entry.map_err(|error| format!("Scan impossible : {error}"))?;
        let path = entry.path();
        let is_python = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("py"));
        let is_package_marker = path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("__init__.py"));
        if entry.file_type().is_file() && is_python && !is_package_marker {
            if let Ok(relative) = path.strip_prefix(root) {
                scripts.push(relative.to_path_buf());
            }
        }
    }
    scripts.sort_by_key(|path| path.to_string_lossy().to_lowercase());
    Ok(scripts)
}

fn user_script_command(relative_path: &Path) -> String {
    let portable = relative_path.to_string_lossy().replace('\\', "/");
    format!("@python \"{portable}\"")
}

fn user_script_id(relative_path: &Path) -> String {
    let slug: String = relative_path
        .with_extension("")
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("user-script-{}", slug.trim_matches('-'))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelsJob {
    Inventory,
    Resolve,
    Download,
    SettingsRead,
    SettingsWrite,
    RecoveryRead,
    RecoveryWrite,
    GuidedWrite,
}

/// An action deferred behind a confirm dialog — applied by
/// [`EguiApp::resolve_pending_action`] only once the user picks "Oui". Every
/// variant is destructive by design (this enum only ever gates destructive
/// actions behind a blocking confirm): the `Remove*` variants delete config
/// entries, `CleanModule` deletes a cache from disk. The four `Docker`-tab
/// variants each carry the façade identifier (`id`/`reference`/volume
/// `name`) alongside a displayable name, since `resolve_pending_action`
/// needs both: the identifier to call the façade with, the name for the
/// "in progress" and result status messages.
enum PendingAction {
    InstallUpdate {
        manifest: crate::update::manifest::ReleaseManifest,
        asset: Box<crate::update::manifest::ReleaseAsset>,
    },
    RemoveCategory(String),
    RemoveCommand(String),
    RemoveCommandGroup(String),
    CleanModule(String),
    SaveModelSettings {
        root: String,
        provider_order: String,
        enabled_providers: String,
        xet_enabled: bool,
        keep_patterns: String,
    },
    RecoverModelOperation {
        operation_id: String,
        action: String,
    },
    GuideModel {
        artifact_id: String,
        destination: String,
        category: Option<String>,
    },
    StopContainer {
        id: String,
        name: String,
    },
    RemoveContainer {
        id: String,
        name: String,
    },
    RemoveImage {
        reference: String,
        name: String,
    },
    RemoveVolume(String),
    /// `docker compose down` — confirmed like every other action that
    /// destroys containers (`docker rm`, `docker rmi`). `up -d` and `stop`
    /// are not: neither destroys anything, and `stop` is undone by `up -d`.
    ComposeDown(StackTarget),
    /// The moves the confirm dialog described, carried rather than re-read
    /// from `docker_port_plan`: what gets written is what was shown.
    ApplyPortReassignment(Vec<crate::ui::port_plan::PortMove>),
    /// The whole batch, already ordered by `docker_view::order_targets`.
    /// Carries the targets rather than re-reading the selection on confirm:
    /// what the dialog described is exactly what runs, even though a refetch
    /// could have pruned the selection in between.
    DeleteSelection(Vec<BatchTarget>),
}

/// A confirmed Docker action stored by [`EguiApp::resolve_pending_action`]
/// instead of being run immediately — see the plan's Phase 3 task 4: a
/// blocking `docker` call made inline while handling the confirm dialog's
/// "Oui" click would freeze the UI before that frame's "Arrêt de …"/
/// "Suppression de …" status text ever reaches the screen (nothing is
/// presented until the whole frame finishes building). Storing the action
/// here lets that frame finish rendering (`ui_content` falls through
/// instead of returning early — see its `DialogOutcome::Accepted` arm) so
/// the status is actually painted, then [`EguiApp::execute_deferred_docker_action`]
/// runs the real, potentially slow call at the very start of the next
/// frame.
enum DeferredDockerAction {
    StopContainer {
        id: String,
        name: String,
    },
    RemoveContainer {
        id: String,
        name: String,
    },
    RemoveImage {
        reference: String,
        name: String,
    },
    RemoveVolume {
        name: String,
    },
    /// Not gated by a confirm dialog (it's not destructive) — stashed here
    /// directly by `render_docker_view` for the same reason as the other
    /// variants: `docker system df -v` takes ~5s on this machine, and running
    /// it inline would freeze the UI before the "Calcul…" status text paints.
    ComputeVolumeSizes,
    /// Deferred for the same reason as `ComputeVolumeSizes` and not because
    /// it is destructive (it reads two console tools and writes nothing):
    /// `netstat -ano` plus `tasklist` take ~1 s on this machine, long enough
    /// to drop a frame if run inline.
    ScanHostPorts,
    /// The only deferred action that writes to a file the user owns. Deferred
    /// like the rest so the « Réattribution… » status paints before the reads,
    /// backups and writes happen — and gated by `docker_actions_enabled`, so
    /// no kittest run can ever touch a real compose file.
    ApplyPortReassignment(Vec<crate::ui::port_plan::PortMove>),
    /// One `docker` call per target, so this is the slowest deferred action
    /// of them all — all the more reason for it to run after its "Suppression
    /// de N ressource(s)…" status has painted.
    DeleteSelection(Vec<BatchTarget>),
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
    native_profile: native_window::NativeProfile,
    native_effect_warning_logged: bool,
    update_state: UpdateState,
    update_rx: Option<Receiver<UpdateState>>,
    update_auto_checked: bool,
    preferences_section: PreferencesSection,
    user_script_proposals: Vec<UserScriptProposal>,
    user_script_scan_error: Option<String>,
    user_scripts_scanned: bool,
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
    models_ui: ModelsUiState,
    models_snapshot: Option<CatalogSnapshot>,
    models_offers: Vec<AcquisitionOffer>,
    models_progress: Vec<ProgressEvent>,
    models_recovery: Vec<models::LibraryJournal>,
    models_guided: Option<models::GuidedMigration>,
    models_terminal: Option<ProgressEvent>,
    models_error: Option<String>,
    models_job: Option<ModelsJob>,
    models_cancel: Option<CancelHandle>,
    models_tx: Sender<ModelWorkerEvent>,
    models_rx: Receiver<ModelWorkerEvent>,
    models_spawning_enabled: bool,
    /// Per-machine command overrides, loaded once at startup (Part 3) via
    /// `storage::load_machine_commands_from(platform::machine_commands_path())`.
    /// A missing file or load error both fall back to an empty map — this
    /// must never crash the app on startup.
    machine_commands: MachineCommands,
    /// This machine's id, resolved once at startup via `platform::machine_id()`
    /// and reused on every frame instead of re-resolving per `build_display_groups` call.
    machine_id: String,
    /// `true` when the Docker tab has a data source on this OS/machine at
    /// all — set once at startup via `docker_view::available()` and drives
    /// whether the "Docker" nav button is rendered at all (risk register:
    /// "tab button rendered only when `docker_available`"). Forced directly
    /// (private field, same module tree) in tests that need the tab either
    /// present or absent deterministically.
    docker_available: bool,
    /// `None` before the first fetch (successful or not) since the tab was
    /// last activated.
    docker: Option<Result<DockerSnapshot, String>>,
    /// Result channel for the Docker snapshot currently being collected.
    /// Docker Desktop can take several seconds to inspect a large image
    /// collection, so this work must never run on egui's rendering thread.
    docker_fetch_rx: Option<Receiver<Result<DockerSnapshot, String>>>,
    /// Keeps the loading state independent from the cached snapshot: a
    /// manual refresh leaves the previous rows visible while the new
    /// snapshot is collected, whereas the first load shows the empty
    /// « Chargement… » state.
    docker_fetching: bool,
    /// A confirmed Docker action awaiting execution at the start of the next
    /// frame — see [`DeferredDockerAction`]'s doc comment. Also drives
    /// `DockerViewState::busy` while it is `Some`.
    deferred_docker_action: Option<DeferredDockerAction>,
    /// Same test-gating pattern as `cleanup_spawning_enabled`: kittest
    /// harness tests never run a real `docker` command. Unlike
    /// `cleanup_spawning_enabled` (which only skips the process *spawn*),
    /// this also skips the post-action refetch, since that would be a real
    /// `docker` call too.
    docker_actions_enabled: bool,
    /// Counts every [`EguiApp::execute_deferred_docker_action`] call that
    /// found a deferred action to run, real call or not — the kittest
    /// assertion "Oui triggers exactly one façade call" reads this rather
    /// than trying to intercept `docker_view`'s free functions.
    docker_action_invocations: u32,
    /// Rows ticked for the next batch. Lives here, not in `docker_view`,
    /// because it has to survive the refetch that follows every action —
    /// and be pruned against it (`prune_docker_selection`).
    docker_selection: HashSet<SelectionKey>,
    /// Per-item result of the last batch, shown until the selection changes
    /// or the user refreshes. Never cleared on a timer: it is the only trace
    /// of what a partially-failed batch actually did.
    docker_batch_report: Vec<BatchOutcome>,
    /// Which of the three resource lists the Docker tab shows. Session-only,
    /// deliberately absent from `config.json`: a tab choice is not a setting,
    /// and « Conteneurs » is the right thing to reopen on.
    docker_active_list: DockerList,
    /// Result of the last host-port scan, `None` until one has run.
    ///
    /// Deliberately *not* refreshed by `refetch_docker`: the scan reads the
    /// machine's sockets, not Docker's state, so a container action has no
    /// reason to invalidate it — and re-running `netstat` after every `docker
    /// rm` would add a second of latency to each one. The « Rescanner l'hôte »
    /// button is how it gets refreshed.
    docker_host_ports: Option<Vec<crate::net::ListeningPort>>,
    /// The port reassignment proposal currently on screen, `None` until the
    /// user asks for one.
    ///
    /// Stored rather than recomputed every frame on purpose: the user reads a
    /// table and then clicks « Appliquer », and a plan that silently changed
    /// in between — because a container stopped, or a scan landed — would
    /// write something they never saw.
    docker_port_plan: Option<crate::ui::port_plan::ReassignmentPlan>,
    /// What the last application wrote, one entry per compose file. Cleared
    /// whenever a new plan is computed, so a stale report cannot be read as
    /// the result of the plan currently displayed.
    docker_port_edits: Vec<crate::docker::compose_edit::FileReport>,

    // --- Compose stacks (Part 2) -------------------------------------------
    /// `docker compose` plugin availability, probed once the first time the
    /// Docker tab is rendered. `None` until then — probing it at startup
    /// would run a `docker` command for a tab the user may never open.
    compose_plugin: Option<bool>,
    /// The stack rows, rebuilt by the background scan/reload worker. Their
    /// `runs`/`state` are recomputed from the live container list on every
    /// frame by `compose_view::link_runs`, so nothing here goes stale.
    compose_stacks: Vec<StackEntry>,
    /// `true` once the memorized `config.docker_stacks` have been handed to
    /// the worker, so the reload fires once per session rather than per frame.
    compose_loaded: bool,
    compose_scanning: bool,
    compose_scan_rx: Option<Receiver<ComposeScanResult>>,
    /// A slow-scan warning kept from the last walk, shown until the next one.
    compose_scan_warning: Option<String>,
    /// The in-flight compose command's streamed output. One command at a
    /// time, so this always has exactly one owner.
    compose_rx: Option<Receiver<TerminalEvent>>,
    compose_log: Vec<String>,
    /// File path of the row `compose_log` belongs to.
    compose_log_target: Option<String>,
    compose_running: bool,
    /// Same role as `docker_action_invocations`: lets a kittest assert that a
    /// click reached the launcher without a real `docker` process ever
    /// existing.
    compose_invocations: u32,
}

/// What the compose worker thread hands back.
struct ComposeScanResult {
    entries: Vec<StackEntry>,
    /// `Some` for a `$HOME` walk, `None` for a reload of the memorized list —
    /// which is exactly what tells the main thread whether to rewrite
    /// `config.docker_stacks`.
    outcome: Option<ScanOutcome>,
}

/// Read every file in `files` through `docker compose config` and turn each
/// into a row.
///
/// Runs on the worker thread: ~89 ms per file here, so the 13 real files
/// would stall the UI for over a second if this ran inline. A file that is
/// gone becomes a `Missing` row rather than disappearing — the user asked for
/// it once, and « Oublier » is how it leaves the list.
fn build_compose_entries(files: &[String]) -> Vec<StackEntry> {
    files
        .iter()
        .map(|file| {
            let path = Path::new(file);
            if !path.is_file() {
                return StackEntry {
                    file: file.clone(),
                    project: compose_view::default_project_name(file),
                    services: Vec::new(),
                    runs: Vec::new(),
                    state: StackState::Missing,
                    error: None,
                };
            }
            match compose_view::read_config(path) {
                Ok(config) => StackEntry {
                    file: file.clone(),
                    // Compose is the authority on its own project naming; the
                    // derived fallback only covers a `name`-less payload.
                    project: if config.name.trim().is_empty() {
                        compose_view::default_project_name(file)
                    } else {
                        config.name
                    },
                    services: config.services,
                    runs: Vec::new(),
                    state: StackState::Stopped,
                    error: None,
                },
                Err(error) => StackEntry::failed(file.clone(), error),
            }
        })
        .collect()
}

/// Past this many lines the inline compose log drops its oldest — the same
/// bounded-buffer rule as the Terminal view, at a tenth of the size since a
/// compose command is short-lived.
const COMPOSE_LOG_MAX: usize = 400;

impl EguiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Extend the default font fallback chain with the full Noto Emoji
        // font so user-picked action/category icons (🧹, 🏗️, …) don't
        // render as tofu — see `crate::ui::fonts`.
        crate::ui::fonts::install(&cc.egui_ctx);
        // egui's default scroll bars (`ScrollStyle::floating()`) are fully
        // transparent until the pointer hovers the scroll area's edge, so a
        // view taller than the window gives no visual hint that more content
        // exists below. `thin()` keeps a narrow bar visible whenever content
        // overflows, expanding on hover.
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.scroll = egui::style::ScrollStyle::thin();
        });
        let config = storage::load().unwrap_or_else(|err| {
            log::warn!("storage::load failed ({err}); falling back to built-in defaults");
            fallback_config()
        });
        theme::apply(
            &cc.egui_ctx,
            theme::ThemeMode::from_preference(&config.default_settings.theme),
        );
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
            docker_view::available(),
        );
        let dark = cc.egui_ctx.theme() == egui::Theme::Dark;
        let desired = native_window::decide(native_window::current_inputs(
            app.config.default_settings.native_effects,
        ));
        match native_window::apply(cc, native_window::NativeProfile::Opaque, desired, dark) {
            Ok(profile) => app.native_profile = profile,
            Err(error) => {
                log::warn!("native window material unavailable; using opaque fallback: {error}");
                app.native_effect_warning_logged = true;
            }
        }
        theme::set_native_material(
            &cc.egui_ctx,
            app.native_profile != native_window::NativeProfile::Opaque,
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
    /// `docker_available` is hardcoded `false` here rather than resolved via
    /// `docker_view::available()` (unlike `new`) so the test suite's default
    /// behavior never depends on whether the machine running it happens to
    /// have `docker` installed — tests that need the Docker tab present
    /// force `app.docker_available = true` directly afterward (private
    /// field, same module tree as this `impl` block).
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
            false,
        )
    }

    // Internal construction helper called from exactly 2 sites (`new`,
    // `new_for_test`), not part of any public API — same rationale as the
    // `variant_command` test builder's identical allow above.
    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        config: Config,
        config_path: Option<PathBuf>,
        icon_backend: EguiIconBackend,
        machine_commands: MachineCommands,
        machine_id: String,
        usage_service: Option<UsageService>,
        report_spawning_enabled: bool,
        docker_available: bool,
    ) -> Self {
        let (application_tx, application_rx) = std::sync::mpsc::channel();
        let (cleanup_tx, cleanup_rx) = std::sync::mpsc::channel();
        let (models_tx, models_rx) = std::sync::mpsc::channel();
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
            native_profile: native_window::NativeProfile::Opaque,
            native_effect_warning_logged: false,
            update_state: if crate::update::keys::configured() {
                UpdateState::Idle
            } else {
                UpdateState::Disabled(
                    "Mises à jour indisponibles dans cette build (aucune clé de production)."
                        .to_string(),
                )
            },
            update_rx: None,
            update_auto_checked: false,
            preferences_section: PreferencesSection::default(),
            user_script_proposals: Vec::new(),
            user_script_scan_error: None,
            user_scripts_scanned: false,
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
            models_ui: ModelsUiState::default(),
            models_snapshot: None,
            models_offers: Vec::new(),
            models_progress: Vec::new(),
            models_recovery: Vec::new(),
            models_guided: None,
            models_terminal: None,
            models_error: None,
            models_job: None,
            models_cancel: None,
            models_tx,
            models_rx,
            models_spawning_enabled: report_spawning_enabled,
            machine_commands,
            machine_id,
            docker_available,
            docker: None,
            docker_fetch_rx: None,
            docker_fetching: false,
            deferred_docker_action: None,
            docker_actions_enabled: report_spawning_enabled,
            docker_action_invocations: 0,
            docker_selection: HashSet::new(),
            docker_batch_report: Vec::new(),
            docker_active_list: DockerList::default(),
            docker_host_ports: None,
            docker_port_plan: None,
            docker_port_edits: Vec::new(),
            compose_plugin: None,
            compose_stacks: Vec::new(),
            compose_loaded: false,
            compose_scanning: false,
            compose_scan_rx: None,
            compose_scan_warning: None,
            compose_rx: None,
            compose_log: Vec::new(),
            compose_log_target: None,
            compose_running: false,
            compose_invocations: 0,
        }
    }

    /// The single-command-slot guard (brief decision: one external command
    /// at a time, whatever launched it — Actions card, Terminal, or a
    /// `clean.py` run).
    fn command_busy(&self) -> bool {
        self.action_running.is_some()
            || self.terminal_running
            || self.cleanup_job.is_some()
            || self.models_job.is_some()
    }

    fn start_update_check(&mut self) {
        let Some(format) = crate::update::service::current_package_format() else {
            self.update_state = UpdateState::Disabled(
                "Mise à jour intégrée réservée aux paquets installés; utilisez GitHub Releases."
                    .to_string(),
            );
            return;
        };
        let Ok(ring) = crate::update::keys::KeyRing::embedded() else {
            self.update_state =
                UpdateState::Disabled("Trousseau de mise à jour invalide.".to_string());
            return;
        };
        let Ok(current) = semver::Version::parse(env!("CARGO_PKG_VERSION")) else {
            self.update_state = UpdateState::Failed("Version applicative invalide.".to_string());
            return;
        };
        let (os, arch) = crate::update::service::current_target();
        let days = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() / 86_400)
            .unwrap_or(0);
        crate::update::service::record_check(days.saturating_mul(86_400));
        self.update_rx = Some(crate::update::service::spawn_check(
            crate::update::service::UpdateService::new(
                crate::update::service::HttpTransport,
                crate::update::service::CheckOnlyInstaller,
                ring,
            ),
            crate::update::MANIFEST_ENDPOINT.to_string(),
            current,
            os.to_string(),
            arch.to_string(),
            format,
            days,
        ));
        self.update_state = UpdateState::Checking;
    }

    fn start_update_install(
        &mut self,
        manifest: crate::update::manifest::ReleaseManifest,
        asset: crate::update::manifest::ReleaseAsset,
    ) {
        let Ok(ring) = crate::update::keys::KeyRing::embedded() else {
            self.update_state = UpdateState::Failed("Trousseau de mise à jour invalide.".into());
            return;
        };
        let days = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() / 86_400)
            .unwrap_or(0);
        self.update_rx = Some(crate::update::service::spawn_install(
            crate::update::service::UpdateService::new(
                crate::update::service::HttpTransport,
                crate::update::service::PlatformInstaller,
                ring,
            ),
            manifest,
            asset,
            days,
        ));
        self.update_state = UpdateState::Downloading;
    }

    fn drain_update_events(&mut self) {
        let Some(receiver) = &self.update_rx else {
            return;
        };
        let mut latest = None;
        while let Ok(state) = receiver.try_recv() {
            latest = Some(state);
        }
        if let Some(state) = latest {
            let terminal = !matches!(state, UpdateState::Checking);
            self.update_state = state;
            if terminal {
                self.update_rx = None;
            }
        }
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

    fn refresh_models(&mut self) {
        self.models_job = Some(ModelsJob::Inventory);
        self.models_error = None;
        if self.models_spawning_enabled {
            models::spawn_inventory(self.models_tx.clone());
        }
    }

    fn refresh_model_recovery(&mut self) {
        if self.models_ui.library_root.trim().is_empty() {
            return;
        }
        self.models_job = Some(ModelsJob::RecoveryRead);
        if self.models_spawning_enabled {
            models::spawn_query(
                vec![
                    "recovery".to_string(),
                    "--root".to_string(),
                    self.models_ui.library_root.clone(),
                ],
                self.models_tx.clone(),
            );
        }
    }

    fn drain_model_events(&mut self) {
        let mut events = Vec::new();
        while let Ok(event) = self.models_rx.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                ModelWorkerEvent::Inventory(result) => {
                    self.models_job = None;
                    match result {
                        Ok(snapshot) => {
                            self.models_snapshot = Some(snapshot);
                            self.models_error = None;
                            if self.models_ui.library_root.is_empty()
                                && self.models_spawning_enabled
                            {
                                self.models_job = Some(ModelsJob::SettingsRead);
                                models::spawn_query(
                                    vec!["settings".to_string()],
                                    self.models_tx.clone(),
                                );
                            } else {
                                self.refresh_model_recovery();
                            }
                        }
                        Err(error) => self.models_error = Some(error),
                    }
                }
                ModelWorkerEvent::Json(result) => {
                    let job = self.models_job.take();
                    match (job, result) {
                        (Some(ModelsJob::Resolve), Ok(value)) => {
                            match serde_json::from_value::<Vec<AcquisitionOffer>>(value) {
                                Ok(mut offers) => {
                                    let manual = self.models_ui.manual_provider.trim();
                                    if !manual.is_empty() {
                                        offers.sort_by_key(|offer| offer.provider != manual);
                                    }
                                    self.models_offers = offers;
                                    self.models_ui.selected_offer = None;
                                    self.models_error = None;
                                }
                                Err(error) => {
                                    self.models_error =
                                        Some(format!("offres modèles invalides: {error}"));
                                }
                            }
                        }
                        (
                            Some(job @ (ModelsJob::SettingsRead | ModelsJob::SettingsWrite)),
                            Ok(value),
                        ) => match serde_json::from_value::<ModelSettings>(value) {
                            Ok(settings) => {
                                self.models_ui.library_root = settings.library_root;
                                self.models_ui.provider_order = settings.provider_order.join(",");
                                self.models_ui.enabled_providers =
                                    settings.enabled_providers.join(",");
                                self.models_ui.xet_enabled = settings.xet_enabled;
                                self.models_ui.keep_pattern = settings.keep_patterns.join(",");
                                self.models_error = None;
                                if job == ModelsJob::SettingsWrite {
                                    self.refresh_models();
                                } else {
                                    self.refresh_model_recovery();
                                }
                            }
                            Err(error) => {
                                self.models_error =
                                    Some(format!("réglages modèles invalides: {error}"));
                            }
                        },
                        (Some(ModelsJob::RecoveryRead), Ok(value)) => {
                            match serde_json::from_value::<Vec<models::LibraryJournal>>(value) {
                                Ok(recovery) => {
                                    self.models_recovery = recovery;
                                    self.models_error = None;
                                }
                                Err(error) => {
                                    self.models_error =
                                        Some(format!("journaux modèles invalides: {error}"));
                                }
                            }
                        }
                        (Some(ModelsJob::RecoveryWrite), Ok(_)) => self.refresh_models(),
                        (Some(ModelsJob::GuidedWrite), Ok(value)) => {
                            match serde_json::from_value::<models::GuidedMigration>(value) {
                                Ok(guided) => {
                                    self.models_guided = Some(guided);
                                    self.models_ui.section = models_view::ModelsSection::Operations;
                                    self.models_error = None;
                                }
                                Err(error) => {
                                    self.models_error =
                                        Some(format!("intégration guidée invalide: {error}"));
                                }
                            }
                        }
                        (_, Err(error)) => self.models_error = Some(error),
                        (_, Ok(_)) => {
                            self.models_error =
                                Some("réponse modèles reçue pour un job inattendu".to_string());
                        }
                    }
                }
                ModelWorkerEvent::Progress(event) => self.models_progress.push(event),
                ModelWorkerEvent::Terminal(result) => {
                    self.models_job = None;
                    self.models_cancel = None;
                    match result {
                        Ok(event) => {
                            if event.kind != "completed" {
                                self.models_error = event.message.clone();
                            }
                            self.models_terminal = Some(event);
                        }
                        Err(error) => self.models_error = Some(error),
                    }
                    if self.models_spawning_enabled {
                        self.refresh_models();
                    }
                }
            }
        }
    }

    fn handle_models_action(&mut self, action: ModelsAction) {
        match action {
            ModelsAction::Refresh => self.refresh_models(),
            ModelsAction::Resolve => {
                self.models_job = Some(ModelsJob::Resolve);
                self.models_error = None;
                let mut arguments = vec![
                    "resolve".to_string(),
                    self.models_ui.locator.trim().to_string(),
                    "--family".to_string(),
                    self.models_ui.family.clone(),
                ];
                for alternative in self
                    .models_ui
                    .alternatives
                    .split([',', '\n'])
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    arguments.push("--alternative".to_string());
                    arguments.push(alternative.to_string());
                }
                if self.models_spawning_enabled {
                    models::spawn_query(arguments, self.models_tx.clone());
                }
            }
            ModelsAction::Review(index) => self.models_ui.selected_offer = Some(index),
            ModelsAction::RunReviewed => {
                let Some(offer) = self
                    .models_ui
                    .selected_offer
                    .and_then(|index| self.models_offers.get(index))
                    .cloned()
                else {
                    self.models_error = Some("Aucun plan exact revu.".to_string());
                    return;
                };
                let Some(digest) = offer.review_digest.clone() else {
                    self.models_error =
                        Some("Le plan revu n'a pas de digest immuable.".to_string());
                    return;
                };
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let operation_id = format!("ui-download-{millis}");
                let mut arguments = vec![
                    "download".to_string(),
                    offer.locator,
                    "--family".to_string(),
                    offer.family,
                    "--operation-id".to_string(),
                    operation_id.clone(),
                    "--review-digest".to_string(),
                    digest,
                ];
                if !self.models_ui.library_root.trim().is_empty() {
                    arguments.push("--root".to_string());
                    arguments.push(self.models_ui.library_root.clone());
                }
                self.models_progress.clear();
                self.models_terminal = None;
                self.models_error = None;
                self.models_job = Some(ModelsJob::Download);
                self.models_ui.section = models_view::ModelsSection::Operations;
                if self.models_spawning_enabled {
                    self.models_cancel = Some(models::spawn_operation(
                        operation_id,
                        arguments,
                        self.models_tx.clone(),
                    ));
                }
            }
            ModelsAction::Cancel => {
                if let Some(cancel) = &self.models_cancel {
                    if let Err(error) = cancel.cancel() {
                        self.models_error = Some(error);
                    }
                }
            }
            ModelsAction::Recover {
                operation_id,
                action,
            } => {
                self.active_dialog = Some(ActiveDialog {
                    kind: dialogs::confirm(
                        "Récupérer l'opération ?",
                        format!(
                            "Appliquer « {action} » uniquement au staging possédé de l'opération exacte « {operation_id} » ?"
                        ),
                    ),
                    on_confirm: Some(PendingAction::RecoverModelOperation {
                        operation_id,
                        action,
                    }),
                });
            }
            ModelsAction::Guide {
                artifact_id,
                destination,
                category,
            } => {
                self.active_dialog = Some(ActiveDialog {
                    kind: dialogs::confirm(
                        "Préparer l'intégration guidée ?",
                        format!(
                            "Préparer l'artefact exact « {artifact_id} » pour {destination} ?\nAucun fichier tiers ne sera réécrit automatiquement."
                        ),
                    ),
                    on_confirm: Some(PendingAction::GuideModel {
                        artifact_id,
                        destination,
                        category,
                    }),
                });
            }
            ModelsAction::SaveSettings => {
                let root = self.models_ui.library_root.clone();
                self.active_dialog = Some(ActiveDialog {
                    kind: dialogs::confirm(
                        "Changer de bibliothèque ?",
                        format!(
                            "Utiliser « {root} » pour les prochains artefacts ?\nLes fichiers existants ne seront ni déplacés ni supprimés.",
                        ),
                    ),
                    on_confirm: Some(PendingAction::SaveModelSettings {
                        root,
                        provider_order: self.models_ui.provider_order.clone(),
                        enabled_providers: self.models_ui.enabled_providers.clone(),
                        xet_enabled: self.models_ui.xet_enabled,
                        keep_patterns: self.models_ui.keep_pattern.clone(),
                    }),
                });
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

    /// Renders one card. Both shapes go through [`render_card_shell`], so
    /// they share the same frame, icon, title and clickable body; the body
    /// is scoped inside `ui.add_enabled_ui`, keyed on whether the command
    /// (or, for a grouped card, the selected variant) is configured —
    /// `false` only for a `machine_specific: true` command with no matching
    /// per-machine mapping entry (Part 3). Favorite management lives
    /// exclusively in Préférences now, not here — see
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
        let shell = render_card_shell(ui, &visual, &card.name, card.is_configured, |_ui| {});

        if let Some(message) = badge_message(card.disabled_message.as_deref(), card.info.as_deref())
        {
            card_corner_badge(ui, shell.rect, &message);
        }

        // `add_enabled_ui` already suppresses `clicked()` for an
        // unconfigured card, so `card.is_configured` is redundant with
        // `body_clicked` here — kept explicit as the same guard
        // `can_launch_card` exposes for its pure unit test.
        if shell.body_clicked && can_launch_card(card.is_configured, self.command_busy()) {
            self.launch_command(&card.command_id, &card.command);
        }
    }

    /// Grouped card: same clickable body as a simple card — it launches the
    /// currently selected variant — plus a `ComboBox` picking among
    /// `card.variants` (session state in `self.selected_variant`, keyed by
    /// `card.group_name`) and an explicit "Lancer" button. The picker sits
    /// outside the body's click target, so choosing a variant never launches
    /// anything by accident.
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
        let selected_badge_message = badge_message(
            selected.disabled_message.as_deref(),
            selected.info.as_deref(),
        );
        let can_launch = can_launch_card(selected_is_configured, self.command_busy());

        let mut requested_variant: Option<String> = None;
        let mut launch_clicked = false;

        let shell = render_card_shell(ui, &visual, &card.name, selected_is_configured, |ui| {
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

            ui.add_enabled_ui(can_launch, |ui| {
                if ui.button("Lancer").clicked() {
                    launch_clicked = true;
                }
            });
        });

        if let Some(message) = &selected_badge_message {
            card_corner_badge(ui, shell.rect, message);
        }

        if let Some(variant_id) = requested_variant {
            self.selected_variant.insert(group_key, variant_id);
        }
        if (launch_clicked || shell.body_clicked) && can_launch {
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
        let user_scripts = self.config.default_settings.user_scripts_directory.trim();
        let user_scripts = (!user_scripts.is_empty()).then(|| Path::new(user_scripts));
        match terminal_view::launch_captured_with_user_scripts(command, user_scripts, tx) {
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
            PendingAction::InstallUpdate { manifest, asset } => {
                self.start_update_install(manifest, *asset);
            }
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
            PendingAction::ComposeDown(target) => {
                let args = compose_view::down_args(&target);
                self.launch_compose(&target, args, "Destruction de la stack");
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
            PendingAction::SaveModelSettings {
                root,
                provider_order,
                enabled_providers,
                xet_enabled,
                keep_patterns,
            } => {
                if self.command_busy() {
                    self.models_error =
                        Some("Une autre commande est active; réglage non modifié.".to_string());
                    return;
                }
                self.models_job = Some(ModelsJob::SettingsWrite);
                if self.models_spawning_enabled {
                    models::spawn_json_mutation(
                        vec![
                            "settings".to_string(),
                            "--set-library-root".to_string(),
                            root,
                            "--set-provider-order".to_string(),
                            provider_order,
                            "--set-enabled-providers".to_string(),
                            enabled_providers,
                            if xet_enabled {
                                "--xet-enabled".to_string()
                            } else {
                                "--no-xet-enabled".to_string()
                            },
                            "--set-keep-patterns".to_string(),
                            keep_patterns,
                        ],
                        self.models_tx.clone(),
                    );
                }
            }
            PendingAction::RecoverModelOperation {
                operation_id,
                action,
            } => {
                if self.command_busy() {
                    self.models_error =
                        Some("Une autre commande est active; reprise non appliquée.".to_string());
                    return;
                }
                self.models_job = Some(ModelsJob::RecoveryWrite);
                if self.models_spawning_enabled {
                    models::spawn_json_mutation(
                        vec![
                            "recover".to_string(),
                            "--root".to_string(),
                            self.models_ui.library_root.clone(),
                            "--operation-id".to_string(),
                            operation_id,
                            "--action".to_string(),
                            action.clone(),
                            "--capability".to_string(),
                            action,
                        ],
                        self.models_tx.clone(),
                    );
                }
            }
            PendingAction::GuideModel {
                artifact_id,
                destination,
                category,
            } => {
                if self.command_busy() {
                    self.models_error = Some(
                        "Une autre commande est active; intégration non préparée.".to_string(),
                    );
                    return;
                }
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let mut arguments = vec![
                    "guided-start".to_string(),
                    "--artifact-id".to_string(),
                    artifact_id,
                    "--destination".to_string(),
                    destination,
                    "--migration-id".to_string(),
                    format!("ui-guided-{millis}"),
                ];
                if let Some(category) = category {
                    arguments.push("--category".to_string());
                    arguments.push(category);
                }
                self.models_job = Some(ModelsJob::GuidedWrite);
                if self.models_spawning_enabled {
                    models::spawn_json_mutation(arguments, self.models_tx.clone());
                }
            }
            // Docker actions never run here — a plain `docker stop` can
            // block for several seconds, and this function runs inline
            // while resolving the confirm dialog's "Oui" click, before that
            // frame has finished being built. Instead: stash the action for
            // `execute_deferred_docker_action` (which runs at the very
            // start of the next frame) and set the "in progress" status
            // text now — `ui_content`'s `DialogOutcome::Accepted` arm lets
            // this frame finish rendering instead of returning early
            // whenever a Docker action was just deferred, so this status
            // text is genuinely painted before the next frame's freeze.
            PendingAction::StopContainer { id, name } => {
                let message = format!("Arrêt de {name}…");
                self.deferred_docker_action =
                    Some(DeferredDockerAction::StopContainer { id, name });
                self.set_status(message, false);
            }
            PendingAction::RemoveContainer { id, name } => {
                let message = format!("Suppression de {name}…");
                self.deferred_docker_action =
                    Some(DeferredDockerAction::RemoveContainer { id, name });
                self.set_status(message, false);
            }
            PendingAction::RemoveImage { reference, name } => {
                let message = format!("Suppression de {name}…");
                self.deferred_docker_action =
                    Some(DeferredDockerAction::RemoveImage { reference, name });
                self.set_status(message, false);
            }
            PendingAction::RemoveVolume(name) => {
                let message = format!("Suppression de {name}…");
                self.deferred_docker_action = Some(DeferredDockerAction::RemoveVolume { name });
                self.set_status(message, false);
            }
            PendingAction::ApplyPortReassignment(moves) => {
                let message = format!("Réattribution de {} port(s)…", moves.len());
                self.deferred_docker_action =
                    Some(DeferredDockerAction::ApplyPortReassignment(moves));
                self.set_status(message, false);
            }
            PendingAction::DeleteSelection(targets) => {
                let message = format!("Suppression de {} ressource(s)…", targets.len());
                self.deferred_docker_action = Some(DeferredDockerAction::DeleteSelection(targets));
                self.set_status(message, false);
            }
        }
    }

    /// Runs a [`DeferredDockerAction`] stashed by `resolve_pending_action`'s
    /// docker arms — called at the very start of every frame (see
    /// `ui_content`), so a frame has already rendered the "in progress"
    /// status text before this potentially blocking call happens (the plan's
    /// Phase 3 task 4 freeze mitigation). Does nothing when there is nothing
    /// deferred, which makes it safe to call unconditionally every frame.
    ///
    /// `docker_action_invocations` is incremented unconditionally so
    /// kittest tests can assert "exactly one façade call" without needing
    /// to intercept `docker_view`'s free functions; the real façade call
    /// (and the success refetch, itself a real `docker` call) only happen
    /// when `docker_actions_enabled` is `true` — `false` in every test
    /// harness, mirroring `cleanup_spawning_enabled`.
    fn execute_deferred_docker_action(&mut self) {
        let Some(action) = self.deferred_docker_action.take() else {
            return;
        };
        self.docker_action_invocations = self.docker_action_invocations.saturating_add(1);
        if !self.docker_actions_enabled {
            return;
        }
        // The batch has its own shape too: N results instead of one, and a
        // report to keep. Handled here rather than in the uniform `match`
        // below for the same reason as `ComputeVolumeSizes`.
        if let DeferredDockerAction::DeleteSelection(targets) = &action {
            let report = docker_view::remove_batch(targets);
            let failures = report.iter().filter(|o| o.result.is_err()).count();
            let succeeded = report.len() - failures;
            // `remove_batch` runs `order_targets`' order, so zipping the two
            // pairs each outcome with the target that produced it — the only
            // way back from a label to its key.
            for (target, outcome) in docker_view::order_targets(targets).iter().zip(&report) {
                if outcome.result.is_ok() {
                    self.docker_selection.remove(&target.key);
                }
            }
            // A failed target stays ticked on purpose: « réessayer » is one
            // click, and unticking it would hide what still needs attention.
            self.docker_batch_report = report;
            let message = if failures == 0 {
                format!("{succeeded} ressource(s) supprimée(s).")
            } else {
                format!("{succeeded} supprimée(s), {failures} en échec — voir le rapport.")
            };
            self.set_status(message, failures > 0);
            self.refetch_docker();
            return;
        }
        // `ComputeVolumeSizes` returns a different shape (a name/size list,
        // not `Result<(), String>`) and merges into the existing snapshot
        // instead of refetching (a refetch would just re-fill every size
        // back to `None`, since `docker volume ls` never reports it) — handle
        // it separately from the other, uniformly-shaped destructive actions.
        if matches!(action, DeferredDockerAction::ComputeVolumeSizes) {
            match docker_view::volume_sizes() {
                Ok(sizes) => {
                    let sizes: HashMap<String, String> = sizes.into_iter().collect();
                    if let Some(Ok(snapshot)) = self.docker.as_mut() {
                        for volume in &mut snapshot.volumes {
                            if let Some(size) = sizes.get(&volume.name) {
                                volume.size = Some(size.clone());
                            }
                        }
                    }
                    self.set_status("Tailles des volumes calculées.", false);
                }
                Err(err) => self.set_status(err, true),
            }
            return;
        }
        // Writes files, reports per file, and must not refetch: rewriting a
        // `ports:` line changes nothing about the containers Docker currently
        // holds — that is exactly the caveat the view warns about.
        if let DeferredDockerAction::ApplyPortReassignment(moves) = &action {
            let reports = crate::docker::compose_edit::apply(moves);
            let written: usize = reports.iter().map(|report| report.applied.len()).sum();
            let refused: usize = reports.iter().map(|report| report.refused.len()).sum();
            self.docker_port_edits = reports;
            // The proposal is dropped whatever happened: half of it may now be
            // stale, and re-clicking « Appliquer » on an already-applied plan
            // would look for ports that are no longer in the file.
            self.docker_port_plan = None;
            let message = if refused == 0 {
                format!("{written} port(s) réécrit(s). Relancer les stacks avec --force-recreate.")
            } else {
                format!("{written} port(s) réécrit(s), {refused} refusé(s) — voir le détail.")
            };
            self.set_status(message, refused > 0);
            return;
        }
        // Same shape argument as `ComputeVolumeSizes`: the result is a list,
        // not a `Result<(), String>`, and it must not trigger a refetch — the
        // Docker snapshot has nothing to do with what the host is listening on.
        if matches!(action, DeferredDockerAction::ScanHostPorts) {
            match crate::net::scan() {
                Ok(ports) => {
                    let message = format!("{} port(s) à l'écoute sur l'hôte.", ports.len());
                    self.docker_host_ports = Some(ports);
                    self.set_status(message, false);
                }
                // The previous scan is kept on failure: stale data with a
                // visible error beats silently emptying the « Hôte » column,
                // which would read as "everything is free now".
                Err(err) => self.set_status(err, true),
            }
            return;
        }
        let result = match &action {
            DeferredDockerAction::StopContainer { id, .. } => docker_view::stop_container(id),
            DeferredDockerAction::RemoveContainer { id, .. } => docker_view::remove_container(id),
            DeferredDockerAction::RemoveImage { reference, .. } => {
                docker_view::remove_image(reference)
            }
            DeferredDockerAction::RemoveVolume { name } => docker_view::remove_volume(name),
            DeferredDockerAction::ComputeVolumeSizes
            | DeferredDockerAction::ScanHostPorts
            | DeferredDockerAction::ApplyPortReassignment(_)
            | DeferredDockerAction::DeleteSelection(_) => {
                unreachable!("handled above")
            }
        };
        match result {
            Ok(()) => {
                let message = match &action {
                    DeferredDockerAction::StopContainer { name, .. } => {
                        format!("Conteneur {name} arrêté.")
                    }
                    DeferredDockerAction::RemoveContainer { name, .. } => {
                        format!("Conteneur {name} supprimé.")
                    }
                    DeferredDockerAction::RemoveImage { name, .. } => {
                        format!("Image {name} supprimée.")
                    }
                    DeferredDockerAction::RemoveVolume { name } => {
                        format!("Volume {name} supprimé.")
                    }
                    DeferredDockerAction::ComputeVolumeSizes
                    | DeferredDockerAction::ScanHostPorts
                    | DeferredDockerAction::ApplyPortReassignment(_)
                    | DeferredDockerAction::DeleteSelection(_) => unreachable!("handled above"),
                };
                self.set_status(message, false);
                self.refetch_docker();
            }
            Err(err) => self.set_status(err, true),
        }
    }

    /// Start a Docker-state refresh without blocking egui's rendering
    /// thread. A second request while one is already running is ignored;
    /// every Docker/Compose action is disabled during the refresh, so the
    /// in-flight snapshot cannot be made stale by this app.
    ///
    /// The spawning gate keeps headless tests independent from the machine's
    /// Docker installation. Such tests can still exercise the loading and
    /// receiving states by installing a receiver directly.
    fn refetch_docker(&mut self) {
        if self.docker_fetching {
            return;
        }
        self.docker_fetching = true;
        if !self.docker_actions_enabled {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.docker_fetch_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(docker_view::fetch());
        });
    }

    /// Merge a completed background fetch into the cached snapshot and only
    /// then re-validate the selection against it.
    fn drain_docker_fetch(&mut self) {
        let Some(rx) = self.docker_fetch_rx.as_ref() else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("Le chargement des données Docker a été interrompu.".to_string())
            }
        };
        self.docker_fetch_rx = None;
        self.docker_fetching = false;
        self.docker = Some(result);
        self.prune_docker_selection();
    }

    /// Re-validate the selection after a Docker refresh.
    ///
    /// The invariant must hold after *every* completed refetch: a key whose
    /// resource has just been deleted — by this app, by another terminal,
    /// by anything — must stop being a batch target, or the next batch would
    /// run `docker rm` on a
    /// container that no longer exists and report a failure the user cannot
    /// act on.
    /// Drop every selected key the current snapshot no longer allows. A
    /// failed fetch clears the selection outright: there is nothing left to
    /// validate against, and keeping keys would mean acting on a state that
    /// could not be read.
    fn prune_docker_selection(&mut self) {
        match self.docker.as_ref() {
            Some(Ok(snapshot)) => {
                self.docker_selection =
                    docker_view::sanitize_selection(&self.docker_selection, snapshot);
            }
            _ => self.docker_selection.clear(),
        }
    }

    /// Builds the confirm-dialog title/message for a destructive
    /// `DockerAction` and opens it as the [`ActiveDialog`] — the actual
    /// façade call is deferred (see `resolve_pending_action` and
    /// `execute_deferred_docker_action`). Looks up the target row in the
    /// last-fetched `self.docker` snapshot for its displayable name (and,
    /// for images, to distinguish an untag from a definitive deletion via
    /// `ImageEntry::is_untagged`) — falls back to the raw identifier if the
    /// snapshot doesn't have it (should not normally happen: the action was
    /// just emitted from that very snapshot's render).
    fn open_docker_confirm(&mut self, action: DockerAction) {
        let snapshot = self.docker.as_ref().and_then(|result| result.as_ref().ok());
        let (title, message, on_confirm) = match action {
            DockerAction::Refresh
            | DockerAction::Retry
            | DockerAction::ComputeVolumeSizes
            | DockerAction::ScanHostPorts
            | DockerAction::PlanPortReassignment
            | DockerAction::ClearPortPlan
            | DockerAction::ToggleSelection(_)
            | DockerAction::SelectDormant
            | DockerAction::ClearSelection
            | DockerAction::SelectList(_) => {
                unreachable!("the non-destructive actions are handled directly in render_docker_view, never opened as a confirm dialog")
            }
            DockerAction::ApplyPortReassignment(moves) => {
                let files = {
                    let mut files: Vec<&str> =
                        moves.iter().map(|entry| entry.file.as_str()).collect();
                    files.sort_unstable();
                    files.dedup();
                    files.len()
                };
                (
                    "Réattribuer les ports".to_string(),
                    format!(
                        "{} port(s) vont être réécrits dans {files} fichier(s) compose.                          Une copie de chaque fichier est enregistrée avant modification.                          Les conteneurs déjà créés gardent leur port jusqu'à un                          « docker compose up -d --force-recreate ». Continuer ?",
                        moves.len()
                    ),
                    PendingAction::ApplyPortReassignment(moves),
                )
            }
            DockerAction::StopContainer(id) => {
                let name = snapshot
                    .and_then(|snapshot| snapshot.containers.iter().find(|c| c.id == id))
                    .map(|container| {
                        if container.name.is_empty() {
                            container.id.clone()
                        } else {
                            container.name.clone()
                        }
                    })
                    .unwrap_or_else(|| id.clone());
                (
                    "Arrêter le conteneur".to_string(),
                    format!("Le conteneur « {name} » va être arrêté (docker stop). Continuer ?"),
                    PendingAction::StopContainer { id, name },
                )
            }
            DockerAction::RemoveContainer(id) => {
                let container =
                    snapshot.and_then(|snapshot| snapshot.containers.iter().find(|c| c.id == id));
                let name = container
                    .map(|container| {
                        if container.name.is_empty() {
                            container.id.clone()
                        } else {
                            container.name.clone()
                        }
                    })
                    .unwrap_or_else(|| id.clone());
                let mut message = format!(
                    "Le conteneur « {name} » va être définitivement supprimé (docker rm). Continuer ?"
                );
                // `rw_size` is empty when the CLI didn't report a size (should
                // not normally happen with `--size`, but must not crash or
                // print a bogus "Libérera environ ." if it ever does).
                if let Some(rw_size) = container
                    .map(|c| c.rw_size.as_str())
                    .filter(|s| !s.is_empty())
                {
                    message.push_str(&format!(
                        " Libérera environ {rw_size} (couche d'écriture du conteneur)."
                    ));
                }
                (
                    "Supprimer le conteneur".to_string(),
                    message,
                    PendingAction::RemoveContainer { id, name },
                )
            }
            DockerAction::RemoveImage(reference) => {
                let image = snapshot.and_then(|snapshot| {
                    snapshot
                        .images
                        .iter()
                        .find(|i| i.rmi_reference == reference)
                });
                // A tagged image is only *untagged* by `docker rmi repo:tag`
                // when other tags still point at the same image id; when this
                // is its sole tag the image is genuinely deleted and its space
                // reclaimed, so the wording (and the size shown) depends on
                // how many snapshot entries share this id.
                let other_tags_remain = image.is_some_and(|image| {
                    snapshot.is_some_and(|snapshot| {
                        snapshot.images.iter().filter(|i| i.id == image.id).count() > 1
                    })
                });
                let (name, mut message) = match image {
                    Some(image) if image.is_untagged() => (
                        image.id.clone(),
                        format!(
                            "L'image « {} » (non taguée) va être définitivement supprimée (docker rmi). Continuer ?",
                            image.id
                        ),
                    ),
                    Some(image) if other_tags_remain => (
                        image.identity.clone(),
                        format!(
                            "Le tag « {} » va être retiré de cette image (docker rmi) — elle ne sera pas supprimée car d'autres tags pointent encore vers elle. Continuer ?",
                            image.identity
                        ),
                    ),
                    Some(image) => (
                        image.identity.clone(),
                        format!(
                            "L'image « {} » va être définitivement supprimée (docker rmi). Continuer ?",
                            image.identity
                        ),
                    ),
                    None => (
                        reference.clone(),
                        format!(
                            "L'image « {reference} » va être supprimée (docker rmi). Continuer ?"
                        ),
                    ),
                };
                match image {
                    Some(_) if other_tags_remain => {
                        message.push_str(
                            " Aucun espace ne sera libéré tant que les autres tags subsistent.",
                        );
                    }
                    Some(image) => {
                        message.push_str(&format!(
                            " Libérera jusqu'à {} (les couches partagées avec d'autres images ne sont pas comptées).",
                            image.size
                        ));
                    }
                    None => {}
                }
                (
                    "Supprimer l'image".to_string(),
                    message,
                    PendingAction::RemoveImage { reference, name },
                )
            }
            DockerAction::RemoveVolume(name) => {
                let volume =
                    snapshot.and_then(|snapshot| snapshot.volumes.iter().find(|v| v.name == name));
                let mut message = format!(
                    "Le volume orphelin « {name} » va être définitivement supprimé (docker volume rm). Continuer ?"
                );
                match volume.and_then(|v| v.size.as_deref()) {
                    Some(size) => message.push_str(&format!(" Libérera {size}.")),
                    None => message.push_str(
                        " Taille inconnue — le bouton « Calculer les tailles » permet de l'estimer.",
                    ),
                }
                (
                    "Supprimer le volume".to_string(),
                    message,
                    PendingAction::RemoveVolume(name),
                )
            }
            DockerAction::DeleteSelection(targets) => {
                let keys: HashSet<SelectionKey> =
                    targets.iter().map(|target| target.key.clone()).collect();
                let (containers, images, volumes) = docker_view::selection_counts(&keys);
                // No snapshot means no size to quote — announced as unknown
                // rather than as `0B`, which would read as "frees nothing".
                let size = snapshot.map(|snapshot| docker_view::selection_size(&keys, snapshot));
                // Laid out on several short lines rather than one long
                // sentence: `egui::Modal` sizes itself to its content, and a
                // 160-character line makes it wider than the window — wide
                // enough for « Oui » to land outside it, where a click reads
                // as a backdrop dismissal instead of a confirmation.
                let mut message = format!(
                    "Suppression définitive, dans cet ordre :\n\
                     • {containers} conteneur(s) — docker rm\n\
                     • {images} image(s) — docker rmi\n\
                     • {volumes} volume(s) — docker volume rm\n\n"
                );
                match size {
                    // `(0, true)` means *nothing* in the selection had a known
                    // size, not that it frees nothing: announcing « ≥ 0B »
                    // there dresses up an absent measurement as a real one.
                    Some((0, true)) | None => message.push_str("Espace récupéré inconnu.\n"),
                    Some((bytes, partial)) => message.push_str(&format!(
                        "Libérera environ {}.\n",
                        docker_view::format_selection_size(bytes, partial)
                    )),
                }
                message.push_str("Continuer ?");
                (
                    "Supprimer la sélection".to_string(),
                    message,
                    PendingAction::DeleteSelection(targets),
                )
            }
        };
        self.active_dialog = Some(ActiveDialog {
            kind: dialogs::confirm(title, message),
            on_confirm: Some(on_confirm),
        });
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

        let info = {
            let trimmed = form.info.trim();
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
                    info,
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
                    info,
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

        let info_label = ui.label("Information");
        ui.text_edit_singleline(&mut form.info)
            .labelled_by(info_label.id);

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

    fn render_preferences_actions_header(&mut self, ui: &mut egui::Ui) {
        ui.strong("Scripts");
        ui.label(
            "Dossier de base des scripts ajoutés par l’utilisateur. Les outils intégrés conservent leur emplacement géré par DevToolBox.",
        );
        let current_directory = self
            .config
            .default_settings
            .user_scripts_directory
            .trim()
            .to_string();
        let mut browse = false;
        let mut reset = false;
        ui.horizontal(|ui| {
            ui.label("Dossier de scripts utilisateur");
            if current_directory.is_empty() {
                ui.weak("Aucun dossier sélectionné");
            } else {
                ui.monospace(&current_directory);
            }
            if ui.button("Parcourir…").clicked() {
                browse = true;
            }
            if ui
                .add_enabled(
                    !current_directory.is_empty(),
                    egui::Button::new("Réinitialiser"),
                )
                .clicked()
            {
                reset = true;
            }
        });
        ui.weak("Exemple : @python sauvegarde.py résout sauvegarde.py dans ce dossier.");

        if browse {
            let mut dialog = rfd::FileDialog::new().set_title("Choisir le dossier de scripts");
            if !current_directory.is_empty() && Path::new(&current_directory).is_dir() {
                dialog = dialog.set_directory(&current_directory);
            }
            if let Some(directory) = dialog.pick_folder() {
                self.persist_user_scripts_directory(Some(&directory));
            }
        } else if reset {
            self.persist_user_scripts_directory(None);
        }

        let mut scan = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!current_directory.is_empty(), egui::Button::new("Scanner"))
                .on_disabled_hover_text("Sélectionnez d’abord un dossier de scripts")
                .clicked()
            {
                scan = true;
            }
            ui.label("Recherche récursivement les fichiers Python à proposer comme actions.");
        });
        if scan {
            self.user_scripts_scanned = true;
            match scan_user_python_scripts(Path::new(&current_directory)) {
                Ok(paths) => {
                    self.user_script_proposals = paths
                        .into_iter()
                        .map(|relative_path| UserScriptProposal {
                            relative_path,
                            selected: true,
                        })
                        .collect();
                    self.user_script_scan_error = None;
                }
                Err(error) => {
                    self.user_script_proposals.clear();
                    self.user_script_scan_error = Some(error);
                }
            }
        }

        if let Some(error) = &self.user_script_scan_error {
            ui.colored_label(egui::Color32::from_rgb(0xC4, 0x2B, 0x1C), error);
        } else if self.user_scripts_scanned && self.user_script_proposals.is_empty() {
            ui.label("Aucun script Python à proposer.");
        }

        if !self.user_script_proposals.is_empty() {
            ui.separator();
            ui.strong("Propositions d’actions");
            let configured: HashSet<String> = self
                .config
                .commands
                .iter()
                .map(|command| command.command.clone())
                .collect();
            for proposal in &mut self.user_script_proposals {
                let command = user_script_command(&proposal.relative_path);
                let already_added = configured.contains(&command);
                ui.horizontal(|ui| {
                    ui.add_enabled(
                        !already_added,
                        egui::Checkbox::new(
                            &mut proposal.selected,
                            proposal.relative_path.display().to_string(),
                        ),
                    );
                    if already_added {
                        ui.weak("déjà configuré");
                    }
                });
            }
            let selected_count = self
                .user_script_proposals
                .iter()
                .filter(|proposal| {
                    proposal.selected
                        && !configured.contains(&user_script_command(&proposal.relative_path))
                })
                .count();
            if ui
                .add_enabled(
                    selected_count > 0,
                    egui::Button::new(format!(
                        "Ajouter les scripts sélectionnés ({selected_count})"
                    )),
                )
                .clicked()
            {
                self.inject_selected_user_scripts();
            }
        }
    }

    fn render_preferences_docker(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("Docker");
        let mut persist_threshold = false;
        ui.horizontal(|ui| {
            let label = ui.label("Seuil de dormance (jours)");
            let response = ui
                .add(
                    egui::DragValue::new(&mut self.config.default_settings.dormant_after_days)
                        .range(1..=3650)
                        .speed(1.0),
                )
                .labelled_by(label.id)
                .on_hover_text(
                    "Un conteneur arrêté, une image inutilisée ou un volume orphelin \
                     plus ancien que ce seuil est signalé « dormant ».",
                );
            if response.drag_stopped() || response.lost_focus() {
                persist_threshold = true;
            }
        });
        if persist_threshold {
            match self.persist() {
                Ok(()) => self.set_status("Seuil de dormance mis à jour.", false),
                Err(err) => self.set_status(format!("Échec de sauvegarde: {err}"), true),
            }
        }
    }

    fn inject_selected_user_scripts(&mut self) {
        let mut used_ids: HashSet<String> = self
            .config
            .commands
            .iter()
            .map(|command| command.id.clone())
            .collect();
        let existing_commands: HashSet<String> = self
            .config
            .commands
            .iter()
            .map(|command| command.command.clone())
            .collect();
        let selected: Vec<PathBuf> = self
            .user_script_proposals
            .iter()
            .filter(|proposal| proposal.selected)
            .map(|proposal| proposal.relative_path.clone())
            .collect();
        let previous = self.config.clone();
        let mut added = 0usize;

        for relative_path in selected {
            let command_text = user_script_command(&relative_path);
            if existing_commands.contains(&command_text) {
                continue;
            }
            let base_id = user_script_id(&relative_path);
            let mut id = base_id.clone();
            let mut suffix = 2usize;
            while used_ids.contains(&id) {
                id = format!("{base_id}-{suffix}");
                suffix += 1;
            }
            used_ids.insert(id.clone());
            let name = relative_path
                .file_stem()
                .map(|stem| stem.to_string_lossy().replace(['_', '-'], " "))
                .unwrap_or_else(|| relative_path.display().to_string());
            self.config.commands.push(storage::Command {
                id,
                name,
                command: command_text,
                category: "user-scripts".to_string(),
                icon: "🐍".to_string(),
                is_favorite: false,
                shortcut: None,
                variant_group: None,
                group_name: None,
                variant_label: None,
                machine_specific: false,
                info: Some(format!("Importé depuis {}", relative_path.display())),
            });
            added += 1;
        }

        if added == 0 {
            self.set_status("Aucune nouvelle action à ajouter.", false);
            return;
        }
        if !self
            .config
            .categories
            .iter()
            .any(|category| category.id == "user-scripts")
        {
            self.config.categories.push(storage::Category {
                id: "user-scripts".to_string(),
                name: "Scripts utilisateur".to_string(),
                icon: "🐍".to_string(),
            });
        }
        match self.persist() {
            Ok(()) => {
                self.user_script_proposals.retain(|proposal| {
                    !self.config.commands.iter().any(|command| {
                        command.command == user_script_command(&proposal.relative_path)
                    })
                });
                self.set_status(format!("{added} action(s) ajoutée(s)."), false);
            }
            Err(error) => {
                self.config = previous;
                self.set_status(format!("Échec de sauvegarde: {error}"), true);
            }
        }
    }

    fn persist_user_scripts_directory(&mut self, directory: Option<&Path>) {
        let previous = self.config.default_settings.user_scripts_directory.clone();
        self.config.default_settings.user_scripts_directory = directory
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        match self.persist() {
            Ok(()) => {
                self.user_script_proposals.clear();
                self.user_script_scan_error = None;
                self.user_scripts_scanned = false;
                self.set_status("Dossier de scripts utilisateur mis à jour.", false);
            }
            Err(err) => {
                self.config.default_settings.user_scripts_directory = previous;
                self.set_status(format!("Échec de sauvegarde: {err}"), true);
            }
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
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.preferences_section,
                PreferencesSection::General,
                "Général",
            );
            ui.selectable_value(
                &mut self.preferences_section,
                PreferencesSection::Actions,
                "Actions et scripts",
            );
            ui.selectable_value(
                &mut self.preferences_section,
                PreferencesSection::Terminal,
                "Terminal",
            );
            ui.selectable_value(
                &mut self.preferences_section,
                PreferencesSection::Automations,
                "Automatisations",
            );
            ui.selectable_value(
                &mut self.preferences_section,
                PreferencesSection::Cleanup,
                "Nettoyage",
            );
            ui.selectable_value(
                &mut self.preferences_section,
                PreferencesSection::Models,
                "Modèles",
            );
            ui.selectable_value(
                &mut self.preferences_section,
                PreferencesSection::Docker,
                "Docker",
            );
        });
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
            match self.preferences_section {
                PreferencesSection::General => {
                    ui.strong("Général");
                    ui.label("Les préférences sont regroupées par espace de travail.");
                    let mut native_effects = self.config.default_settings.native_effects;
                    if ui
                        .checkbox(
                            &mut native_effects,
                            "Utiliser les effets de fenêtre natifs lorsqu'ils sont disponibles",
                        )
                        .changed()
                    {
                        self.config.default_settings.native_effects = native_effects;
                        if let Err(error) = self.persist() {
                            self.set_status(format!("Échec de sauvegarde: {error}"), true);
                        }
                    }
                    if ui.button("Diagnostiquer Python").clicked() {
                        let executable = crate::python_runtime::python_for_script(Path::new("."));
                        let message = match crate::python_runtime::diagnose_python(&executable) {
                            crate::python_runtime::PythonDiagnostic::Supported {
                                executable,
                                version,
                            } => format!(
                                "Python {}.{} pris en charge ({executable}).",
                                version.0, version.1
                            ),
                            crate::python_runtime::PythonDiagnostic::Missing { expected } => {
                                format!(
                                    "Python introuvable ({expected}); installez Python 3.10 à 3.13."
                                )
                            }
                            crate::python_runtime::PythonDiagnostic::Unsupported {
                                executable,
                                version,
                            } => format!(
                                "Python {}.{} non pris en charge ({executable}); utilisez 3.10 à 3.13.",
                                version.0, version.1
                            ),
                            crate::python_runtime::PythonDiagnostic::Unreadable { executable } => {
                                format!("Version Python illisible ({executable}).")
                            }
                        };
                        self.set_status(message, false);
                    }
                    if ui.button("Préparer la désinstallation").clicked() {
                        match crate::uninstall::prepare() {
                            Ok(_) => self.set_status(
                                "Intégrations retirées. Les données utilisateur sont conservées.",
                                false,
                            ),
                            Err(error) => self.set_status(
                                format!("Préparation de la désinstallation impossible: {error}"),
                                true,
                            ),
                        }
                    }
                    ui.separator();
                    ui.strong("Mises à jour");
                    match &self.update_state {
                        UpdateState::Disabled(message)
                        | UpdateState::Failed(message)
                        | UpdateState::Recovery(message)
                        | UpdateState::HandOff(message) => {
                            ui.label(message);
                        }
                        UpdateState::Available { manifest, asset } => {
                            ui.label(format!(
                                "Version {} disponible — {} octets",
                                manifest.version, asset.size
                            ));
                            ui.label(&manifest.notes);
                            ui.label(
                                "L'installation demande une confirmation explicite dans le paquet natif.",
                            );
                            if components::primary_button(ui, "Télécharger et installer").clicked()
                            {
                                self.active_dialog = Some(ActiveDialog {
                                    kind: dialogs::confirm(
                                        "Installer la mise à jour ?",
                                        format!(
                                            "Télécharger, vérifier puis installer DevToolBox {} ({} octets) ? L'application devra redémarrer.",
                                            manifest.version, asset.size
                                        ),
                                    ),
                                    on_confirm: Some(PendingAction::InstallUpdate {
                                        manifest: manifest.clone(),
                                        asset: asset.clone(),
                                    }),
                                });
                            }
                        }
                        UpdateState::Checking => {
                            ui.spinner();
                            ui.label("Recherche d'une mise à jour…");
                        }
                        UpdateState::UpToDate => {
                            ui.label("DevToolBox est à jour.");
                        }
                        UpdateState::Idle => {
                            ui.label("Recherche automatique après le premier rendu.");
                        }
                        UpdateState::Downloading => {
                            ui.label("Téléchargement…");
                        }
                        UpdateState::Verifying => {
                            ui.label("Vérification de la signature…");
                        }
                        UpdateState::Installing => {
                            ui.label("Installation…");
                        }
                        UpdateState::RestartRequired => {
                            ui.label("Redémarrage requis pour terminer la mise à jour.");
                        }
                    }
                    if !matches!(self.update_state, UpdateState::Checking)
                        && ui.button("Vérifier maintenant").clicked()
                    {
                        self.start_update_check();
                    }
                    return;
                }
                PreferencesSection::Actions => {
                    self.render_preferences_actions_header(ui);
                    ui.separator();
                }
                PreferencesSection::Docker => {
                    self.render_preferences_docker(ui);
                    return;
                }
                PreferencesSection::Terminal => {
                    ui.strong("Terminal");
                    ui.label("Aucun réglage spécifique pour le moment.");
                    return;
                }
                PreferencesSection::Automations => {
                    ui.strong("Automatisations");
                    ui.label("Aucun réglage spécifique pour le moment.");
                    return;
                }
                PreferencesSection::Cleanup => {
                    ui.strong("Nettoyage");
                    ui.label("Aucun réglage spécifique pour le moment.");
                    return;
                }
                PreferencesSection::Models => {
                    ui.strong("Modèles");
                    ui.label(
                        "Les réglages de bibliothèque et de fournisseurs restent accessibles dans Modèles → Réglages.",
                    );
                    return;
                }
            }

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
                                // ⏵/⏷ (U+23F5/23F7, emoji-icon-font) rather
                                // than ▸/▾ (U+25B8/25BE): the small triangles
                                // aren't covered by any font in the chain
                                // (see `crate::ui::fonts`) and rendered as
                                // tofu.
                                let toggle_label = if is_expanded { "⏷" } else { "⏵" };
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
        // Runs any Docker action deferred by last frame's confirm dialog
        // *before* anything else this frame builds — see
        // `execute_deferred_docker_action`'s doc comment. A no-op on every
        // other frame (the common case).
        self.execute_deferred_docker_action();
        self.drain_docker_fetch();

        if let Some(dialog) = self.active_dialog.take() {
            match dialogs::show(ui.ctx(), &dialog.kind) {
                DialogOutcome::Pending => {
                    self.active_dialog = Some(dialog);
                    return;
                }
                DialogOutcome::Accepted => {
                    if let Some(action) = dialog.on_confirm {
                        self.resolve_pending_action(action);
                    }
                    if self.deferred_docker_action.is_none() {
                        return;
                    }
                    // A Docker action was just deferred: fall through and
                    // render the rest of this frame instead of the usual
                    // early return, so the "in progress" status text
                    // `resolve_pending_action` just set is actually painted
                    // before `execute_deferred_docker_action` blocks at the
                    // start of the next frame. `request_repaint` guarantees
                    // that next frame happens even without further input.
                    ui.ctx().request_repaint();
                }
                DialogOutcome::Dismissed => {
                    // Cancel — the pending action (if any) is simply dropped;
                    // nothing was mutated by opening the dialog.
                    return;
                }
            }
        }

        self.drain_terminal_events();
        self.drain_action_events();
        self.drain_application_events();
        self.drain_cleanup_events();
        self.drain_model_events();
        self.drain_compose_events();
        self.drain_scan_events();
        self.drain_update_events();
        if !self.update_auto_checked {
            self.update_auto_checked = true;
            if crate::update::keys::configured() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0);
                let jitter = crate::update::service::machine_jitter_secs(&self.machine_id);
                if crate::update::service::should_auto_check(
                    crate::update::service::read_last_check(),
                    now,
                    jitter,
                ) {
                    crate::update::service::record_check(now);
                    self.start_update_check();
                }
            }
        }
        if self.update_rx.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.compose_running || self.compose_scanning || self.docker_fetching {
            // Same polling rationale as `cleanup_job`: the worker's events
            // only land on a repaint.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.cleanup_job.is_some() {
            // Same polling rationale as `terminal_running` below: the
            // cleanup thread's events only land on a repaint.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.models_job.is_some() {
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

        if ui.available_width() < theme::COMPACT_BREAKPOINT {
            egui::ScrollArea::horizontal()
                .id_salt("compact-navigation")
                .show(ui, |ui| self.render_navigation(ui, true));
            ui.separator();
            self.render_active_view(ui);
        } else {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(theme::NAV_WIDTH, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.render_navigation(ui, false),
                );
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_width((ui.available_width() - theme::GRID).max(320.0));
                    self.render_active_view(ui);
                });
            });
        }
    }

    fn render_navigation(&mut self, ui: &mut egui::Ui, compact: bool) {
        let render_items = |ui: &mut egui::Ui, this: &mut Self| {
            for (view, label) in [
                (ActiveView::Actions, "Actions"),
                (ActiveView::Terminal, "Terminal"),
                (ActiveView::Automations, "Automatisations"),
                (ActiveView::Cleanup, "Nettoyage"),
                (ActiveView::Models, "Modèles"),
            ] {
                ui.selectable_value(&mut this.active_view, view, label);
            }
            if this.docker_available {
                ui.selectable_value(&mut this.active_view, ActiveView::Docker, "Docker");
            }
            ui.selectable_value(
                &mut this.active_view,
                ActiveView::Preferences,
                "Préférences",
            );
        };

        if compact {
            ui.horizontal(|ui| {
                render_items(ui, self);
                self.render_about_button(ui);
            });
        } else {
            components::card(ui, |ui| {
                components::page_header(ui, "DevToolBox", "Vos outils, au même endroit");
            });
            render_items(ui, self);
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                self.render_about_button(ui);
                components::badge(ui, concat!("Version ", env!("CARGO_PKG_VERSION")));
            });
        }
    }

    fn render_about_button(&mut self, ui: &mut egui::Ui) {
        if ui.button("À propos").clicked() {
            self.active_dialog = Some(ActiveDialog {
                kind: dialogs::info(
                    "À propos de DevToolBox",
                    format!(
                        "DevToolBox {}\nLanceur de scripts et d'outils.",
                        env!("CARGO_PKG_VERSION")
                    ),
                ),
                on_confirm: None,
            });
        }
    }

    fn render_active_view(&mut self, ui: &mut egui::Ui) {
        match self.active_view {
            ActiveView::Actions => self.render_actions_view(ui),
            ActiveView::Terminal => self.render_terminal_view(ui),
            ActiveView::Automations => self.render_automations_view(ui),
            ActiveView::Cleanup => self.render_cleanup_view(ui),
            ActiveView::Models => self.render_models_view(ui),
            ActiveView::Docker => self.render_docker_view(ui),
            ActiveView::Preferences => self.render_preferences_view(ui),
        }
    }

    fn render_models_view(&mut self, ui: &mut egui::Ui) {
        let state = ModelsViewState {
            snapshot: self.models_snapshot.as_ref(),
            offers: &self.models_offers,
            progress: &self.models_progress,
            recovery: &self.models_recovery,
            guided: self.models_guided.as_ref(),
            terminal: self.models_terminal.as_ref(),
            error: self.models_error.as_deref(),
            loading: self.models_job == Some(ModelsJob::Inventory),
            busy: self.models_job.is_some(),
        };
        let actions = models_view::render(ui, &state, &mut self.models_ui);
        for action in actions {
            self.handle_models_action(action);
        }
    }

    fn render_actions_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("DevToolBox — Actions");

        if let Some(status) = &self.status {
            let kind = if status.is_error {
                components::MessageKind::Error
            } else {
                components::MessageKind::Success
            };
            components::status_message(ui, kind, &status.text);
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
        let user_scripts = self.config.default_settings.user_scripts_directory.trim();
        let user_scripts = (!user_scripts.is_empty()).then(|| Path::new(user_scripts));
        match terminal_view::launch_captured_with_user_scripts(&command, user_scripts, tx) {
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

    /// Hand `files` — or a fresh `$HOME` walk when `root` is `Some` — to a
    /// worker thread that resolves each one through `docker compose config`.
    ///
    /// Off the UI thread on purpose: the walk takes ~1 s here and each
    /// `config` call ~89 ms, so doing this inline would freeze the window for
    /// well over a second every time the Docker tab opens.
    fn start_compose_job(&mut self, root: Option<PathBuf>) {
        if self.compose_scanning {
            return;
        }
        self.compose_invocations = self.compose_invocations.saturating_add(1);
        // Same seam as `docker_actions_enabled`: a kittest harness must never
        // shell out. The flag is left `true` only in production.
        if !self.docker_actions_enabled {
            return;
        }
        let memorized = self.config.docker_stacks.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let outcome = root.as_deref().map(compose_view::discover);
            let files = match &outcome {
                Some(outcome) => outcome.files.clone(),
                None => memorized,
            };
            let entries = build_compose_entries(&files);
            let _ = tx.send(ComposeScanResult { entries, outcome });
        });
        self.compose_scan_rx = Some(rx);
        self.compose_scanning = true;
    }

    /// Collect a finished scan/reload, and persist the file list when it was
    /// a real scan.
    fn drain_scan_events(&mut self) {
        let Some(rx) = &self.compose_scan_rx else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        self.compose_scan_rx = None;
        self.compose_scanning = false;
        self.compose_stacks = result.entries;

        let Some(outcome) = result.outcome else {
            // A reload of the memorized list changes nothing on disk.
            return;
        };
        self.compose_scan_warning = outcome.warning.clone();
        // The scan result *replaces* the memorized list: a file the walk no
        // longer finds is genuinely gone from `$HOME`. One that is still
        // running from a vanished path is not lost either — `link_runs` gives
        // it a `Missing` row of its own, built from the container labels.
        let files: Vec<String> = self
            .compose_stacks
            .iter()
            .map(|stack| stack.file.clone())
            .collect();
        if files != self.config.docker_stacks {
            self.config.docker_stacks = files;
            if let Err(error) = self.persist() {
                self.set_status(format!("Échec de sauvegarde: {error}"), true);
                return;
            }
        }
        self.set_status(
            format!(
                "{} fichier(s) compose trouvé(s) en {} ms.",
                outcome.files.len(),
                outcome.elapsed_ms
            ),
            false,
        );
    }

    /// Spawn one detached `docker compose …` command, streaming its output
    /// into the inline log of the row it belongs to.
    ///
    /// Detached, unlike every Part 1 Docker action: `up -d` on a stack that
    /// has to pull images can run for minutes, which the blocking
    /// `run_docker` path (30 s ceiling, no output until it returns) cannot
    /// represent at all.
    /// Drop the log panel's content *and* its owner, which is what makes it
    /// disappear: `render_log_panel` keys its visibility on `log_target`, so
    /// clearing the lines alone would leave an empty panel anchored.
    fn close_compose_log(&mut self) {
        self.compose_log.clear();
        self.compose_log_target = None;
    }

    fn launch_compose(&mut self, target: &StackTarget, args: Vec<String>, label: &str) {
        if self.compose_running || self.compose_scanning {
            self.set_status("Une commande compose est déjà en cours.", true);
            return;
        }
        self.compose_invocations = self.compose_invocations.saturating_add(1);
        self.compose_log.clear();
        self.compose_log_target = Some(target.file.clone());
        if !self.docker_actions_enabled {
            return;
        }
        // The compose file's own directory: `up -d` resolves relative build
        // contexts and `env_file` paths against the current directory.
        let working_dir = Path::new(&target.file).parent().map(Path::to_path_buf);
        let (tx, rx) = std::sync::mpsc::channel();
        match terminal_view::launch_captured_program(
            "docker",
            &args,
            working_dir.as_deref(),
            label,
            tx,
        ) {
            Ok(_pid) => {
                self.compose_rx = Some(rx);
                self.compose_running = true;
                self.set_status(format!("{label}…"), false);
            }
            Err(error) => {
                self.compose_log.push(format!("(erreur: {error})"));
                self.set_status(format!("Échec du lancement: {error}"), true);
            }
        }
    }

    /// Drain the in-flight compose command's output into the inline log, and
    /// refetch the Docker snapshot once it settles so the row's state catches
    /// up with what the command just did.
    fn drain_compose_events(&mut self) {
        let Some(rx) = &self.compose_rx else {
            return;
        };
        let mut settled: Option<bool> = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                TerminalEvent::Started { .. } => {}
                TerminalEvent::Output(line) => self.compose_log.push(line),
                TerminalEvent::Finished { code } => {
                    self.compose_log.push(format_exit_line(code));
                    settled = Some(code == Some(0));
                }
                TerminalEvent::Failed(error) => {
                    self.compose_log.push(format!("(erreur: {error})"));
                    settled = Some(false);
                }
            }
        }
        if self.compose_log.len() > COMPOSE_LOG_MAX {
            let excess = self.compose_log.len() - COMPOSE_LOG_MAX;
            self.compose_log.drain(..excess);
        }

        let Some(succeeded) = settled else {
            return;
        };
        self.compose_rx = None;
        self.compose_running = false;
        if succeeded {
            self.set_status("Commande compose terminée.", false);
        } else {
            self.set_status("Commande compose en échec — voir le journal.", true);
        }
        // Whatever the outcome: `up -d` can fail *after* starting half the
        // services, so the row's state must be re-read, not inferred.
        self.refetch_docker();
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
                    self.terminal_lines.push_back(format_exit_line(code));
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
                    self.terminal_lines.push_back(format_exit_line(code));
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
        if !crate::platform::capabilities().automations {
            ui.label("Cette fonction n'est pas encore disponible sur macOS.");
            return;
        }
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

        let capabilities = crate::platform::capabilities();
        if !capabilities.cleanup && !capabilities.recommendations {
            ui.label(
                "Le nettoyage et les recommandations ne sont pas encore disponibles sur macOS.",
            );
            return;
        }

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

    /// « Docker » view: lazy first fetch on tab activation (Automations
    /// pattern at `render_automations_view`), consuming `docker_view::
    /// render`'s emitted `Vec<DockerAction>` the same way `render_cleanup_view`
    /// consumes `Vec<CleanupAction>` — `Refresh`/`Retry` refetch directly
    /// (read-only, never blocks meaningfully), the four destructive
    /// variants go through `open_docker_confirm` instead of running
    /// immediately.
    fn render_docker_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("DevToolBox — Docker");

        if let Some(status) = &self.status {
            let color = if status.is_error {
                egui::Color32::from_rgb(0xC4, 0x2B, 0x1C)
            } else {
                egui::Color32::from_rgb(0x1B, 0x5E, 0x20)
            };
            ui.colored_label(color, &status.text);
        }

        if self.docker.is_none() && !self.docker_fetching {
            self.refetch_docker();
        }

        // --- Stacks -------------------------------------------------------
        // Probed once per process: `docker compose version` costs a fork, and
        // a plugin does not appear or vanish while the app runs.
        if self.compose_plugin.is_none() {
            self.compose_plugin = Some(if self.docker_actions_enabled {
                compose_view::plugin_available()
            } else {
                false
            });
        }
        // One-shot reload of the memorized list, so reopening the tab shows
        // the stacks found by the last scan without re-walking `$HOME`.
        if !self.compose_loaded {
            self.compose_loaded = true;
            if !self.config.docker_stacks.is_empty() {
                self.start_compose_job(None);
            }
        }

        let (snapshot, error) = match &self.docker {
            Some(Ok(snapshot)) => (Some(snapshot), None),
            Some(Err(err)) => (None, Some(err.as_str())),
            None => (None, None),
        };
        // The runs come from the snapshot's containers; when the snapshot
        // itself failed there is nothing to link against, and every row must
        // read `Unknown` rather than claim « arrêtée ».
        let stacks = match snapshot {
            Some(snapshot) => {
                compose_view::link_runs(&self.compose_stacks, &snapshot.containers, &|file| {
                    Path::new(file).is_file()
                })
            }
            None => self
                .compose_stacks
                .iter()
                .cloned()
                .map(|mut stack| {
                    stack.runs.clear();
                    stack.state = StackState::Unknown;
                    stack
                })
                .collect(),
        };
        let container_owners = snapshot
            .map(docker_view::container_port_owners)
            .unwrap_or_default();
        let conflicts = compose_view::all_conflicts(container_owners.clone(), &stacks);
        let compose_state = ComposeViewState {
            stacks: &stacks,
            conflicts: &conflicts,
            plugin_available: self.compose_plugin.unwrap_or(false),
            scanning: self.compose_scanning,
            busy: self.compose_running
                || self.deferred_docker_action.is_some()
                || self.docker_fetching,
            log: &self.compose_log,
            log_target: self.compose_log_target.as_deref(),
            scan_warning: self.compose_scan_warning.as_deref(),
        };
        // The log panel first: egui shrinks the parent's cursor when a panel
        // claims its edge, so everything below has to be laid out after it.
        let mut stack_actions = compose_view::render_log_panel(ui, &compose_state);
        stack_actions.extend(compose_view::render(ui, &compose_state));
        // The declared owners the container sections need to badge their own
        // rows — same computation the conflicts above already ran, kept as a
        // separate slice because `docker_view` owns its side of the merge.
        let declared = compose_view::declared_owners(&stacks);
        let state = DockerViewState {
            snapshot,
            error,
            busy: self.deferred_docker_action.is_some()
                || self.compose_running
                || self.docker_fetching,
            dormant_after_days: self.config.default_settings.dormant_after_days,
            now_epoch_secs: now_epoch_secs(),
            extra_port_owners: &declared,
            selection: &self.docker_selection,
            batch_report: &self.docker_batch_report,
            active_list: self.docker_active_list,
            host_ports: self.docker_host_ports.as_deref(),
            port_plan: self.docker_port_plan.as_ref(),
            port_edits: &self.docker_port_edits,
        };
        let actions = docker_view::render(ui, &state);
        for action in actions {
            match action {
                DockerAction::Refresh | DockerAction::Retry => {
                    // A manual refresh is one of the two moments the plan
                    // allows the report to disappear (the other being the
                    // next selection change).
                    self.docker_batch_report.clear();
                    self.refetch_docker();
                }
                DockerAction::ToggleSelection(key) => {
                    if !self.docker_selection.remove(&key) {
                        self.docker_selection.insert(key);
                    }
                    self.docker_batch_report.clear();
                    // Re-validated on every toggle, not only on refetch:
                    // unticking a container has to untick, on the same
                    // frame, every image that was only selectable because of
                    // it.
                    self.prune_docker_selection();
                }
                DockerAction::SelectDormant => {
                    let cutoff = docker_view::cutoff_epoch(
                        now_epoch_secs(),
                        self.config.default_settings.dormant_after_days,
                    );
                    if let Some(Ok(snapshot)) = self.docker.as_ref() {
                        self.docker_selection = docker_view::dormant_selection(snapshot, cutoff);
                    }
                    self.docker_batch_report.clear();
                }
                DockerAction::ClearSelection => {
                    self.docker_selection.clear();
                    self.docker_batch_report.clear();
                }
                // Pure view state: no refetch, and the batch report survives
                // — switching tabs to check what a batch did to the volumes
                // must not be what makes the report disappear.
                DockerAction::SelectList(list) => self.docker_active_list = list,
                // Not destructive — must never go through open_docker_confirm
                // (no confirm dialog), unlike the four Remove*/Stop* actions.
                DockerAction::ComputeVolumeSizes => {
                    self.deferred_docker_action = Some(DeferredDockerAction::ComputeVolumeSizes);
                    self.set_status("Calcul des tailles des volumes…", false);
                    ui.ctx().request_repaint();
                }
                // Also non-destructive: it only reads the host's sockets.
                DockerAction::ScanHostPorts => {
                    self.deferred_docker_action = Some(DeferredDockerAction::ScanHostPorts);
                    self.set_status("Analyse des ports de l'hôte…", false);
                    ui.ctx().request_repaint();
                }
                // Non-destructive too, and cheap enough to run inline: the
                // planner is pure arithmetic over lists already in memory —
                // no `docker` call, no `netstat`, nothing to defer.
                DockerAction::PlanPortReassignment => {
                    let declarations = compose_view::declared_ports(&stacks);
                    // Container owners only — the clone made above, and not
                    // a fresh `snapshot` read, because touching `snapshot`
                    // here would hold `self.docker` borrowed across the whole
                    // dispatch loop and lock every `set_status` out of it.
                    //
                    // Container owners are also the *right* input: they are
                    // the ones carrying a `com.docker.compose.*` declaration
                    // key, which is how the planner tells which side of a
                    // collision is actually up. `declared_owners` sets no such
                    // key, so feeding it in would make every declared stack
                    // look like a container created outside compose.
                    let listeners = self.docker_host_ports.clone().unwrap_or_default();
                    let plan =
                        port_plan::plan_reassignment(&declarations, &container_owners, &listeners);
                    let message = if plan.is_empty() {
                        "Aucun conflit de port à corriger.".to_string()
                    } else {
                        format!(
                            "{} réattribution(s) proposée(s), {} conflit(s) non réglable(s).",
                            plan.moves.len(),
                            plan.blocked.len()
                        )
                    };
                    // A report from a previous application describes files as
                    // they were before this plan was computed; keeping it next
                    // to a fresh proposal would read as its result.
                    self.docker_port_edits.clear();
                    self.docker_port_plan = Some(plan);
                    self.set_status(message, false);
                }
                DockerAction::ClearPortPlan => {
                    self.docker_port_plan = None;
                    self.docker_port_edits.clear();
                }
                destructive => self.open_docker_confirm(destructive),
            }
        }

        // Deliberately after the Docker dispatch: every arm below mutates
        // `self`, which the `snapshot` borrow held by `DockerViewState`
        // forbids until `docker_view::render` has returned.
        for action in stack_actions {
            match action {
                StackAction::Scan => match compose_scan_root() {
                    Some(root) => {
                        self.set_status("Recherche des fichiers compose…", false);
                        self.start_compose_job(Some(root));
                    }
                    None => self.set_status("Dossier personnel introuvable.", true),
                },
                StackAction::Up(target) => {
                    let args = compose_view::up_args(&target);
                    self.launch_compose(&target, args, "Démarrage de la stack");
                }
                StackAction::Stop(target) => {
                    let args = compose_view::stop_args(&target);
                    self.launch_compose(&target, args, "Arrêt de la stack");
                }
                // The only destructive compose action — `down` removes the
                // containers and the project network, so it is confirmed like
                // `docker rm`/`docker rmi` are.
                StackAction::Down(target) => {
                    let label = target
                        .project
                        .clone()
                        .unwrap_or_else(|| compose_view::default_project_name(&target.file));
                    self.active_dialog = Some(ActiveDialog {
                        kind: dialogs::confirm(
                            "Détruire la stack".to_string(),
                            format!(
                                "Le projet « {label} » va être détruit (docker compose down). Les volumes sont conservés. Continuer ?"
                            ),
                        ),
                        on_confirm: Some(PendingAction::ComposeDown(target)),
                    });
                }
                StackAction::CloseLog => self.close_compose_log(),
                StackAction::Forget(file) => {
                    self.config
                        .docker_stacks
                        .retain(|memorized| *memorized != file);
                    self.compose_stacks.retain(|stack| stack.file != file);
                    if self.compose_log_target.as_deref() == Some(file.as_str()) {
                        self.close_compose_log();
                    }
                    match self.persist() {
                        Ok(()) => self.set_status("Fichier oublié.", false),
                        Err(error) => {
                            self.set_status(format!("Échec de sauvegarde: {error}"), true)
                        }
                    }
                }
            }
        }

        // The worker cannot wake egui by itself because it deliberately owns
        // no UI context. Keep polling while it runs so the completed snapshot
        // is integrated even when the user does not move the mouse or press a
        // key after opening the tab.
        if self.docker_fetching {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

/// Root of the compose-file walk: the user's home directory.
///
/// Read from the environment rather than through `platform::` — the walk is a
/// user-facing "scan my projects" gesture, so it must start where the user's
/// files are, not in the app's config/data directory.
fn compose_scan_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Wall-clock seconds since the Unix epoch, for the Docker tab's dormancy
/// cutoff. A clock before 1970 (or an unset RTC) reads as 0, which simply
/// puts every date in the future and badges nothing — the same fail-safe the
/// rest of the dormancy path uses.
fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
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
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let dark = ui.visuals().dark_mode;
        let desired = native_window::decide(native_window::current_inputs(
            self.config.default_settings.native_effects,
        ));
        if desired != self.native_profile {
            match native_window::apply(frame, self.native_profile, desired, dark) {
                Ok(profile) => {
                    self.native_profile = profile;
                    self.native_effect_warning_logged = false;
                    theme::set_native_material(
                        ui.ctx(),
                        profile != native_window::NativeProfile::Opaque,
                    );
                }
                Err(error) => {
                    self.native_profile = native_window::NativeProfile::Opaque;
                    theme::set_native_material(ui.ctx(), false);
                    if !self.native_effect_warning_logged {
                        log::warn!(
                            "native window material unavailable; using opaque fallback: {error}"
                        );
                        self.native_effect_warning_logged = true;
                    }
                }
            }
        }
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

    fn echo_test_command(message: &str) -> String {
        if cfg!(windows) {
            format!("cmd.exe /C echo {message}")
        } else {
            format!("echo {message}")
        }
    }

    fn sample_config() -> Config {
        Config {
            docker_stacks: Vec::new(),
            version: "0.1.0".to_string(),
            default_settings: Settings {
                show_categories: true,
                icon_size: 32,
                theme: "light".to_string(),
                launch_at_startup: false,
                show_descriptions: true,
                dormant_after_days: 60,
                user_scripts_directory: String::new(),
                native_effects: true,
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
                    info: None,
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
                    info: None,
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
            info: None,
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
            info: None,
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
            info: None,
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
            info: None,
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
    fn preferences_use_one_per_workspace_subtab_and_persist_user_scripts() {
        let config_path = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-preferences-tabs.json",
            std::process::id()
        ));
        let app = EguiApp::new_for_test(sample_config(), config_path.clone());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );

        harness.get_by_label("Préférences").click();
        harness.run();
        for label in ["Général", "Actions et scripts"] {
            assert!(
                harness.query_by_label(label).is_some(),
                "onglet absent: {label}"
            );
        }
        for label in ["Terminal", "Automatisations", "Nettoyage", "Modèles"] {
            assert!(
                harness.get_all_by_label(label).nth(1).is_some(),
                "sous-onglet absent: {label}"
            );
        }
        assert!(harness.query_by_label("Docker").is_some());
        assert!(harness.query_by_label("Catégories").is_some());
        assert!(harness
            .query_by_label("Dossier de scripts utilisateur")
            .is_some());
        assert!(harness.query_by_label("Parcourir…").is_some());
        assert!(harness.query_by_label("Scanner").is_some());

        harness.get_by_label("Général").click();
        harness.run();
        assert!(harness.query_by_label("Catégories").is_none());
        harness.get_by_label("Actions et scripts").click();
        harness.run();

        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        harness
            .state_mut()
            .persist_user_scripts_directory(Some(repository_root));

        let reloaded = storage::json::load_from(&config_path).expect("reload persisted config");
        assert_eq!(
            reloaded.default_settings.user_scripts_directory,
            repository_root.display().to_string(),
            "saving the Scripts preference must persist the selected root"
        );
        let _ = std::fs::remove_file(config_path);
    }

    #[test]
    fn script_library_scan_proposes_python_files_and_injects_selected_actions() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-script-library",
            std::process::id()
        ));
        let library = dir.join("library");
        for relative in [
            "backup.py",
            "tools/report.py",
            "tools/__init__.py",
            ".venv/ignored.py",
            "notes.txt",
        ] {
            let path = library.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "print('ok')").unwrap();
        }

        let found = scan_user_python_scripts(&library).expect("scan script library");
        assert_eq!(
            found,
            vec![PathBuf::from("backup.py"), PathBuf::from("tools/report.py")]
        );

        let config_path = dir.join("config.json");
        let mut app = EguiApp::new_for_test(sample_config(), config_path.clone());
        app.persist_user_scripts_directory(Some(&library));
        app.user_script_proposals = found
            .into_iter()
            .map(|relative_path| UserScriptProposal {
                relative_path,
                selected: true,
            })
            .collect();
        app.inject_selected_user_scripts();

        let reloaded = storage::json::load_from(&config_path).expect("reload imported actions");
        assert!(reloaded
            .categories
            .iter()
            .any(|category| category.id == "user-scripts"));
        assert!(reloaded
            .commands
            .iter()
            .any(|command| command.command == "@python \"backup.py\""));
        assert!(reloaded
            .commands
            .iter()
            .any(|command| command.command == "@python \"tools/report.py\""));

        let _ = std::fs::remove_dir_all(dir);
    }

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

        harness.get_by_label("Information").focus();
        harness.run();
        harness
            .get_by_label("Information")
            .type_text("Ouvre la calculatrice");
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
        assert_eq!(created.info.as_deref(), Some("Ouvre la calculatrice"));

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
            assert_eq!(
                form.info, "",
                "an info-less command must open with an empty Information field"
            );
        }

        harness.get_by_label("raccourci").focus();
        harness.run();
        harness.get_by_label("raccourci").type_text("Ctrl+Shift+N");
        harness.run();

        harness.get_by_label("Information").focus();
        harness.run();
        harness
            .get_by_label("Information")
            .type_text("Éditeur de texte simple");
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
        assert_eq!(updated.info.as_deref(), Some("Éditeur de texte simple"));
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
            .type_text(&echo_test_command("hello-from-kittest-terminal-view"));
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

    #[test]
    fn format_exit_line_spells_out_the_exit_status_instead_of_the_debug_form() {
        assert_eq!(format_exit_line(Some(0)), "(terminé — succès)");
        assert_eq!(format_exit_line(Some(2)), "(terminé — code 2)");
        assert_eq!(
            format_exit_line(None),
            "(terminé — interrompu par un signal)",
            "a signal-killed child must not surface as the raw `None` debug form"
        );
    }

    // -- Optional per-command info badge ------------------------------------

    #[test]
    fn badge_message_combines_the_disabled_diagnostic_and_the_free_text_info() {
        assert_eq!(badge_message(None, None), None);
        assert_eq!(
            badge_message(None, Some("Nécessite le VPN")).as_deref(),
            Some("Nécessite le VPN")
        );
        assert_eq!(
            badge_message(Some("Non configuré"), None).as_deref(),
            Some("Non configuré")
        );
        assert_eq!(
            badge_message(Some("Non configuré"), Some("Nécessite le VPN")).as_deref(),
            Some("Non configuré\n\nNécessite le VPN"),
            "the unconfigured diagnostic must come first, then the free-text note"
        );
        assert_eq!(
            badge_message(None, Some("   ")),
            None,
            "a whitespace-only info string must not raise a badge"
        );
    }

    #[test]
    fn a_commands_info_string_reaches_its_card_and_its_variants() {
        let mut config = sample_config();
        config.default_settings.show_categories = false;
        config.commands.push(Command {
            id: "vpn-card".into(),
            name: "VPN".into(),
            command: "vpn up".into(),
            category: "system".into(),
            icon: "🔧".into(),
            is_favorite: true,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: false,
            info: Some("Nécessite le VPN".into()),
        });
        config.commands.push(Command {
            id: "grouped-card".into(),
            name: "Rapport complet".into(),
            command: "report --full".into(),
            category: "system".into(),
            icon: "🔧".into(),
            is_favorite: true,
            shortcut: None,
            variant_group: Some("report".into()),
            group_name: Some("Rapport".into()),
            variant_label: Some("Complet".into()),
            machine_specific: false,
            info: Some("Prend 3 minutes".into()),
        });

        let groups = build_display_groups(&config, &MachineCommands::default(), "laptop-x");
        let cards: Vec<&CardData> = groups.iter().flat_map(|g| &g.cards).collect();

        let simple = cards
            .iter()
            .find(|c| c.command_id == "vpn-card")
            .expect("the 'vpn-card' card must be present");
        assert_eq!(simple.info.as_deref(), Some("Nécessite le VPN"));

        let grouped = cards
            .iter()
            .find(|c| c.group_name.as_deref() == Some("Rapport"))
            .expect("the grouped 'Rapport' card must be present");
        assert_eq!(
            grouped.variants[0].info.as_deref(),
            Some("Prend 3 minutes"),
            "a grouped card carries its info per variant, not on the group itself"
        );
    }

    #[test]
    fn a_configured_card_with_an_info_string_still_renders_its_badge() {
        let mut config = sample_config();
        config.default_settings.show_categories = false;
        config.commands.push(Command {
            id: "vpn-card".into(),
            name: "VPN".into(),
            command: "vpn up".into(),
            category: "system".into(),
            icon: "🔧".into(),
            is_favorite: true,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: false,
            info: Some("Nécessite le VPN".into()),
        });

        let app = EguiApp::new_for_test(config, PathBuf::from("unused-config.json"));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut EguiApp| {
                    app.ui_content(ui);
                },
                app,
            );
        harness.run();

        assert!(
            harness.query_by_label("Nécessite le VPN").is_some(),
            "a configured card whose command carries an info string must expose the badge"
        );
        assert!(
            harness.query_by_label("Bloc-notes").is_some(),
            "the info-less sample cards must still render"
        );
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
            command: echo_test_command("hello-from-card-click"),
            category: "system".into(),
            icon: "🔧".into(),
            is_favorite: false,
            shortcut: None,
            variant_group: None,
            group_name: None,
            variant_label: None,
            machine_specific: false,
            info: None,
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
            info: None,
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
                &echo_test_command("hello-from-pro"),
                "system",
                true,
            ),
            variant_command(
                "sync-perso",
                "sync",
                "Synchroniser",
                "Perso",
                &echo_test_command("hello-from-perso"),
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

    /// A grouped card's body launches the selected variant, exactly like a
    /// simple card's — the behavior `render_card_shell` now shares between
    /// both shapes instead of only the simple one having it.
    #[test]
    fn clicking_a_grouped_cards_body_launches_the_selected_variant() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "variant-body-click"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let commands = vec![
            variant_command(
                "sync-pro",
                "sync",
                "Synchroniser",
                "Pro",
                &echo_test_command("hello-from-pro"),
                "system",
                true,
            ),
            variant_command(
                "sync-perso",
                "sync",
                "Synchroniser",
                "Perso",
                &echo_test_command("hello-from-perso"),
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
        harness.get_by_value("Pro").click();
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Perso")
            .click();
        harness.run();

        // The card body itself — not the "Lancer" button.
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Synchroniser")
            .click();
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
            "clicking a grouped card's body must launch the selected variant; got {:?}",
            harness.state().status.as_ref().map(|s| &s.text)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The body click obeys the same configuration guard as "Lancer": an
    /// unconfigured selected variant must stay inert.
    #[test]
    fn clicking_a_grouped_cards_body_is_inert_when_the_selected_variant_is_unconfigured() {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-test-{}-{}",
            std::process::id(),
            "variant-body-click-unconfigured"
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("config.json");

        let mut pro = variant_command(
            "sync-pro",
            "sync",
            "Synchroniser",
            "Pro",
            "echo hello-from-pro",
            "system",
            true,
        );
        // No override exists for "test-machine" in the empty `MachineCommands`
        // used by `new_for_test`, so this variant resolves as unconfigured.
        pro.machine_specific = true;
        let config = config_with_commands(vec![pro]);

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
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, "Synchroniser")
            .click();
        harness.run();

        assert!(
            harness.state().action_running.is_none(),
            "clicking the body of a card whose selected variant is unconfigured must not launch"
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
        harness.get_by_label("⏵").click();
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
        harness.get_by_label("⏵").click();
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

    /// One removable-only container (`Exited`: `is_removable() == true`,
    /// `is_stoppable() == false`) and nothing else, so the harness sees
    /// exactly one enabled « Supprimer » button and no « Arrêter » — no
    /// ambiguity about which button a bare `get_by_label` click hits.
    fn sample_docker_snapshot() -> DockerSnapshot {
        DockerSnapshot {
            containers: vec![docker_view::ContainerEntry {
                id: "c1".to_string(),
                name: "web".to_string(),
                image: "nginx:alpine".to_string(),
                state: docker_view::ContainerState::Exited,
                status: "Exited (0) 2 hours ago".to_string(),
                rw_size: "767kB".to_string(),
                ports: Vec::new(),
                last_activity: None,
                compose_project: None,
                compose_files: Vec::new(),
                compose_service: None,
                declared_host_ports: std::collections::BTreeSet::new(),
                exit_code: None,
            }],
            images: vec![],
            volumes: vec![],
        }
    }

    #[test]
    fn docker_tab_hidden_when_unavailable_and_shown_when_available() {
        let (mut app, dir) = cleanup_test_app("docker-visibility-off");
        app.docker_available = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);
        harness.run();
        assert!(harness.query_by_label("Docker").is_none());
        let _ = std::fs::remove_dir_all(dir);

        let (mut app, dir) = cleanup_test_app("docker-visibility-on");
        app.docker_available = true;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);
        harness.run();
        assert!(harness.query_by_label("Docker").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn first_docker_render_starts_loading_without_a_snapshot() {
        let (mut app, dir) = cleanup_test_app("docker-async-first-load");
        app.docker_available = true;
        app.active_view = ActiveView::Docker;
        // Keep the test hermetic: neither lazy Compose probing nor Docker
        // collection may invoke the host's real client.
        app.compose_plugin = Some(false);
        app.compose_loaded = true;

        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);
        harness.run_steps(1);

        assert!(harness.state().docker_fetching);
        assert!(harness.state().docker.is_none());
        assert_eq!(
            harness
                .query_all_by_label("Chargement des données Docker…")
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn completed_docker_fetch_replaces_the_cache_and_prunes_selection() {
        let (mut app, dir) = cleanup_test_app("docker-async-result");
        app.docker = Some(Ok(sample_docker_snapshot()));
        app.docker_selection = HashSet::from([
            SelectionKey::container("c1"),
            SelectionKey::container("disparu"),
        ]);
        let (tx, rx) = std::sync::mpsc::channel();
        app.docker_fetching = true;
        app.docker_fetch_rx = Some(rx);
        tx.send(Ok(DockerSnapshot::default())).unwrap();

        app.drain_docker_fetch();

        assert!(!app.docker_fetching);
        assert!(app.docker_fetch_rx.is_none());
        assert!(matches!(
            app.docker,
            Some(Ok(DockerSnapshot {
                ref containers,
                ref images,
                ref volumes,
            })) if containers.is_empty() && images.is_empty() && volumes.is_empty()
        ));
        assert!(app.docker_selection.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_keeps_the_cached_snapshot_visible_while_loading() {
        let (mut app, dir) = cleanup_test_app("docker-async-refresh-cache");
        app.docker = Some(Ok(sample_docker_snapshot()));

        app.refetch_docker();

        assert!(app.docker_fetching);
        assert!(matches!(app.docker, Some(Ok(_))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn docker_destructive_action_confirms_and_executes_exactly_once() {
        let (mut app, dir) = cleanup_test_app("docker-dialog");
        app.docker_available = true;
        // Pre-populated so the lazy `if self.docker.is_none()` fetch in
        // `render_docker_view` never fires — the harness must not shell out
        // to a real `docker` binary (Phase 3 acceptance criteria).
        app.docker = Some(Ok(sample_docker_snapshot()));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        harness.get_by_label("Supprimer").click();
        harness.run_steps(2);
        assert!(matches!(
            harness.state().active_dialog,
            Some(ActiveDialog {
                on_confirm: Some(PendingAction::RemoveContainer { .. }),
                ..
            })
        ));
        assert_eq!(harness.state().docker_action_invocations, 0);

        // « Non » closes the dialog and executes nothing.
        harness.get_by_label("Non").click();
        harness.run_steps(2);
        assert!(harness.state().active_dialog.is_none());
        assert_eq!(harness.state().docker_action_invocations, 0);

        // « Oui » only defers the action (`deferred_docker_action`) on its
        // own frame — the same frame that paints the "Suppression de …"
        // status — and `execute_deferred_docker_action` only actually runs
        // it at the very start of the *next* frame (see that method's doc
        // comment). `run_steps(1)` processes the Oui click's frame alone;
        // the extra `harness.step()` processes the following frame where
        // the deferred action is executed.
        harness.get_by_label("Supprimer").click();
        harness.run_steps(2);
        harness.get_by_label("Oui").click();
        harness.run_steps(1);
        assert_eq!(harness.state().docker_action_invocations, 0);
        harness.step();
        assert_eq!(harness.state().docker_action_invocations, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn batch_delete_goes_through_the_same_confirm_and_executes_exactly_once() {
        let (mut app, dir) = cleanup_test_app("docker-batch");
        app.docker_available = true;
        app.docker = Some(Ok(sample_docker_snapshot()));
        app.docker_selection = HashSet::from([SelectionKey::container("c1")]);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        harness.get_by_label("Supprimer la sélection").click();
        // `run()` rather than a fixed step count: `egui::Modal` centres
        // itself only once it knows its own size, so its first frame lands
        // off-centre and a click queued from that layout would miss the
        // buttons and read as a backdrop dismissal.
        harness.run();
        // A batch is destructive like any other deletion: same blocking
        // dialog, same deferral, no shortcut.
        assert!(matches!(
            harness.state().active_dialog,
            Some(ActiveDialog {
                on_confirm: Some(PendingAction::DeleteSelection(_)),
                ..
            })
        ));
        assert_eq!(harness.state().docker_action_invocations, 0);

        harness.get_by_label("Oui").click();
        harness.run_steps(1);
        assert_eq!(harness.state().docker_action_invocations, 0);
        harness.step();
        assert_eq!(harness.state().docker_action_invocations, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_selection_is_pruned_against_every_new_snapshot() {
        let (mut app, dir) = cleanup_test_app("docker-prune");
        app.docker = Some(Ok(sample_docker_snapshot()));
        app.docker_selection = HashSet::from([
            SelectionKey::container("c1"),
            // Removed from another terminal between two fetches.
            SelectionKey::container("disparu"),
        ]);
        app.prune_docker_selection();
        assert_eq!(
            app.docker_selection,
            HashSet::from([SelectionKey::container("c1")])
        );

        // A failed fetch leaves nothing to validate against: the selection
        // must not survive it.
        app.docker = Some(Err("daemon Docker inaccessible".to_string()));
        app.prune_docker_selection();
        assert!(app.docker_selection.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A stopped stack and a running one, both memorized, with the container
    /// carrying the compose labels that `link_runs` matches on.
    ///
    /// `with_missing` is opt-in because a `Missing` row renders its own
    /// (disabled) « Détruire » button, which would make a bare
    /// `get_by_label` click ambiguous.
    fn compose_test_app(name: &str, with_missing: bool) -> (EguiApp, PathBuf) {
        let (mut app, dir) = cleanup_test_app(name);
        app.docker_available = true;
        // The lazy probe must not shell out to `docker compose version`.
        app.compose_plugin = Some(true);
        app.compose_loaded = true;
        let running_file = "/tmp/lab/docker-compose.yml".to_string();
        app.config.docker_stacks = vec![running_file.clone()];
        app.compose_stacks = vec![StackEntry {
            file: running_file.clone(),
            project: "lab".to_string(),
            services: Vec::new(),
            runs: Vec::new(),
            state: StackState::Stopped,
            error: None,
        }];
        if with_missing {
            app.config
                .docker_stacks
                .push("/gone/compose.yml".to_string());
            let mut gone = StackEntry::failed("/gone/compose.yml", "fichier introuvable");
            gone.state = StackState::Missing;
            app.compose_stacks.push(gone);
        }
        let mut container = docker_view::ContainerEntry {
            id: "lab1".to_string(),
            name: "lab-web-1".to_string(),
            image: "nginx:alpine".to_string(),
            state: docker_view::ContainerState::Running,
            status: "Up 4 hours".to_string(),
            rw_size: "1MB".to_string(),
            ports: Vec::new(),
            last_activity: None,
            compose_project: Some("lab".to_string()),
            compose_files: vec![running_file],
            compose_service: None,
            declared_host_ports: std::collections::BTreeSet::new(),
            exit_code: None,
        };
        container.compose_project = Some("lab".to_string());
        app.docker = Some(Ok(DockerSnapshot {
            containers: vec![container],
            images: vec![],
            volumes: vec![],
        }));
        (app, dir)
    }

    #[test]
    fn destroying_a_stack_confirms_first_and_launches_exactly_one_command() {
        let (app, dir) = compose_test_app("compose-down", false);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 900.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        harness.get_by_label("Détruire").click();
        harness.run_steps(2);
        // `down` is the only destructive compose action, so it is confirmed
        // like `docker rm`/`docker rmi` are — `up -d` and `stop` are not.
        assert!(matches!(
            harness.state().active_dialog,
            Some(ActiveDialog {
                on_confirm: Some(PendingAction::ComposeDown(_)),
                ..
            })
        ));
        assert_eq!(harness.state().compose_invocations, 0);

        // `run()`, not `run_steps`: the modal needs a settled frame before
        // its centered position is final, and clicking a stale rect lands on
        // the backdrop — which `Modal::should_close` reads as a cancel.
        harness.run();
        harness.get_by_label("Oui").click();
        harness.run_steps(2);
        assert!(harness.state().active_dialog.is_none());
        assert_eq!(harness.state().compose_invocations, 1);
        // The seam held: nothing was actually spawned, so no command is in
        // flight and the log stays empty.
        assert!(!harness.state().compose_running);
        assert!(harness.state().compose_log.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_log_panel_is_anchored_and_dismissable_without_touching_the_stack() {
        let (mut app, dir) = compose_test_app("compose-log-panel", false);
        app.compose_log_target = Some("/tmp/lab/docker-compose.yml".to_string());
        app.compose_log = vec!["Container lab-web-1 Removed".to_string()];
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 900.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        assert!(harness.query_by_label("Journal — lab").is_some());

        harness.get_by_label("✕").click();
        harness.run();
        // The panel is gone, and so is its owner — clearing only the lines
        // would leave an empty strip anchored to the bottom edge.
        assert!(harness.state().compose_log.is_empty());
        assert!(harness.state().compose_log_target.is_none());
        assert!(harness.query_by_label("Journal — lab").is_none());
        // Dismissing a log is not forgetting a stack: the row it belonged to
        // is untouched, memorized list included.
        assert_eq!(harness.state().compose_stacks.len(), 1);
        assert_eq!(harness.state().config.docker_stacks.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn forgetting_a_vanished_file_drops_it_from_the_memorized_list_and_persists() {
        let (app, dir) = compose_test_app("compose-forget", true);
        let path = app
            .config_path
            .clone()
            .expect("test app writes to a real path");
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 900.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        harness.get_by_label("Oublier").click();
        harness.run_steps(2);
        assert_eq!(
            harness.state().config.docker_stacks,
            vec!["/tmp/lab/docker-compose.yml".to_string()]
        );
        assert!(harness
            .state()
            .compose_stacks
            .iter()
            .all(|stack| stack.file != "/gone/compose.yml"));
        // Persisted, not just dropped in memory: without the write the dead
        // row would come back on the next launch.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("/gone/compose.yml"));
        assert!(written.contains("/tmp/lab/docker-compose.yml"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scanning_never_shells_out_when_the_seam_is_closed() {
        let (app, dir) = compose_test_app("compose-scan", false);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 900.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        harness.get_by_label("Scanner").click();
        harness.run_steps(2);
        assert_eq!(harness.state().compose_invocations, 1);
        // `docker_actions_enabled` is false in every test app, so the worker
        // thread is never spawned and the flag never latches.
        assert!(!harness.state().compose_scanning);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scan_host_ports_never_opens_a_dialog_and_executes_exactly_once() {
        let (mut app, dir) = cleanup_test_app("docker-scan-host-ports");
        app.docker_available = true;
        app.docker = Some(Ok(sample_docker_snapshot()));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        harness.get_by_label("Ports (0)").click();
        harness.run();
        assert_eq!(harness.state().docker_active_list, DockerList::Ports);
        harness.get_by_label("Scanner les ports de l'hôte").click();
        // Same two-frame shape as `ComputeVolumeSizes` above, and for the
        // same reason: the click stashes the deferred action, the next frame
        // runs it.
        harness.run_steps(1);
        assert!(
            harness.state().active_dialog.is_none(),
            "ScanHostPorts must never open a confirm dialog — it reads sockets, it writes nothing"
        );
        assert_eq!(harness.state().docker_action_invocations, 0);
        harness.step();
        assert_eq!(harness.state().docker_action_invocations, 1);
        // `docker_actions_enabled` is false in a test app, so nothing was
        // actually spawned and the result stays `None` — the point here is
        // the dispatch, not `netstat` itself (covered in `crate::net`).
        assert!(harness.state().docker_host_ports.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Two real compose files, both publishing 8080. Real files because
    /// `link_runs` resolves a stack's state with `Path::is_file`, and a
    /// `Missing` stack declares nothing — a fabricated path would test the
    /// empty case by accident.
    fn colliding_stacks_app(name: &str) -> (EguiApp, PathBuf) {
        let (mut app, dir) = cleanup_test_app(name);
        app.docker_available = true;
        app.docker = Some(Ok(DockerSnapshot::default()));
        app.compose_plugin = Some(true);
        app.compose_loaded = true;
        let body = "services:\n  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n";
        for project in ["alpha", "beta"] {
            let folder = dir.join(project);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("docker-compose.yml"), body).unwrap();
            let file = folder
                .join("docker-compose.yml")
                .to_string_lossy()
                .into_owned();
            app.config.docker_stacks.push(file.clone());
            app.compose_stacks.push(StackEntry {
                file,
                project: project.to_string(),
                services: vec![compose_view::StackService {
                    name: "web".to_string(),
                    ports: vec![crate::ui::ports::PortBinding {
                        host_ip: "0.0.0.0".to_string(),
                        host_port: 8080,
                        container_port: 80,
                        protocol: "tcp".to_string(),
                    }],
                    host_network: false,
                }],
                runs: Vec::new(),
                state: StackState::Stopped,
                error: None,
            });
        }
        (app, dir)
    }

    #[test]
    fn planning_a_reassignment_never_opens_a_dialog_and_moves_one_of_the_two_stacks() {
        let (app, dir) = colliding_stacks_app("docker-plan-ports");
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        // The tab label counts the allocation rows, and this app declares
        // ports without running a container, so the count is neither 0 nor
        // a number worth pinning here.
        harness.get_by_label_contains("Ports (").click();
        harness.run();
        harness.get_by_label("Proposer une réattribution").click();
        harness.run();

        assert!(
            harness.state().active_dialog.is_none(),
            "computing a proposal writes nothing, so it must not ask"
        );
        assert_eq!(
            harness.state().docker_action_invocations,
            0,
            "the planner is pure arithmetic — nothing is deferred for it"
        );
        let plan = harness
            .state()
            .docker_port_plan
            .as_ref()
            .expect("a plan was computed");
        assert_eq!(plan.moves.len(), 1, "one of the two stacks keeps 8080");
        assert_eq!(plan.moves[0].from, 8080);
        assert_eq!(plan.moves[0].to, 8081);
        assert!(
            plan.moves[0].file.contains("beta"),
            "the tie-break is the file path, so alpha keeps the port: {}",
            plan.moves[0].file
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The one action of the Docker tab that writes to a file the user owns.
    /// It must ask first — and, with the seam closed, still write nothing.
    #[test]
    fn applying_a_reassignment_asks_before_writing_anything() {
        let (app, dir) = colliding_stacks_app("docker-apply-ports");
        let before = std::fs::read_to_string(dir.join("beta").join("docker-compose.yml")).unwrap();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        // The tab label counts the allocation rows, and this app declares
        // ports without running a container, so the count is neither 0 nor
        // a number worth pinning here.
        harness.get_by_label_contains("Ports (").click();
        harness.run();
        harness.get_by_label("Proposer une réattribution").click();
        harness.run();
        harness.get_by_label("Appliquer la réattribution").click();
        harness.run_steps(1);

        assert!(matches!(
            harness.state().active_dialog,
            Some(ActiveDialog {
                on_confirm: Some(PendingAction::ApplyPortReassignment(_)),
                ..
            })
        ));
        assert_eq!(harness.state().docker_action_invocations, 0);

        harness.run();
        harness.get_by_label("Oui").click();
        harness.run_steps(2);
        assert!(harness.state().active_dialog.is_none());
        assert_eq!(harness.state().docker_action_invocations, 1);
        // `docker_actions_enabled` is false in a test app, so the write half
        // never runs: the file on disk is the one the test wrote.
        assert_eq!(
            std::fs::read_to_string(dir.join("beta").join("docker-compose.yml")).unwrap(),
            before
        );
        assert!(harness.state().docker_port_edits.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn compute_volume_sizes_never_opens_a_dialog_and_executes_exactly_once() {
        let (mut app, dir) = cleanup_test_app("docker-compute-volume-sizes");
        app.docker_available = true;
        app.docker = Some(Ok(sample_docker_snapshot()));
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 850.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);

        harness.run();
        harness.get_by_label("Docker").click();
        harness.run();
        // The button lives on the Volumes tab; this click also covers the
        // `SelectList` dispatch — the tab strip labels every list with its
        // row count, and this snapshot has no volume.
        harness.get_by_label("Volumes (0)").click();
        harness.run();
        assert_eq!(harness.state().docker_active_list, DockerList::Volumes);
        harness.get_by_label("Calculer les tailles").click();
        // Unlike the destructive-action flow (which defers only once the
        // user confirms a dialog, itself a later click), the click here
        // directly stashes `deferred_docker_action` on its own frame — so a
        // single `run_steps(1)` is enough to process that frame and still
        // observe zero executions; the *following* frame is the one that
        // actually runs it (see `execute_deferred_docker_action`'s doc
        // comment: it always runs at the very start of the next frame).
        harness.run_steps(1);
        assert!(
            harness.state().active_dialog.is_none(),
            "ComputeVolumeSizes must never open a confirm dialog — it isn't destructive"
        );
        assert_eq!(harness.state().docker_action_invocations, 0);
        harness.step();
        assert_eq!(harness.state().docker_action_invocations, 1);
        assert!(harness.state().active_dialog.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn models_tab_is_permanent_and_empty_state_never_spawns_in_tests() {
        let (app, dir) = cleanup_test_app("models-tab-empty");
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 760.0))
            .build_ui_state(|ui, app: &mut EguiApp| app.ui_content(ui), app);
        harness.run();
        harness.get_by_label("Modèles").click();
        harness.run();
        assert!(harness.query_by_label("DevToolBox — Modèles").is_some());
        assert!(harness
            .query_by_label_contains("Aucun inventaire chargé")
            .is_some());
        assert!(harness.state().models_job.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn model_download_requires_and_preserves_the_review_digest() {
        let (mut app, dir) = cleanup_test_app("models-reviewed-download");
        app.models_offers.push(AcquisitionOffer {
            provider: "direct".into(),
            locator: "https://example.test/model.gguf".into(),
            family: "llm".into(),
            filename: "model.gguf".into(),
            format: "gguf".into(),
            executable: true,
            review_digest: Some("a".repeat(64)),
            ..Default::default()
        });
        app.models_ui.selected_offer = Some(0);
        app.handle_models_action(ModelsAction::RunReviewed);
        assert_eq!(app.models_job, Some(ModelsJob::Download));
        assert_eq!(
            app.models_ui.section,
            models_view::ModelsSection::Operations
        );

        app.models_job = None;
        app.models_offers[0].review_digest = None;
        app.handle_models_action(ModelsAction::RunReviewed);
        assert!(app
            .models_error
            .as_deref()
            .is_some_and(|error| error.contains("digest")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn changing_model_library_is_blocked_by_an_explicit_dialog() {
        let (mut app, dir) = cleanup_test_app("models-library-dialog");
        app.models_ui.library_root = "/data/models".into();
        app.handle_models_action(ModelsAction::SaveSettings);
        assert!(matches!(
            app.active_dialog,
            Some(ActiveDialog {
                on_confirm: Some(PendingAction::SaveModelSettings { .. }),
                ..
            })
        ));
        assert!(app.models_job.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
