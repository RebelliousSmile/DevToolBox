//! Application state: settings + command list loaded from `crate::storage`,
//! and the Win32 child-control host that renders them as a native button grid.
//!
//! ## Render modes
//!
//! When `Settings.show_categories == false` (default flat path — issue #1/#5):
//!   Only favorite commands are shown in a flat grid (flat path, unchanged).
//!
//! When `Settings.show_categories == true` (grouped path — issue #6):
//!   ALL commands are grouped by category. Each category group is preceded by
//!   a STATIC section-header control (category name). Orphan commands whose
//!   `category` id does not match any declared category appear under a
//!   synthetic "Sans catégorie" header (never persisted — Decision D4).
//!
//! ## Click handling (issue #7 — Decision D1/D2)
//!
//! A `SetWindowSubclass` is installed on the parent HWND in `UiHost::new`.
//! The subclass proc handles `WM_COMMAND / BN_CLICKED` only and chains every
//! other message via `DefSubclassProc` so tao's own proc is never bypassed.
//! On `BN_CLICKED`, the control id (`LOWORD(wParam)`) is resolved via the
//! `id_to_command` map and `process::launch` is called best-effort (fire and
//! forget; errors logged).  `RemoveWindowSubclass` is called in `Drop for UiHost`
//! to restore the original subclass chain.
//!
//! ## AC3 — icon_size drives cell sizing (issue #7 — Decision D6)
//!
//! Both flat and grouped layout paths derive button dimensions from
//! `config.default_settings.icon_size` via `xaml_gen::cell_size`, replacing
//! the old hardcoded `CELL_H = 80` in the grouped path.
//!
//! ## Deferred CRUD UI seam (issue #9)
//!
//! `crate::storage::{add_category, rename_category, remove_category}` form the
//! callable API seam for the future settings/alias-editor UI (issue #9).
//! No interactive widgets are built here; the logic + persistence are complete.
//!
//! ## Favorite-toggle seam (issue #7 / issue #9)
//!
//! `crate::storage::toggle_favorite` + `UiHost::reload` form the callable
//! favorite-toggle API + grid-refresh seam.  The interactive toggle widget is
//! DEFERRED to issue #9.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, GetStockObject, InvalidateRect, COLOR_WINDOW, DEFAULT_GUI_FONT,
    FW_NORMAL, FW_REGULAR, FW_SEMIBOLD, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ,
};
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DrawMenuBar,
    GetCursorPos, GetDlgCtrlID, GetParent, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, LoadCursorW,
    MessageBoxW, PostMessageW, RegisterClassW, SendMessageW, SetMenu, SetTimer, SetWindowPos,
    ShowWindow, BN_CLICKED, BS_OWNERDRAW, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, ES_AUTOHSCROLL,
    GWLP_HINSTANCE, HCURSOR, HICON, HMENU, HWND_TOP, IDC_ARROW, IDYES, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, LB_ADDSTRING,
    LB_DELETESTRING, LB_GETCOUNT, LB_GETCURSEL, LB_RESETCONTENT, LB_SETTOPINDEX,
    MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO, MF_POPUP, MF_SEPARATOR, MF_STRING,
    SWP_NOZORDER, SW_HIDE, SW_SHOW, TPM_LEFTALIGN, TPM_TOPALIGN, TrackPopupMenuEx, WINDOW_EX_STYLE, WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC,
    WM_DRAWITEM, WM_MOUSEMOVE, WM_SETFONT, WM_TIMER, WNDCLASSW, WS_BORDER, WS_CHILD, WS_HSCROLL,
    WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

/// `WM_MOUSELEAVE` (0x02A3) — sent by TrackMouseEvent when the cursor leaves
/// the tracked window.  Not re-exported by the `windows` crate's 0.52
/// `Win32_UI_WindowsAndMessaging` feature; defined here as a constant.
const WM_MOUSELEAVE: u32 = 0x02A3;

use crate::icons;
use crate::ui::card::{CardState, IconSource};
use crate::ui::xaml_gen::{
    assign_control_ids, build_grid, build_sectioned, cell_size, GridEntry, GridModel, GridSection,
    SectionRow, SectionedModel,
};

// ---------------------------------------------------------------------------
// Subclass identifier
// ---------------------------------------------------------------------------

/// Unique subclass id for DevToolBox's WM_COMMAND handler.
///
/// Must be unique per (HWND, proc) pair.  We use 1 as our private id since
/// tao does not install its own subclass under this id.
const SUBCLASS_ID: usize = 1;

/// Unique subclass id for the per-button hover tracking subclass (Phase 3).
///
/// Distinct from `SUBCLASS_ID` (1) so both subclasses can coexist on different
/// HWNDs without id collisions.  Each button HWND gets this id installed once
/// by `create_button` and removed in `reload`/`Drop`.
const HOVER_SUBCLASS_ID: usize = 2;
const NAV_ACTIONS_ID: u16 = 10;
const NAV_TERMINAL_ID: u16 = 11;
const NAV_AUTOMATIONS_ID: u16 = 12;
const AUTOMATION_STOP_ID: u16 = 20;
const AUTOMATION_TOGGLE_ID: u16 = 21;
const AUTOMATION_APPLY_ID: u16 = 22;
const CONTENT_TOP: u32 = 52;
const ACTION_COLS: u32 = 4;
const VARIANT_MENU_ID_BASE: u16 = 30_000;

// Menu-bar command ids (range 100-199; distinct from nav ids 10-12 and button
// control ids 1000+).
const IDM_FILE_RELOAD: u16 = 100;
const IDM_FILE_QUIT: u16 = 101;
const IDM_VIEW_ACTIONS: u16 = 102;
const IDM_VIEW_TERMINAL: u16 = 103;
const IDM_VIEW_AUTOMATIONS: u16 = 104;
const IDM_HELP_ABOUT: u16 = 105;

/// Window-class name for the per-view container windows.
const VIEW_CLASS: PCWSTR = w!("WinFXViewContainer");
const TERMINAL_PLACEHOLDER: &str = "Aucune action lancée.";
const MAX_TERMINAL_LINES: usize = 5_000;
const TRIMMED_TERMINAL_LINES: usize = 3_500;

/// Registers [`VIEW_CLASS`] exactly once per process (it is a global resource).
static REGISTER_VIEW_CLASS: std::sync::Once = std::sync::Once::new();

#[derive(Clone, Copy, PartialEq)]
enum DashboardView {
    Actions,
    Terminal,
    Automations,
}

// ---------------------------------------------------------------------------
// Layout mode tag
// ---------------------------------------------------------------------------

/// Which layout is active for this UiHost instance.
enum LayoutMode {
    /// Flat favorites-only grid (show_categories == false).
    Flat { grid: GridModel },
    /// Grouped all-commands layout (show_categories == true).
    Grouped { model: SectionedModel },
}

#[derive(Clone, Debug)]
struct ActionOption {
    label: String,
    command: String,
}

#[derive(Clone, Debug)]
enum ActionBinding {
    Direct(String),
    Variants { title: String, options: Vec<ActionOption> },
}

#[derive(Clone, Debug)]
struct ActionButtonSpec {
    action_key: String,
    label: String,
    icon: String,
    binding: ActionBinding,
}

// ---------------------------------------------------------------------------
// Host state
// ---------------------------------------------------------------------------

/// Owns the parent HWND, the spawned child-button handles, and every HBITMAP
/// created for icon display.
///
/// # GDI handle ownership (AC3)
///
/// Every HBITMAP created by the icon pipeline is pushed into `bitmaps`.
/// `clear_bitmaps` calls `DeleteObject` on each handle and empties the vec;
/// it MUST be called before any button rebuild or reload so handles are never
/// leaked across iterations.  `Drop for UiHost` frees any remaining handles
/// on exit.  `layout_children` only repositions windows and NEVER creates
/// bitmaps, so repeated resizes are leak-free.
///
/// # Header controls (grouped mode)
///
/// STATIC controls created for section headers are tracked in `headers`.
/// They are children of `parent` and are destroyed with the parent window.
/// They follow the same lifetime policy as `buttons` — no explicit
/// `DestroyWindow` call needed at drop (children are destroyed with parent).
///
/// # Control-id map (issue #7)
///
/// `id_to_command` maps each button's `u16` control id to the command string
/// (`Command::command`) for that button.  It is rebuilt from scratch on every
/// `UiHost::new` / `reload` (Decision D3).
pub struct UiHost {
    pub parent: HWND,
    /// Button HWNDs (one per command in display order).
    pub buttons: Vec<HWND>,
    /// Header STATIC HWNDs (one per non-empty section, grouped mode only).
    pub headers: Vec<HWND>,
    /// GDI bitmap handles owned by this host.  Freed in `clear_bitmaps` /
    /// `Drop`.  Never modified by `layout_children`.
    pub bitmaps: Vec<HBITMAP>,
    /// Active layout mode.
    mode: LayoutMode,
    /// Maps each button's u16 control id → the command string for that button.
    ///
    /// Rebuilt atomically on every `new`/`reload`.  The subclass proc reads
    /// this (via `ui::handle_command`) to launch the clicked command.
    id_to_action: HashMap<u16, ActionBinding>,
    /// Maps each button's u16 control id → its resolved icon source.
    ///
    /// This is the SOLE owner of all HBITMAP handles produced by the icon
    /// pipeline.  `Drop` iterates this map and calls `DeleteObject` for each
    /// `IconSource::Bitmap(h)` variant.  Neither `bitmaps` nor any other field
    /// holds the same handle — no double-free.
    pub id_to_icon: HashMap<u16, IconSource>,
    /// Cached Segoe UI font for card labels.  Created in `UiHost::new`,
    /// freed in `Drop`.  Never recreated on reload.
    pub label_font: HFONT,
    /// Cached Segoe UI Emoji font for the icon region.  Created in `UiHost::new`,
    /// freed in `Drop`.  Never recreated on reload.
    pub emoji_font: HFONT,
    /// Cached Segoe UI SemiBold font for category headers (Phase 4).
    ///
    /// Applied to STATIC header controls via `WM_SETFONT` after creation.
    /// Created in `UiHost::new`, freed in `Drop`.  Never recreated on reload.
    pub header_font: HFONT,
    /// Cached background brush for the container window (used by WM_CTLCOLORSTATIC
    /// to give headers a transparent-to-container background).
    pub header_bg_brush: HBRUSH,
    /// Cached fill brush for the normal card state.
    pub fill_brush_normal: HBRUSH,
    /// Cached fill brush for the pressed card state.
    pub fill_brush_pressed: HBRUSH,
    /// Cached fill brush for the hover card state (Phase 3).
    pub fill_brush_hover: HBRUSH,
    /// Cached border brush for the normal card state.
    pub border_brush_normal: HBRUSH,
    /// Cached border brush for the focused/accent card state.
    pub border_brush_focus: HBRUSH,
    /// Per-button hover state keyed by control id.
    ///
    /// Set to `true` by the per-button hover subclass on `WM_MOUSEMOVE`;
    /// cleared on `WM_MOUSELEAVE`.  Absent key is treated as `false`.
    /// Rebuilt from scratch on every `new`/`reload`.
    pub id_to_hover: HashMap<u16, bool>,
    /// Cell height (pixels) for the grouped layout, derived from icon_size at
    /// build time via `cell_size(icon_size).1`.  Stored so `layout_children`
    /// can use it without holding a reference to the config.
    cell_h_grouped: u32,
    nav_buttons: Vec<HWND>,
    /// Opaque container window for the Actions view (hosts buttons + headers).
    actions_container: HWND,
    /// Opaque container window for the Terminal view (hosts the terminal EDIT).
    terminal_container: HWND,
    /// Opaque container window for the Automations view (hosts that EDIT).
    automations_container: HWND,
    terminal: HWND,
    automations: HWND,
    automation_controls: Vec<HWND>,
    schedule_time: HWND,
    schedule_interval: HWND,
    scheduled_tasks: Vec<crate::windows::process::ScheduledTask>,
    active_view: DashboardView,
    events: Option<(
        Sender<crate::windows::process::ActionEvent>,
        Receiver<crate::windows::process::ActionEvent>,
    )>,
    terminal_lines: VecDeque<String>,
    terminal_partial_line: String,
    terminal_placeholder_visible: bool,
    variant_menu_commands: HashMap<u16, String>,
}

impl UiHost {
    /// Load config via `crate::storage::load`, create Win32 child controls
    /// parented to `parent_hwnd`, and install the WM_COMMAND subclass.
    ///
    /// Branches on `config.default_settings.show_categories`:
    /// - `false`: flat favorites-only grid (issue #1/#5 behavior unchanged).
    /// - `true`: grouped all-commands layout with STATIC section headers.
    pub fn new(parent_hwnd: HWND) -> Result<Self, Box<dyn std::error::Error>> {
        let config = crate::storage::load().unwrap_or_else(|err| {
            log::warn!("storage::load failed ({err}); falling back to built-in defaults");
            crate::storage::Config {
                version: "0.1.0".to_string(),
                default_settings: crate::storage::Settings {
                    show_categories: true,
                    icon_size: 56,
                    theme: "light".to_string(),
                    launch_at_startup: false,
                    show_descriptions: true,
                },
                categories: Vec::new(),
                commands: vec![
                    crate::storage::Command {
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
                    },
                    crate::storage::Command {
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
                    },
                    crate::storage::Command {
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
                    },
                ],
            }
        });

        // Register the container class and create one opaque container per view.
        // The HINSTANCE is taken from the parent window (no LibraryLoader needed).
        let instance = HINSTANCE(unsafe { GetWindowLongPtrW(parent_hwnd, GWLP_HINSTANCE) });
        ensure_view_class(instance);
        let actions_container = create_container(parent_hwnd, instance, true);
        let terminal_container = create_container(parent_hwnd, instance, false);
        let automations_container = create_container(parent_hwnd, instance, false);

        // Buttons/headers are parented to the Actions container, not the window.
        let mut host = Self::build_from_config(parent_hwnd, actions_container, &config);
        host.actions_container = actions_container;
        host.terminal_container = terminal_container;
        host.automations_container = automations_container;
        host.events = Some(mpsc::channel());

        // Create cached GDI resources (fonts + brushes).
        // These are created ONCE here and freed in Drop — never recreated on reload.
        host.label_font = create_font_segoe_ui(12, false);
        host.emoji_font = create_font_segoe_ui_emoji(22);
        // Phase 4: SemiBold header font (10pt) for category section labels.
        host.header_font = create_font_segoe_ui(10, true);
        // Phase 4: background brush for WM_CTLCOLORSTATIC (matches container bg).
        // We use white (0xFFFFFF) so headers blend with the container surface.
        host.header_bg_brush =
            crate::ui::card::make_brush(windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
        // Apply the header font to all existing STATIC header controls via WM_SETFONT.
        host.apply_header_font();
        host.fill_brush_normal = crate::ui::card::make_brush(crate::ui::card::COLOR_FILL_NORMAL);
        host.fill_brush_pressed = crate::ui::card::make_brush(crate::ui::card::COLOR_FILL_PRESSED);
        host.fill_brush_hover = crate::ui::card::make_brush(crate::ui::card::COLOR_FILL_HOVER);
        host.border_brush_normal =
            crate::ui::card::make_brush(crate::ui::card::COLOR_BORDER_NORMAL);
        host.border_brush_focus = crate::ui::card::make_brush(crate::ui::card::COLOR_BORDER_FOCUS);

        host.create_dashboard_controls();

        // Install the WM_COMMAND subclass (Decision D1/D2).
        // dwRefData = 0: host state is accessed via the thread_local HOST, not
        // via a raw pointer through the FFI boundary.
        let subclass_ok =
            unsafe { SetWindowSubclass(parent_hwnd, Some(subclass_proc), SUBCLASS_ID, 0) };
        if !subclass_ok.as_bool() {
            log::warn!("SetWindowSubclass failed; button clicks will not be dispatched");
        } else {
            log::info!("WM_COMMAND subclass installed on parent HWND");
        }

        // Install the native menu bar. SetMenu shrinks the client area, which
        // makes tao emit a Resized event → on_resize → layout_children, so the
        // content re-flows under the menu automatically.
        install_menu_bar(parent_hwnd);

        host.layout_children(800, 600);
        unsafe { SetTimer(parent_hwnd, 1, 100, None) };
        Ok(host)
    }

    /// Build a `UiHost` from a pre-loaded `Config` WITHOUT installing the
    /// subclass.
    ///
    /// Used by both `new` (which installs the subclass once) and `reload`
    /// (which keeps the existing subclass and only replaces child controls).
    fn build_from_config(
        parent_hwnd: HWND,
        content_parent: HWND,
        config: &crate::storage::Config,
    ) -> Self {
        let icon_size = config.default_settings.icon_size;
        let icon_dirs = icons::icons_dirs();
        // Derive the cell height once; used by layout_grouped.
        let (_cell_w, cell_h) = cell_size(icon_size);

        if config.default_settings.show_categories {
            // ----------------------------------------------------------------
            // GROUPED PATH (show_categories == true)
            // ----------------------------------------------------------------
            let groups = crate::storage::group_commands_by_category(config);

            let action_sections: Vec<(String, Vec<ActionButtonSpec>)> = groups
                .iter()
                .map(|g| {
                    let header = match g.category {
                        Some(cat) => cat.name.clone(),
                        None => "Sans catégorie".to_string(),
                    };
                    let actions = build_action_buttons(g.commands.iter().copied());
                    (header, actions)
                })
                .collect();

            let sections: Vec<GridSection> = action_sections
                .iter()
                .map(|(header, actions)| GridSection {
                    header: header.clone(),
                    entries: actions
                        .iter()
                        .map(|action| GridEntry {
                            label: action.label.clone(),
                            icon: action.icon.clone(),
                            command_id: Some(action.action_key.clone()),
                        })
                        .collect(),
                })
                .collect();

            let sectioned = build_sectioned(&sections, ACTION_COLS);

            // Collect all cells in row order to assign control ids.
            let all_cells: Vec<crate::ui::xaml_gen::GridCell> = sectioned
                .rows
                .iter()
                .flat_map(|row| match row {
                    SectionRow::Cells { cells, .. } => cells.clone(),
                    _ => Vec::new(),
                })
                .collect();

            let action_specs: HashMap<String, ActionBinding> = action_sections
                .iter()
                .flat_map(|(_, actions)| actions.iter())
                .map(|action| (action.action_key.clone(), action.binding.clone()))
                .collect();

            let id_pairs = assign_control_ids(&all_cells);
            let id_to_action: HashMap<u16, ActionBinding> = id_pairs
                .iter()
                .filter_map(|(ctrl_id, action_key)| {
                    action_specs
                        .get(action_key.as_str())
                        .cloned()
                        .map(|binding| (*ctrl_id, binding))
                })
                .collect();

            // Create STATIC header controls and BUTTON controls.
            let mut buttons: Vec<HWND> = Vec::new();
            let mut headers: Vec<HWND> = Vec::new();
            // id_to_icon is the sole owner of all HBITMAP handles.
            // bitmaps vec is kept empty — action buttons own icons through id_to_icon.
            let mut id_to_icon: HashMap<u16, IconSource> = HashMap::new();

            // Iterate id_pairs in step with cells-that-have-command-ids.
            let mut id_pair_idx = 0usize;

            for row in &sectioned.rows {
                match row {
                    SectionRow::Header { label, .. } => {
                        let label_w: Vec<u16> =
                            label.encode_utf16().chain(std::iter::once(0)).collect();
                        let class_w: Vec<u16> = "STATIC\0".encode_utf16().collect();

                        let hwnd_hdr = unsafe {
                            CreateWindowExW(
                                WINDOW_EX_STYLE(0),
                                PCWSTR(class_w.as_ptr()),
                                PCWSTR(label_w.as_ptr()),
                                WS_CHILD | WS_VISIBLE,
                                0,
                                0,
                                0,
                                0,
                                content_parent,
                                None,
                                None,
                                None,
                            )
                        };

                        if hwnd_hdr.0 == 0 {
                            log::warn!("CreateWindowExW returned null for header '{label}'");
                        } else {
                            unsafe { ShowWindow(hwnd_hdr, SW_SHOW) };
                            headers.push(hwnd_hdr);
                            log::info!("Created header: '{label}'");
                        }
                    }
                    SectionRow::Cells { cells, .. } => {
                        for cell in cells {
                            let ctrl_id: Option<u16> = if cell.command_id.is_some() {
                                let id = id_pairs.get(id_pair_idx).map(|(id, _)| *id);
                                id_pair_idx += 1;
                                id
                            } else {
                                None
                            };
                            if let Some(btn) = create_button(
                                content_parent,
                                cell,
                                icon_size,
                                &icon_dirs,
                                &mut id_to_icon,
                                ctrl_id,
                            ) {
                                buttons.push(btn);
                            }
                        }
                    }
                }
            }

            log::info!(
                "Grouped mode: {} sections, {} headers, {} buttons",
                sections.len(),
                headers.len(),
                buttons.len()
            );

            UiHost {
                parent: parent_hwnd,
                buttons,
                headers,
                bitmaps: Vec::new(),
                id_to_icon,
                id_to_action,
                id_to_hover: HashMap::new(),
                // Fonts and brushes are created in UiHost::new after build_from_config.
                // Initialize to null/invalid handles here; Drop checks before freeing.
                label_font: HFONT(0),
                emoji_font: HFONT(0),
                header_font: HFONT(0),
                header_bg_brush: HBRUSH(0),
                fill_brush_normal: HBRUSH(0),
                fill_brush_pressed: HBRUSH(0),
                fill_brush_hover: HBRUSH(0),
                border_brush_normal: HBRUSH(0),
                border_brush_focus: HBRUSH(0),
                mode: LayoutMode::Grouped { model: sectioned },
                cell_h_grouped: cell_h,
                nav_buttons: Vec::new(),
                actions_container: HWND(0),
                terminal_container: HWND(0),
                automations_container: HWND(0),
                terminal: HWND(0),
                automations: HWND(0),
                automation_controls: Vec::new(),
                schedule_time: HWND(0),
                schedule_interval: HWND(0),
                scheduled_tasks: Vec::new(),
                active_view: DashboardView::Actions,
                events: None,
                terminal_lines: VecDeque::new(),
                terminal_partial_line: String::new(),
                terminal_placeholder_visible: true,
                variant_menu_commands: HashMap::new(),
            }
        } else {
            // ----------------------------------------------------------------
            // FLAT PATH (show_categories == false)
            // ----------------------------------------------------------------
            let action_buttons =
                build_action_buttons(config.commands.iter().filter(|command| command.is_favorite));
            let entries: Vec<GridEntry> = action_buttons
                .iter()
                .map(|action| GridEntry {
                    label: action.label.clone(),
                    icon: action.icon.clone(),
                    command_id: Some(action.action_key.clone()),
                })
                .collect();

            log::info!("Loaded {} favorite actions (flat mode)", entries.len());

            let grid = build_grid(&entries, ACTION_COLS);

            let action_specs: HashMap<String, ActionBinding> = action_buttons
                .iter()
                .map(|action| (action.action_key.clone(), action.binding.clone()))
                .collect();
            let id_pairs = assign_control_ids(&grid.cells);
            let id_to_action: HashMap<u16, ActionBinding> = id_pairs
                .iter()
                .filter_map(|(ctrl_id, action_key)| {
                    action_specs
                        .get(action_key.as_str())
                        .cloned()
                        .map(|binding| (*ctrl_id, binding))
                })
                .collect();

            let mut buttons: Vec<HWND> = Vec::with_capacity(grid.cells.len());
            // id_to_icon is the sole owner of all HBITMAP handles.
            // bitmaps vec is kept empty — action buttons own icons through id_to_icon.
            let mut id_to_icon: HashMap<u16, IconSource> = HashMap::new();
            let mut id_pair_idx = 0usize;

            for cell in &grid.cells {
                let ctrl_id: Option<u16> = if cell.command_id.is_some() {
                    let id = id_pairs.get(id_pair_idx).map(|(id, _)| *id);
                    id_pair_idx += 1;
                    id
                } else {
                    None
                };
                if let Some(btn) = create_button(
                    content_parent,
                    cell,
                    icon_size,
                    &icon_dirs,
                    &mut id_to_icon,
                    ctrl_id,
                ) {
                    buttons.push(btn);
                }
            }

            UiHost {
                parent: parent_hwnd,
                buttons,
                headers: Vec::new(),
                bitmaps: Vec::new(),
                id_to_icon,
                id_to_action,
                id_to_hover: HashMap::new(),
                // Fonts and brushes are created in UiHost::new after build_from_config.
                // Initialize to null/invalid handles here; Drop checks before freeing.
                label_font: HFONT(0),
                emoji_font: HFONT(0),
                header_font: HFONT(0),
                header_bg_brush: HBRUSH(0),
                fill_brush_normal: HBRUSH(0),
                fill_brush_pressed: HBRUSH(0),
                fill_brush_hover: HBRUSH(0),
                border_brush_normal: HBRUSH(0),
                border_brush_focus: HBRUSH(0),
                mode: LayoutMode::Flat { grid },
                cell_h_grouped: cell_h,
                nav_buttons: Vec::new(),
                actions_container: HWND(0),
                terminal_container: HWND(0),
                automations_container: HWND(0),
                terminal: HWND(0),
                automations: HWND(0),
                automation_controls: Vec::new(),
                schedule_time: HWND(0),
                schedule_interval: HWND(0),
                scheduled_tasks: Vec::new(),
                active_view: DashboardView::Actions,
                events: None,
                terminal_lines: VecDeque::new(),
                terminal_partial_line: String::new(),
                terminal_placeholder_visible: true,
                variant_menu_commands: HashMap::new(),
            }
        }
    }

    /// Delete and release all tracked GDI bitmap handles from the legacy vec.
    ///
    /// Must be called before any button rebuild / reload to avoid leaking
    /// GDI handles (AC3).  The legacy `bitmaps` vec is kept for API
    /// compatibility but should remain empty for the owner-draw path.
    pub fn clear_bitmaps(&mut self) {
        for hbitmap in self.bitmaps.drain(..) {
            icons::gdi::delete_bitmap(hbitmap);
        }
        log::debug!("clear_bitmaps: all HBITMAP handles freed");
    }

    /// Delete and release all icon handles owned by `id_to_icon`.
    ///
    /// Must be called before any reload so HBITMAP handles are never leaked
    /// across rebuild iterations (AC3).
    pub fn clear_id_to_icon(&mut self) {
        for (_id, source) in self.id_to_icon.drain() {
            if let IconSource::Bitmap(hbitmap) = source {
                // Safety: hbitmap was created by rgba_to_hbitmap and stored
                // in id_to_icon as the sole owner; it is not currently
                // selected into any DC (buttons are not mid-paint at reload).
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(hbitmap.0));
                }
            }
        }
        log::debug!("clear_id_to_icon: all owner-draw HBITMAP handles freed");
    }

    /// Apply the cached `header_font` to all STATIC header controls via `WM_SETFONT`.
    ///
    /// Called from `UiHost::new` (and again after `reload`) once `header_font`
    /// has been created.  The `TRUE` redraw parameter tells each STATIC to
    /// repaint immediately with the new font.
    ///
    /// This is the Phase 4 "WM_SETFONT" half of approach (b); the
    /// `WM_CTLCOLORSTATIC` half lives in `container_proc`.
    pub fn apply_header_font(&self) {
        if self.header_font.0 == 0 {
            return;
        }
        for &hwnd in &self.headers {
            if hwnd.0 != 0 {
                unsafe {
                    SendMessageW(
                        hwnd,
                        WM_SETFONT,
                        WPARAM(self.header_font.0 as usize),
                        LPARAM(1), // TRUE = redraw
                    );
                }
            }
        }
        log::debug!(
            "apply_header_font: font applied to {} header(s)",
            self.headers.len()
        );
    }

    /// Phase 2 Fluent card owner-draw paint.
    ///
    /// Called from `container_proc` on `WM_DRAWITEM` for action buttons.
    /// Reads `ODS_SELECTED` / `ODS_FOCUS` from `DRAWITEMSTRUCT.itemState`,
    /// looks up the resolved `IconSource` from `id_to_icon`, and delegates
    /// to `card::paint_card` for all GDI work via a memory DC.
    ///
    /// `is_hot` is read from `id_to_hover` (Phase 3 hover tracking).
    ///
    /// # Safety
    /// `dis` must point to a valid `DRAWITEMSTRUCT` provided by Windows.
    pub unsafe fn draw_item(&self, dis: *const DRAWITEMSTRUCT) {
        use std::sync::atomic::{AtomicBool, Ordering};

        // Safety: Windows guarantees dis is valid for the duration of WM_DRAWITEM.
        let dis = &*dis;
        let hdc = dis.hDC;
        let rect = dis.rcItem;
        let ctrl_id = dis.CtlID as u16;
        let item_state = dis.itemState;

        // Log on the very first WM_DRAWITEM handled (proves routing is correct).
        static FIRST_PAINT_LOGGED: AtomicBool = AtomicBool::new(false);
        if !FIRST_PAINT_LOGGED.swap(true, Ordering::Relaxed) {
            log::info!(
                "WM_DRAWITEM Phase2 first paint: container_proc → draw_item (ctrl_id={ctrl_id})"
            );
        }

        // Build CardState from ODS_* flags and the Phase 3 hover map.
        // itemState is ODS_FLAGS (newtype wrapper over u32) — use .0 for bitmasking.
        let state = CardState {
            is_hot: self.id_to_hover.get(&ctrl_id).copied().unwrap_or(false),
            is_pressed: (item_state.0 & crate::ui::card::ODS_SELECTED) != 0,
            is_focused: (item_state.0 & crate::ui::card::ODS_FOCUS) != 0,
        };

        // Select fill and border brushes based on state.
        let fill_brush = if state.is_pressed {
            self.fill_brush_pressed
        } else if state.is_hot {
            self.fill_brush_hover
        } else {
            self.fill_brush_normal
        };
        let border_brush = if state.is_focused || state.is_pressed {
            self.border_brush_focus
        } else {
            self.border_brush_normal
        };

        // Retrieve the button label from the window text.
        let mut label_buf = [0u16; 256];
        let len =
            windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(dis.hwndItem, &mut label_buf);
        let label = if len > 0 {
            String::from_utf16_lossy(&label_buf[..len as usize])
        } else {
            String::new()
        };

        // Look up icon source (borrow — no HBITMAP is transferred).
        let no_icon = IconSource::NoIcon;
        let icon = self.id_to_icon.get(&ctrl_id).unwrap_or(&no_icon);

        // Guard: skip paint if fonts/brushes not yet initialized (can happen
        // if WM_DRAWITEM fires before UiHost::new finishes initialization).
        if self.label_font.0 == 0 || self.emoji_font.0 == 0 {
            return;
        }

        crate::ui::card::paint_card(
            hdc,
            rect,
            state,
            self.label_font,
            self.emoji_font,
            fill_brush,
            border_brush,
            icon,
            &label,
        );
    }

    /// Re-position child controls to fill the parent client area.
    ///
    /// This method only calls `SetWindowPos` — it NEVER creates bitmaps.
    /// This is the guarantee that repeated resizes cannot leak GDI handles.
    ///
    /// In flat mode: positions buttons using dimensions from `cell_size(icon_size)`
    /// (AC3 — Decision D6).
    /// In grouped mode: stacks header rows and button rows vertically, also
    /// using `cell_size`-derived height (replaces hardcoded `CELL_H = 80`).
    pub fn layout_children(&mut self, width: u32, height: u32) {
        // Position the nav bar and the three containers first.
        self.layout_dashboard(width, height);

        // Child controls are laid out RELATIVE to their container, whose origin
        // is already at CONTENT_TOP — so the content area starts at (0, 0).
        let content_w = width;
        let content_h = height.saturating_sub(CONTENT_TOP).max(1);

        match &self.mode {
            LayoutMode::Flat { grid } => {
                layout_flat(
                    grid,
                    &self.buttons,
                    content_w,
                    content_h,
                    self.cell_h_grouped,
                );
            }
            LayoutMode::Grouped { model } => {
                let rows_snapshot: Vec<(bool, usize)> = model
                    .rows
                    .iter()
                    .map(|r| match r {
                        SectionRow::Header { .. } => (true, 0),
                        SectionRow::Cells { cells, .. } => (false, cells.len()),
                    })
                    .collect();

                layout_grouped(
                    &rows_snapshot,
                    &self.headers,
                    &self.buttons,
                    content_w,
                    content_h,
                    model.cols,
                    self.cell_h_grouped,
                );
            }
        }

        // Terminal fills its container; automations reserve a bottom command bar.
        const INSET: u32 = 8;
        let edit_w = content_w.saturating_sub(INSET * 2).max(1);
        let full_h = content_h.saturating_sub(INSET * 2).max(1);
        let _ = unsafe {
            SetWindowPos(
                self.terminal,
                HWND_TOP,
                INSET as i32,
                INSET as i32,
                edit_w as i32,
                full_h as i32,
                SWP_NOZORDER,
            )
        };
        let list_h = content_h.saturating_sub(118).max(80);
        let _ = unsafe {
            SetWindowPos(
                self.automations,
                HWND_TOP,
                INSET as i32,
                INSET as i32,
                edit_w as i32,
                list_h as i32,
                SWP_NOZORDER,
            )
        };
        let y1 = (list_h + 14) as i32;
        let y2 = y1 + 32;
        let positions = [
            (8, y1, 110, 24),
            (122, y1, 80, 24),
            (214, y1, 150, 24),
            (368, y1, 80, 24),
            (8, y2, 150, 30),
            (166, y2, 160, 30),
            (334, y2, 210, 30),
        ];
        for (hwnd, (x, y, w, h)) in self.automation_controls.iter().zip(positions) {
            let _ = unsafe { SetWindowPos(*hwnd, HWND_TOP, x, y, w, h, SWP_NOZORDER) };
        }

        self.apply_view_visibility();
    }

    fn create_dashboard_controls(&mut self) {
        for (id, label) in [
            (NAV_ACTIONS_ID, "Actions"),
            (NAV_TERMINAL_ID, "Terminal"),
            (NAV_AUTOMATIONS_ID, "Automatisations"),
        ] {
            if let Some(button) = create_text_control(
                self.parent,
                "BUTTON",
                label,
                WS_CHILD
                    | WS_VISIBLE
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_PUSHBUTTON as u32),
                id,
            ) {
                self.nav_buttons.push(button);
            }
        }
        // Terminal and automation controls are permanently visible inside their container; the
        // container's own visibility (toggled on view switch) governs them.
        let terminal_style = WS_CHILD
            | WS_VISIBLE
            | WS_BORDER
            | WS_VSCROLL
            | WS_HSCROLL
            | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(LBS_NOINTEGRALHEIGHT as u32);
        self.terminal = create_text_control(
            self.terminal_container,
            "LISTBOX",
            "",
            terminal_style,
            0,
        )
        .unwrap_or(HWND(0));
        listbox_add(self.terminal, TERMINAL_PLACEHOLDER);
        self.automations = create_text_control(
            self.automations_container,
            "LISTBOX",
            "",
            WS_CHILD
                | WS_VISIBLE
                | WS_BORDER
                | WS_VSCROLL
                | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(LBS_NOTIFY as u32),
            0,
        )
        .unwrap_or(HWND(0));
        for (class, label, id, style) in [
            ("STATIC", "Heure (HH:mm)", 0, WS_CHILD | WS_VISIBLE),
            (
                "EDIT",
                "02:00",
                0,
                WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            ),
            ("STATIC", "Répétition (minutes)", 0, WS_CHILD | WS_VISIBLE),
            (
                "EDIT",
                "60",
                0,
                WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            ),
            (
                "BUTTON",
                "Arrêter maintenant",
                AUTOMATION_STOP_ID,
                WS_CHILD
                    | WS_VISIBLE
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_PUSHBUTTON as u32),
            ),
            (
                "BUTTON",
                "Activer / désactiver",
                AUTOMATION_TOGGLE_ID,
                WS_CHILD
                    | WS_VISIBLE
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_PUSHBUTTON as u32),
            ),
            (
                "BUTTON",
                "Appliquer la planification",
                AUTOMATION_APPLY_ID,
                WS_CHILD
                    | WS_VISIBLE
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_PUSHBUTTON as u32),
            ),
        ] {
            if let Some(control) =
                create_text_control(self.automations_container, class, label, style, id)
            {
                if class == "EDIT" && self.schedule_time.0 == 0 {
                    self.schedule_time = control;
                } else if class == "EDIT" {
                    self.schedule_interval = control;
                }
                self.automation_controls.push(control);
            }
        }
        self.apply_view_visibility();
    }

    pub fn launch_action(&mut self, command: &str) {
        self.switch_view(DashboardView::Terminal);
        self.append_terminal(&format!("\r\n> {command}\r\n"));
        let Some((sender, _)) = &self.events else {
            return;
        };
        if let Err(error) = crate::windows::process::launch_captured(command, sender.clone()) {
            self.append_terminal(&format!("ERREUR: {error}\r\n"));
        }
    }

    pub fn handle_action(&mut self, control_id: u16, source_hwnd: HWND) -> bool {
        let Some(binding) = self.id_to_action.get(&control_id).cloned() else {
            return false;
        };
        match binding {
            ActionBinding::Direct(command) => {
                log::info!("handle_action: direct control_id={control_id} cmd='{command}'");
                self.launch_action(&command);
            }
            ActionBinding::Variants { title, options } => {
                log::info!("handle_action: variant menu control_id={control_id} title='{title}'");
                self.show_action_variant_menu(source_hwnd, &options);
            }
        }
        true
    }

    pub fn handle_action_variant_menu(&mut self, menu_id: u16) -> bool {
        let Some(command) = self.variant_menu_commands.remove(&menu_id) else {
            return false;
        };
        self.variant_menu_commands.clear();
        log::info!("handle_action_variant_menu: menu_id={menu_id} cmd='{command}'");
        self.launch_action(&command);
        true
    }

    fn show_action_variant_menu(&mut self, source_hwnd: HWND, options: &[ActionOption]) {
        self.variant_menu_commands.clear();
        if options.is_empty() {
            return;
        }
        let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
            log::warn!("CreatePopupMenu failed for action variants");
            return;
        };

        for (index, option) in options.iter().enumerate() {
            let menu_id = VARIANT_MENU_ID_BASE.saturating_add(index as u16);
            self.variant_menu_commands
                .insert(menu_id, option.command.clone());
            let label_w: Vec<u16> = option
                .label
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let _ = unsafe { AppendMenuW(menu, MF_STRING, menu_id as usize, PCWSTR(label_w.as_ptr())) };
        }

        let mut point = POINT { x: 0, y: 0 };
        if source_hwnd.0 != 0 {
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(source_hwnd, &mut rect) }.is_ok() {
                point.x = rect.left;
                point.y = rect.bottom;
            }
        }
        if point.x == 0 && point.y == 0 {
            let _ = unsafe { GetCursorPos(&mut point) };
        }

        let _ = unsafe {
            TrackPopupMenuEx(
                menu,
                (TPM_LEFTALIGN | TPM_TOPALIGN).0,
                point.x,
                point.y,
                self.parent,
                None,
            )
        };
        let _ = unsafe { DestroyMenu(menu) };
    }

    fn layout_dashboard(&self, width: u32, height: u32) {
        const PAD: u32 = 8;
        const NAV_H: u32 = 28;
        let nav_count = self.nav_buttons.len().max(1) as u32;
        let nav_w = width.saturating_sub(PAD * (nav_count + 1)) / nav_count;
        for (index, button) in self.nav_buttons.iter().enumerate() {
            let x = (PAD + index as u32 * (nav_w + PAD)) as i32;
            let _ = unsafe {
                SetWindowPos(
                    *button,
                    HWND_TOP,
                    x,
                    PAD as i32,
                    nav_w as i32,
                    NAV_H as i32,
                    SWP_NOZORDER,
                )
            };
        }

        let content_y = CONTENT_TOP as i32;
        let content_h = height.saturating_sub(CONTENT_TOP).max(1) as i32;
        for hwnd in [
            self.actions_container,
            self.terminal_container,
            self.automations_container,
        ] {
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    0,
                    content_y,
                    width as i32,
                    content_h,
                    SWP_NOZORDER,
                )
            };
        }
    }

    fn apply_view_visibility(&self) {
        unsafe {
            ShowWindow(self.actions_container, if self.active_view == DashboardView::Actions { SW_SHOW } else { SW_HIDE });
            ShowWindow(self.terminal_container, if self.active_view == DashboardView::Terminal { SW_SHOW } else { SW_HIDE });
            ShowWindow(self.automations_container, if self.active_view == DashboardView::Automations { SW_SHOW } else { SW_HIDE });
        }
    }

    fn switch_view(&mut self, view: DashboardView) {
        self.active_view = view;
        self.apply_view_visibility();
        if view == DashboardView::Automations {
            self.load_automations();
        }
    }

    pub fn switch_view_control(&mut self, control_id: u16) {
        match control_id {
            NAV_ACTIONS_ID => self.switch_view(DashboardView::Actions),
            NAV_TERMINAL_ID => self.switch_view(DashboardView::Terminal),
            NAV_AUTOMATIONS_ID => self.switch_view(DashboardView::Automations),
            _ => {}
        }
    }
    pub fn handle_automation_control(&mut self, control_id: u16) {
        let Some(index) = selected_listbox_index(self.automations) else {
            show_message(self.parent, "Automatisations", "Sélectionnez une automatisation dans la liste.", true);
            return;
        };
        let Some(task) = self.scheduled_tasks.get(index).cloned() else {
            return;
        };
        let (task_path, task_name) = split_task_name(&task.name);
        match control_id {
            AUTOMATION_STOP_ID => {
                if !confirm_action(self.parent, "Arrêter la tâche", &format!("Arrêter maintenant {} ?", task.name)) {
                    return;
                }
                match run_powershell_command(&format!("Stop-ScheduledTask -TaskPath '{}' -TaskName '{}'", ps_single_quote(&task_path), ps_single_quote(&task_name))) {
                    Ok(_) => {
                        show_message(self.parent, "Automatisations", "Tâche arrêtée.", false);
                        self.load_automations();
                    }
                    Err(error) => show_message(self.parent, "Automatisations", &error, true),
                }
            }
            AUTOMATION_TOGGLE_ID => {
                let enable = task.state.eq_ignore_ascii_case("disabled");
                let verb = if enable { "Enable-ScheduledTask" } else { "Disable-ScheduledTask" };
                let message = if enable { format!("Activer {} ?", task.name) } else { format!("Désactiver {} ?", task.name) };
                if !confirm_action(self.parent, "Automatisations", &message) {
                    return;
                }
                match run_powershell_command(&format!("{verb} -TaskPath '{}' -TaskName '{}'", ps_single_quote(&task_path), ps_single_quote(&task_name))) {
                    Ok(_) => {
                        show_message(self.parent, "Automatisations", "Modification appliquée.", false);
                        self.load_automations();
                    }
                    Err(error) => show_message(self.parent, "Automatisations", &error, true),
                }
            }
            AUTOMATION_APPLY_ID => {
                let time = control_text(self.schedule_time);
                if !valid_time(&time) {
                    show_message(self.parent, "Automatisations", "Heure invalide. Utilisez le format HH:mm.", true);
                    return;
                }
                let interval_text = control_text(self.schedule_interval);
                let Ok(interval_minutes) = interval_text.trim().parse::<u32>() else {
                    show_message(self.parent, "Automatisations", "Intervalle invalide. Entrez un nombre de minutes.", true);
                    return;
                };
                if interval_minutes == 0 {
                    show_message(self.parent, "Automatisations", "L'intervalle doit être supérieur à 0.", true);
                    return;
                }
                if !confirm_action(self.parent, "Automatisations", &format!("Mettre à jour {} avec un lancement à {} et une répétition toutes les {} minutes ?", task.name, time, interval_minutes)) {
                    return;
                }
                let script = format!(
                    "$start = [datetime]::ParseExact('{time}', 'HH:mm', $null); \
                     $trigger = New-ScheduledTaskTrigger -Once -At $start \
                       -RepetitionInterval (New-TimeSpan -Minutes {interval}) \
                       -RepetitionDuration ([TimeSpan]::MaxValue); \
                     Set-ScheduledTask -TaskPath '{path}' -TaskName '{name}' -Trigger $trigger | Out-Null",
                    path = ps_single_quote(&task_path),
                    name = ps_single_quote(&task_name),
                    time = ps_single_quote(&time),
                    interval = interval_minutes,
                );
                match run_powershell_command(&script) {
                    Ok(_) => {
                        show_message(self.parent, "Automatisations", "Planification mise à jour.", false);
                        self.load_automations();
                    }
                    Err(error) => show_message(self.parent, "Automatisations", &error, true),
                }
            }
            _ => {}
        }
    }

    pub fn handle_menu_command(&mut self, id: u16) -> bool {
        match id {
            IDM_FILE_RELOAD => {
                self.reload();
                true
            }
            IDM_FILE_QUIT => {
                let _ = unsafe { PostMessageW(self.parent, WM_CLOSE, WPARAM(0), LPARAM(0)) };
                true
            }
            IDM_VIEW_ACTIONS | IDM_VIEW_TERMINAL | IDM_VIEW_AUTOMATIONS => {
                self.switch_view_control(match id {
                    IDM_VIEW_ACTIONS => NAV_ACTIONS_ID,
                    IDM_VIEW_TERMINAL => NAV_TERMINAL_ID,
                    _ => NAV_AUTOMATIONS_ID,
                });
                true
            }
            IDM_HELP_ABOUT => {
                show_about(self.parent);
                true
            }
            _ => false,
        }
    }

    pub fn poll_events(&mut self) {
        let mut pending = Vec::new();
        if let Some((_, receiver)) = &self.events {
            while let Ok(event) = receiver.try_recv() {
                pending.push(event);
            }
        }
        for event in pending {
            match event {
                crate::windows::process::ActionEvent::Started { command, pid } => self.append_terminal(&format!("Lancement: {command} (pid {pid})\n")),
                crate::windows::process::ActionEvent::Output(line) => {
                    self.append_terminal(&line);
                    self.append_terminal("\n");
                }
                crate::windows::process::ActionEvent::Finished { code } => self.append_terminal(&format!("Terminé (code {:?})\n", code)),
                crate::windows::process::ActionEvent::Failed(error) => self.append_terminal(&format!("ERREUR: {error}\n")),
                crate::windows::process::ActionEvent::AutomationsLoaded(result) => match result {
                    Ok(tasks) => {
                        self.scheduled_tasks = tasks;
                        let _ = unsafe { SendMessageW(self.automations, LB_RESETCONTENT, WPARAM(0), LPARAM(0)) };
                        for task in &self.scheduled_tasks {
                            listbox_add(self.automations, &format_automation(task));
                        }
                    }
                    Err(error) => {
                        let _ = unsafe { SendMessageW(self.automations, LB_RESETCONTENT, WPARAM(0), LPARAM(0)) };
                        listbox_add(self.automations, &format!("Erreur: {error}"));
                    }
                },
            }
        }
    }

    pub fn load_automations(&mut self) {
        match load_scheduled_tasks() {
            Ok(tasks) => {
                self.scheduled_tasks = tasks;
                let _ = unsafe { SendMessageW(self.automations, LB_RESETCONTENT, WPARAM(0), LPARAM(0)) };
                for task in &self.scheduled_tasks {
                    listbox_add(self.automations, &format_automation(task));
                }
            }
            Err(error) => {
                self.scheduled_tasks.clear();
                let _ = unsafe { SendMessageW(self.automations, LB_RESETCONTENT, WPARAM(0), LPARAM(0)) };
                listbox_add(self.automations, &format!("Erreur: {error}"));
            }
        }
    }

    pub fn append_terminal(&mut self, text: &str) {
        if self.terminal.0 == 0 {
            return;
        }
        if self.terminal_placeholder_visible {
            let _ = unsafe { SendMessageW(self.terminal, LB_RESETCONTENT, WPARAM(0), LPARAM(0)) };
            self.terminal_placeholder_visible = false;
        }
        let previous_len = self.terminal_lines.len();
        feed_terminal_text(&mut self.terminal_lines, &mut self.terminal_partial_line, text);
        sync_terminal_listbox(self.terminal, &mut self.terminal_lines, previous_len);
    }
    pub fn reload(&mut self) {
        self.clear_bitmaps();
        self.clear_id_to_icon();
        for hwnd in self.buttons.drain(..) {
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(hover_subclass_proc), HOVER_SUBCLASS_ID);
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            }
        }
        for hwnd in self.headers.drain(..) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            }
        }
        self.id_to_hover.clear();
        self.id_to_action.clear();
        self.variant_menu_commands.clear();
        let config = crate::storage::load().unwrap_or_else(|err| {
            log::warn!("reload: storage::load failed ({err}); rebuilding with empty commands");
            crate::storage::Config {
                version: "0.1.0".to_string(),
                default_settings: crate::storage::Settings {
                    show_categories: false,
                    icon_size: 56,
                    theme: "light".to_string(),
                    launch_at_startup: false,
                    show_descriptions: true,
                },
                categories: Vec::new(),
                commands: Vec::new(),
            }
        });
        let mut new_host = Self::build_from_config(self.parent, self.actions_container, &config);
        std::mem::swap(&mut self.buttons, &mut new_host.buttons);
        std::mem::swap(&mut self.headers, &mut new_host.headers);
        std::mem::swap(&mut self.bitmaps, &mut new_host.bitmaps);
        std::mem::swap(&mut self.id_to_icon, &mut new_host.id_to_icon);
        std::mem::swap(&mut self.id_to_action, &mut new_host.id_to_action);
        std::mem::swap(&mut self.mode, &mut new_host.mode);
        self.cell_h_grouped = new_host.cell_h_grouped;
        self.apply_header_font();
        self.layout_children(800, 600);
        std::mem::forget(new_host);
        log::info!("UiHost reloaded");
    }
}

impl Drop for UiHost {
    fn drop(&mut self) {
        let removed = unsafe { RemoveWindowSubclass(self.parent, Some(subclass_proc), SUBCLASS_ID) };
        if !removed.as_bool() {
            log::warn!("RemoveWindowSubclass failed (may already be removed)");
        }
        for hwnd in &self.buttons {
            let _ = unsafe { RemoveWindowSubclass(*hwnd, Some(hover_subclass_proc), HOVER_SUBCLASS_ID) };
        }
        self.clear_bitmaps();
        self.clear_id_to_icon();
        for font in [self.label_font, self.emoji_font, self.header_font] {
            if font.0 != 0 {
                let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
            }
        }
        for brush in [self.header_bg_brush, self.fill_brush_normal, self.fill_brush_pressed, self.fill_brush_hover, self.border_brush_normal, self.border_brush_focus] {
            if brush.0 != 0 {
                let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
            }
        }
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid_subclass: usize,
    _ref_data: usize,
) -> LRESULT {
    if msg == WM_COMMAND {
        let notif_code = (wparam.0 >> 16) as u16;
        let ctrl_id = wparam.0 as u16;
        if notif_code == 0 && lparam.0 == 0 {
            if crate::ui::handle_action_variant_menu(ctrl_id) {
                return LRESULT(0);
            }
            crate::ui::handle_menu(ctrl_id);
            return LRESULT(0);
        }
        if notif_code == BN_CLICKED as u16 {
            if matches!(ctrl_id, NAV_ACTIONS_ID | NAV_TERMINAL_ID | NAV_AUTOMATIONS_ID) {
                crate::ui::switch_view(ctrl_id);
                return LRESULT(0);
            }
            if matches!(ctrl_id, AUTOMATION_STOP_ID | AUTOMATION_TOGGLE_ID | AUTOMATION_APPLY_ID) {
                crate::ui::handle_automation(ctrl_id);
                return LRESULT(0);
            }
            crate::ui::handle_command(ctrl_id, HWND(lparam.0 as isize));
            return LRESULT(0);
        }
    }
    if msg == WM_TIMER {
        crate::ui::poll_action_events();
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn hover_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid_subclass: usize,
    _ref_data: usize,
) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let ctrl_id = GetDlgCtrlID(hwnd) as u16;
            if crate::ui::set_hover(ctrl_id, true) {
                let _ = InvalidateRect(hwnd, None, true);
            }
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tracking);
        }
        WM_MOUSELEAVE => {
            let ctrl_id = GetDlgCtrlID(hwnd) as u16;
            if crate::ui::set_hover(ctrl_id, false) {
                let _ = InvalidateRect(hwnd, None, true);
            }
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn container_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let parent = GetParent(hwnd);
            if parent.0 != 0 {
                return SendMessageW(parent, msg, wparam, lparam);
            }
        }
        WM_DRAWITEM => {
            crate::ui::draw_item_for_container(lparam.0 as *const DRAWITEMSTRUCT);
            return LRESULT(1);
        }
        WM_CTLCOLORSTATIC => {
            if let Some(result) = crate::ui::ctlcolor_static_for_container(HDC(wparam.0 as isize)) {
                return result;
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn ensure_view_class(instance: HINSTANCE) {
    REGISTER_VIEW_CLASS.call_once(|| {
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or(HCURSOR(0));
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(container_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: HICON(0),
            hCursor: cursor,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: VIEW_CLASS,
        };
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            log::warn!("RegisterClassW for view containers failed");
        }
    });
}

fn create_container(parent: HWND, instance: HINSTANCE, visible: bool) -> HWND {
    let style = if visible { WS_CHILD | WS_VISIBLE } else { WS_CHILD };
    let hwnd = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), VIEW_CLASS, PCWSTR::null(), style, 0, 0, 0, 0, parent, None, instance, None)
    };
    if hwnd.0 == 0 {
        log::warn!("CreateWindowExW returned null for a view container");
    }
    hwnd
}
fn install_menu_bar(hwnd: HWND) {
    unsafe {
        let Ok(menubar) = CreateMenu() else {
            log::warn!("CreateMenu failed; no menu bar installed");
            return;
        };
        let add_item = |menu: HMENU, id: u16, label: PCWSTR| {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, label);
        };
        let add_popup = || -> Option<HMENU> {
            match CreatePopupMenu() {
                Ok(menu) => Some(menu),
                Err(_) => None,
            }
        };
        if let Some(file) = add_popup() {
            add_item(file, IDM_FILE_RELOAD, w!("Recharger la configuration"));
            let _ = AppendMenuW(file, MF_SEPARATOR, 0, PCWSTR::null());
            add_item(file, IDM_FILE_QUIT, w!("Quitter"));
            let _ = AppendMenuW(menubar, MF_POPUP, file.0 as usize, w!("Fichier"));
        }
        if let Some(view) = add_popup() {
            add_item(view, IDM_VIEW_ACTIONS, w!("Actions"));
            add_item(view, IDM_VIEW_TERMINAL, w!("Terminal"));
            add_item(view, IDM_VIEW_AUTOMATIONS, w!("Automatisations"));
            let _ = AppendMenuW(menubar, MF_POPUP, view.0 as usize, w!("Affichage"));
        }
        if let Some(help) = add_popup() {
            add_item(help, IDM_HELP_ABOUT, w!("À propos"));
            let _ = AppendMenuW(menubar, MF_POPUP, help.0 as usize, w!("Aide"));
        }
        if SetMenu(hwnd, menubar).is_ok() {
            let _ = DrawMenuBar(hwnd);
        }
    }
}

fn show_about(owner: HWND) {
    unsafe {
        MessageBoxW(owner, w!("DevToolBox\nLanceur de scripts et d'outils Windows."), w!("À propos de DevToolBox"), MB_OK | MB_ICONINFORMATION);
    }
}

fn show_message(owner: HWND, title: &str, message: &str, warning: bool) {
    let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let message_w: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = MessageBoxW(owner, PCWSTR(message_w.as_ptr()), PCWSTR(title_w.as_ptr()), MB_OK | if warning { MB_ICONWARNING } else { MB_ICONINFORMATION });
    }
}

fn confirm_action(owner: HWND, title: &str, message: &str) -> bool {
    let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let message_w: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { MessageBoxW(owner, PCWSTR(message_w.as_ptr()), PCWSTR(title_w.as_ptr()), MB_YESNO | MB_ICONWARNING) == IDYES }
}

fn valid_time(value: &str) -> bool {
    let Some((hours, minutes)) = value.split_once(':') else {
        return false;
    };
    matches!((hours.parse::<u8>(), minutes.parse::<u8>()), (Ok(h), Ok(m)) if h < 24 && m < 60)
}

fn build_action_buttons<'a>(commands: impl IntoIterator<Item = &'a crate::storage::Command>) -> Vec<ActionButtonSpec> {
    let mut actions: Vec<ActionButtonSpec> = Vec::new();
    let mut grouped_index: HashMap<String, usize> = HashMap::new();
    for command in commands {
        if let Some(group_key) = &command.variant_group {
            let option = ActionOption {
                label: command.variant_label.clone().unwrap_or_else(|| command.name.clone()),
                command: command.command.clone(),
            };
            if let Some(index) = grouped_index.get(group_key).copied() {
                if let ActionBinding::Variants { options, .. } = &mut actions[index].binding {
                    options.push(option);
                }
                continue;
            }
            let label = command.group_name.clone().unwrap_or_else(|| command.name.clone());
            grouped_index.insert(group_key.clone(), actions.len());
            actions.push(ActionButtonSpec {
                action_key: format!("variant:{group_key}"),
                label: label.clone(),
                icon: command.icon.clone(),
                binding: ActionBinding::Variants { title: label, options: vec![option] },
            });
        } else {
            actions.push(ActionButtonSpec {
                action_key: command.id.clone(),
                label: command.name.clone(),
                icon: command.icon.clone(),
                binding: ActionBinding::Direct(command.command.clone()),
            });
        }
    }
    actions
}

fn selected_listbox_index(hwnd: HWND) -> Option<usize> {
    let index = unsafe { SendMessageW(hwnd, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    if index < 0 { None } else { Some(index as usize) }
}

fn format_automation(task: &crate::windows::process::ScheduledTask) -> String {
    format!("[{}] {} — {} — {}", task.category, task.name, task.next_run, task.state)
}

fn split_task_name(full_name: &str) -> (String, String) {
    if let Some(index) = full_name.rfind('\\') {
        let path = if index == 0 { "\\".to_string() } else { full_name[..=index].to_string() };
        let name = full_name[index + 1..].to_string();
        (path, name)
    } else {
        ("\\".to_string(), full_name.to_string())
    }
}

fn ps_single_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn run_powershell_command(script: &str) -> Result<String, String> {
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() { Err("La commande PowerShell a échoué.".to_string()) } else { Err(stderr) }
    }
}

fn load_scheduled_tasks() -> Result<Vec<crate::windows::process::ScheduledTask>, String> {
    let script = r#"
$tasks = Get-ScheduledTask | ForEach-Object {
  $info = $_ | Get-ScheduledTaskInfo
  [pscustomobject]@{
    Name = "$($_.TaskPath)$($_.TaskName)"
    Category = if ($_.TaskPath -like '\Microsoft\*') { 'Système / Windows' } else { 'Tiers / perso' }
    NextRun = if ($info.NextRunTime -and $info.NextRunTime -gt [datetime]::MinValue) { $info.NextRunTime.ToString('yyyy-MM-dd HH:mm') } else { 'N/A' }
    State = $_.State.ToString()
    Author = $_.Author
  }
}
$tasks | ConvertTo-Json -Compress
"#;
    let json = run_powershell_command(script)?;
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        serde_json::from_str(trimmed).map_err(|error| error.to_string())
    } else {
        serde_json::from_str::<crate::windows::process::ScheduledTask>(trimmed).map(|task| vec![task]).map_err(|error| error.to_string())
    }
}
#[cfg(test)]
mod dashboard_tests {
    use std::collections::VecDeque;
    use super::{ActionBinding, build_action_buttons, feed_terminal_text, valid_time};

    #[test]
    fn schedule_time_accepts_24_hour_format() {
        assert!(valid_time("00:00"));
        assert!(valid_time("23:59"));
        assert!(valid_time("2:05"));
    }

    #[test]
    fn schedule_time_rejects_invalid_values() {
        assert!(!valid_time("24:00"));
        assert!(!valid_time("12:60"));
        assert!(!valid_time("noon"));
    }

    #[test]
    fn terminal_feed_splits_crlf_lines() {
        let mut lines = VecDeque::new();
        let mut partial = String::new();
        feed_terminal_text(&mut lines, &mut partial, "alpha\r\nbeta\nchar");
        assert_eq!(lines.into_iter().collect::<Vec<_>>(), vec!["alpha", "beta"]);
        assert_eq!(partial, "char");
    }

    #[test]
    fn build_action_buttons_groups_variants() {
        let commands = vec![
            crate::storage::Command {
                id: "one".into(),
                name: "Sync Pro".into(),
                command: "a".into(),
                category: "sync".into(),
                icon: "x".into(),
                is_favorite: true,
                shortcut: None,
                variant_group: Some("sync".into()),
                group_name: Some("Synchroniser".into()),
                variant_label: Some("Pro".into()),
            },
            crate::storage::Command {
                id: "two".into(),
                name: "Sync Perso".into(),
                command: "b".into(),
                category: "sync".into(),
                icon: "x".into(),
                is_favorite: true,
                shortcut: None,
                variant_group: Some("sync".into()),
                group_name: Some("Synchroniser".into()),
                variant_label: Some("Perso".into()),
            },
        ];
        let actions = build_action_buttons(commands.iter());
        assert_eq!(actions.len(), 1);
        match &actions[0].binding {
            ActionBinding::Variants { options, .. } => {
                assert_eq!(options.len(), 2);
                assert_eq!(options[0].label, "Pro");
                assert_eq!(options[1].label, "Perso");
            }
            ActionBinding::Direct(_) => panic!("expected variants"),
        }
    }
}
fn create_text_control(
    parent: HWND,
    class_name: &str,
    text: &str,
    style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
    id: u16,
) -> Option<HWND> {
    let class_w: Vec<u16> = class_name.encode_utf16().chain(std::iter::once(0)).collect();
    let text_w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let hwnd = unsafe {
        CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(text_w.as_ptr()),
            style,
            0,
            0,
            0,
            0,
            parent,
            HMENU(id as isize),
            None,
            None,
        )
    };
    if hwnd.0 == 0 {
        return None;
    }
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    unsafe { SendMessageW(hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1)) };
    Some(hwnd)
}

fn listbox_add(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = unsafe { SendMessageW(hwnd, LB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize)) };
}

fn listbox_scroll_to_bottom(hwnd: HWND) {
    let count = unsafe { SendMessageW(hwnd, LB_GETCOUNT, WPARAM(0), LPARAM(0)).0 };
    if count > 0 {
        let _ = unsafe { SendMessageW(hwnd, LB_SETTOPINDEX, WPARAM((count - 1) as usize), LPARAM(0)) };
    }
}

fn sync_terminal_listbox(hwnd: HWND, lines: &mut VecDeque<String>, previous_len: usize) {
    let mut trimmed = false;
    while lines.len() > MAX_TERMINAL_LINES {
        lines.pop_front();
        let _ = unsafe { SendMessageW(hwnd, LB_DELETESTRING, WPARAM(0), LPARAM(0)) };
        trimmed = true;
    }
    if lines.len() > TRIMMED_TERMINAL_LINES {
        while lines.len() > TRIMMED_TERMINAL_LINES {
            lines.pop_front();
            let _ = unsafe { SendMessageW(hwnd, LB_DELETESTRING, WPARAM(0), LPARAM(0)) };
            trimmed = true;
        }
    }
    if trimmed {
        let _ = unsafe { SendMessageW(hwnd, LB_RESETCONTENT, WPARAM(0), LPARAM(0)) };
        for line in lines.iter() {
            listbox_add(hwnd, line);
        }
        listbox_scroll_to_bottom(hwnd);
        return;
    }
    let start = previous_len.min(lines.len());
    for line in lines.iter().skip(start) {
        listbox_add(hwnd, line);
    }
    listbox_scroll_to_bottom(hwnd);
}

fn feed_terminal_text(lines: &mut VecDeque<String>, partial: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '\r' => {}
            '\n' => lines.push_back(std::mem::take(partial)),
            _ => partial.push(ch),
        }
    }
}

fn control_text(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied as usize])
}

fn create_button(
    parent_hwnd: HWND,
    cell: &crate::ui::xaml_gen::GridCell,
    icon_size: u32,
    icon_dirs: &[std::path::PathBuf],
    id_to_icon: &mut HashMap<u16, IconSource>,
    ctrl_id: Option<u16>,
) -> Option<HWND> {
    let resolution = icons::resolve_icon(&cell.icon, icon_dirs);
    let label_w: Vec<u16> = cell.label.encode_utf16().chain(std::iter::once(0)).collect();
    let class_w: Vec<u16> = "BUTTON\0".encode_utf16().collect();
    let hmenu = ctrl_id.map(|id| HMENU(id as isize)).unwrap_or(HMENU(0));
    let hwnd_btn = unsafe {
        CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(label_w.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_OWNERDRAW as u32),
            0,
            0,
            0,
            0,
            parent_hwnd,
            hmenu,
            None,
            None,
        )
    };
    if hwnd_btn.0 == 0 {
        log::warn!("CreateWindowExW returned null for button '{}'", cell.label);
        return None;
    }
    unsafe { ShowWindow(hwnd_btn, SW_SHOW) };
    let hover_ok = unsafe { SetWindowSubclass(hwnd_btn, Some(hover_subclass_proc), HOVER_SUBCLASS_ID, 0) };
    if !hover_ok.as_bool() {
        log::warn!("SetWindowSubclass(hover) failed for button '{}'", cell.label);
    }
    if let Some(id) = ctrl_id {
        let source = match &resolution {
            icons::IconResolution::EmojiFallback(text) if !text.is_empty() => IconSource::Emoji(text.clone()),
            icons::IconResolution::Image(path) => match icons::decode_resize_file(path, icon_size) {
                Ok(decoded) => match icons::gdi::rgba_to_hbitmap(&decoded) {
                    Ok(hbitmap) => IconSource::Bitmap(hbitmap),
                    Err(_) => IconSource::NoIcon,
                },
                Err(_) => IconSource::NoIcon,
            },
            _ => IconSource::NoIcon,
        };
        id_to_icon.insert(id, source);
    }
    Some(hwnd_btn)
}

fn create_font_segoe_ui(pt_size: i32, bold: bool) -> HFONT {
    let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    let mut face_arr = [0i16; 32];
    for (i, ch) in face.iter().enumerate().take(31) {
        face_arr[i] = *ch as i16;
    }
    let height = -(pt_size * 96 / 72);
    let weight = if bold { FW_SEMIBOLD.0 as i32 } else { FW_REGULAR.0 as i32 };
    unsafe {
        CreateFontW(height, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, windows::core::PCWSTR(face_arr.as_ptr() as *const u16))
    }
}

fn create_font_segoe_ui_emoji(pt_size: i32) -> HFONT {
    let face: Vec<u16> = "Segoe UI Emoji\0".encode_utf16().collect();
    let mut face_arr = [0i16; 32];
    for (i, ch) in face.iter().enumerate().take(31) {
        face_arr[i] = *ch as i16;
    }
    let height = -(pt_size * 96 / 72);
    unsafe {
        CreateFontW(height, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, 1, 0, 0, 5, 0, windows::core::PCWSTR(face_arr.as_ptr() as *const u16))
    }
}

fn layout_flat(grid: &GridModel, buttons: &[HWND], width: u32, height: u32, cell_h: u32) {
    let cols = grid.cols.max(1);
    let rows = grid.row_count().max(1);
    const PAD: u32 = 8;
    let avail_w = width.saturating_sub(PAD * (cols + 1));
    let avail_h = height.saturating_sub(PAD * (rows + 1));
    let cell_w = (avail_w / cols).max(1);
    let effective_cell_h = cell_h.min(avail_h / rows).max(1);
    for (btn_hwnd, cell) in buttons.iter().zip(grid.cells.iter()) {
        let x = (PAD + cell.col * (cell_w + PAD)) as i32;
        let y = (PAD + cell.row * (effective_cell_h + PAD)) as i32;
        let _ = unsafe { SetWindowPos(*btn_hwnd, HWND_TOP, x, y, cell_w as i32, effective_cell_h as i32, SWP_NOZORDER) };
    }
}

fn layout_grouped(
    rows_snapshot: &[(bool, usize)],
    headers: &[HWND],
    buttons: &[HWND],
    width: u32,
    _height: u32,
    cols: u32,
    cell_h: u32,
) {
    const PAD: u32 = 8;
    const HEADER_H: u32 = 24;
    let cols = cols.max(1);
    let avail_w = width.saturating_sub(PAD * (cols + 1));
    let cell_w = (avail_w / cols).max(1);
    let mut y: i32 = PAD as i32;
    let mut header_idx = 0usize;
    let mut button_idx = 0usize;
    for &(is_header, cell_count) in rows_snapshot {
        if is_header {
            if header_idx < headers.len() {
                let _ = unsafe { SetWindowPos(headers[header_idx], HWND_TOP, PAD as i32, y, (width.saturating_sub(PAD * 2)) as i32, HEADER_H as i32, SWP_NOZORDER) };
                header_idx += 1;
            }
            y += (HEADER_H + PAD) as i32;
        } else {
            for col in 0..cell_count {
                if button_idx < buttons.len() {
                    let x = (PAD + col as u32 * (cell_w + PAD)) as i32;
                    let _ = unsafe { SetWindowPos(buttons[button_idx], HWND_TOP, x, y, cell_w as i32, cell_h as i32, SWP_NOZORDER) };
                    button_idx += 1;
                }
            }
            y += (cell_h + PAD) as i32;
        }
    }
}
