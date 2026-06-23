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

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, COLOR_WINDOW, DEFAULT_GUI_FONT, HBITMAP, HBRUSH};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DrawMenuBar,
    GetParent, GetWindowLongPtrW, LoadCursorW, MessageBoxW, PostMessageW, RegisterClassW,
    SendMessageW, SetMenu, SetTimer, SetWindowPos, ShowWindow, BN_CLICKED, BS_CENTER, BS_MULTILINE,
    BS_PUSHBUTTON, BS_VCENTER, CS_HREDRAW, CS_VREDRAW, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY,
    ES_WANTRETURN, GWLP_HINSTANCE, HCURSOR, HICON, HMENU, HWND_TOP, IDC_ARROW, MB_ICONINFORMATION,
    MB_OK, MF_POPUP, MF_SEPARATOR, MF_STRING, SWP_NOZORDER, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE,
    WM_CLOSE, WM_COMMAND, WM_SETFONT, WM_TIMER, WNDCLASSW, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};

use crate::icons;
use crate::ui::xaml_gen::{
    assign_control_ids, build_grid, build_sectioned, cell_size, GridEntry, GridModel, GridSection,
    SectionRow, SectionedModel,
};

// ---------------------------------------------------------------------------
// Subclass identifier
// ---------------------------------------------------------------------------

/// Unique subclass id for WinFXStart's WM_COMMAND handler.
///
/// Must be unique per (HWND, proc) pair.  We use 1 as our private id since
/// tao does not install its own subclass under this id.
const SUBCLASS_ID: usize = 1;
const NAV_ACTIONS_ID: u16 = 10;
const NAV_TERMINAL_ID: u16 = 11;
const NAV_AUTOMATIONS_ID: u16 = 12;
const CONTENT_TOP: u32 = 52;

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
    pub id_to_command: HashMap<u16, String>,
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
    active_view: DashboardView,
    events: Option<(
        Sender<crate::windows::process::ActionEvent>,
        Receiver<crate::windows::process::ActionEvent>,
    )>,
    terminal_text: String,
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
                    icon_size: 80,
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
                    },
                    crate::storage::Command {
                        id: "cmd".into(),
                        name: "Invite de commandes".into(),
                        command: "cmd.exe".into(),
                        category: "system".into(),
                        icon: "💻".into(),
                        is_favorite: true,
                        shortcut: None,
                    },
                    crate::storage::Command {
                        id: "ipconfig".into(),
                        name: "Adresse IP".into(),
                        command: "ipconfig /all".into(),
                        category: "system".into(),
                        icon: "🌐".into(),
                        is_favorite: true,
                        shortcut: None,
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

            let sections: Vec<GridSection> = groups
                .iter()
                .map(|g| {
                    let header = match g.category {
                        Some(cat) => cat.name.clone(),
                        None => "Sans catégorie".to_string(),
                    };
                    let entries: Vec<GridEntry> = g
                        .commands
                        .iter()
                        .map(|c| GridEntry {
                            label: c.name.clone(),
                            icon: c.icon.clone(),
                            command_id: Some(c.id.clone()),
                        })
                        .collect();
                    GridSection { header, entries }
                })
                .collect();

            let sectioned = build_sectioned(&sections, 3);

            // Collect all cells in row order to assign control ids.
            let all_cells: Vec<crate::ui::xaml_gen::GridCell> = sectioned
                .rows
                .iter()
                .flat_map(|row| match row {
                    SectionRow::Cells { cells, .. } => cells.clone(),
                    _ => Vec::new(),
                })
                .collect();

            // Build a lookup: command_id → command string.
            let cmd_str_map: HashMap<&str, &str> = config
                .commands
                .iter()
                .map(|c| (c.id.as_str(), c.command.as_str()))
                .collect();

            let id_pairs = assign_control_ids(&all_cells);
            let id_to_command: HashMap<u16, String> = id_pairs
                .iter()
                .filter_map(|(ctrl_id, cmd_id)| {
                    cmd_str_map
                        .get(cmd_id.as_str())
                        .map(|&cmd| (*ctrl_id, cmd.to_string()))
                })
                .collect();

            // Create STATIC header controls and BUTTON controls.
            let mut buttons: Vec<HWND> = Vec::new();
            let mut headers: Vec<HWND> = Vec::new();
            let mut bitmaps: Vec<HBITMAP> = Vec::new();

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
                                &mut bitmaps,
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
                bitmaps,
                mode: LayoutMode::Grouped { model: sectioned },
                id_to_command,
                cell_h_grouped: cell_h,
                nav_buttons: Vec::new(),
                actions_container: HWND(0),
                terminal_container: HWND(0),
                automations_container: HWND(0),
                terminal: HWND(0),
                automations: HWND(0),
                active_view: DashboardView::Actions,
                events: None,
                terminal_text: String::new(),
            }
        } else {
            // ----------------------------------------------------------------
            // FLAT PATH (show_categories == false)
            // ----------------------------------------------------------------
            let cmd_str_map: HashMap<&str, &str> = config
                .commands
                .iter()
                .map(|c| (c.id.as_str(), c.command.as_str()))
                .collect();

            let entries: Vec<GridEntry> = config
                .commands
                .iter()
                .filter(|c| c.is_favorite)
                .map(|c| GridEntry {
                    label: c.name.clone(),
                    icon: c.icon.clone(),
                    command_id: Some(c.id.clone()),
                })
                .collect();

            log::info!("Loaded {} favorite commands (flat mode)", entries.len());

            let grid = build_grid(&entries, 3);

            let id_pairs = assign_control_ids(&grid.cells);
            let id_to_command: HashMap<u16, String> = id_pairs
                .iter()
                .filter_map(|(ctrl_id, cmd_id)| {
                    cmd_str_map
                        .get(cmd_id.as_str())
                        .map(|&cmd| (*ctrl_id, cmd.to_string()))
                })
                .collect();

            let mut buttons: Vec<HWND> = Vec::with_capacity(grid.cells.len());
            let mut bitmaps: Vec<HBITMAP> = Vec::new();
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
                    &mut bitmaps,
                    ctrl_id,
                ) {
                    buttons.push(btn);
                }
            }

            UiHost {
                parent: parent_hwnd,
                buttons,
                headers: Vec::new(),
                bitmaps,
                mode: LayoutMode::Flat { grid },
                id_to_command,
                cell_h_grouped: cell_h,
                nav_buttons: Vec::new(),
                actions_container: HWND(0),
                terminal_container: HWND(0),
                automations_container: HWND(0),
                terminal: HWND(0),
                automations: HWND(0),
                active_view: DashboardView::Actions,
                events: None,
                terminal_text: String::new(),
            }
        }
    }

    /// Delete and release all tracked GDI bitmap handles.
    ///
    /// Must be called before any button rebuild / reload to avoid leaking
    /// GDI handles (AC3).
    pub fn clear_bitmaps(&mut self) {
        for hbitmap in self.bitmaps.drain(..) {
            icons::gdi::delete_bitmap(hbitmap);
        }
        log::debug!("clear_bitmaps: all HBITMAP handles freed");
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
                layout_flat(grid, &self.buttons, content_w, content_h, self.cell_h_grouped);
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

        // Terminal / automations EDITs fill their container with a small inset.
        const INSET: u32 = 8;
        let edit_w = content_w.saturating_sub(INSET * 2).max(1);
        let edit_h = content_h.saturating_sub(INSET * 2).max(1);
        for edit in [self.terminal, self.automations] {
            if edit.0 != 0 {
                let _ = unsafe {
                    SetWindowPos(
                        edit,
                        HWND_TOP,
                        INSET as i32,
                        INSET as i32,
                        edit_w as i32,
                        edit_h as i32,
                        SWP_NOZORDER,
                    )
                };
            }
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
        // EDIT controls are permanently visible inside their container; the
        // container's own visibility (toggled on view switch) governs them.
        let edit_style = WS_CHILD
            | WS_VISIBLE
            | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_WANTRETURN) as u32,
            )
            | WS_VSCROLL;
        self.terminal = create_text_control(
            self.terminal_container,
            "EDIT",
            "Aucune action lancée.",
            edit_style,
            0,
        )
        .unwrap_or(HWND(0));
        self.automations = create_text_control(
            self.automations_container,
            "EDIT",
            "Ouvrez cette vue pour charger les tâches planifiées.",
            edit_style,
            0,
        )
        .unwrap_or(HWND(0));
        self.apply_view_visibility();
    }

    pub fn launch_action(&mut self, command: &str) {
        self.switch_view(DashboardView::Terminal);
        self.terminal_text.push_str(&format!("\r\n> {command}\r\n"));
        self.refresh_terminal();
        let Some((sender, _)) = &self.events else {
            return;
        };
        if let Err(error) = crate::windows::process::launch_captured(command, sender.clone()) {
            self.terminal_text.push_str(&format!("ERREUR: {error}\r\n"));
            self.refresh_terminal();
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
        let view = match control_id {
            NAV_TERMINAL_ID => DashboardView::Terminal,
            NAV_AUTOMATIONS_ID => DashboardView::Automations,
            _ => DashboardView::Actions,
        };
        self.switch_view(view);
    }

    /// Dispatch a menu-bar command id. Returns `true` if the id was recognized.
    pub fn handle_menu_command(&mut self, id: u16) -> bool {
        match id {
            IDM_FILE_RELOAD => self.reload(),
            IDM_FILE_QUIT => unsafe {
                let _ = PostMessageW(self.parent, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            IDM_VIEW_ACTIONS => self.switch_view(DashboardView::Actions),
            IDM_VIEW_TERMINAL => self.switch_view(DashboardView::Terminal),
            IDM_VIEW_AUTOMATIONS => self.switch_view(DashboardView::Automations),
            IDM_HELP_ABOUT => show_about(self.parent),
            _ => return false,
        }
        true
    }

    pub fn poll_events(&mut self) {
        let events: Vec<_> = self
            .events
            .as_ref()
            .map(|(_, receiver)| receiver.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            use crate::windows::process::ActionEvent;
            match event {
                ActionEvent::Started { command, pid } => {
                    log::info!("Action started: pid={pid} command={command}");
                    self.terminal_text
                        .push_str(&format!("Démarré (PID {pid}) : {command}\r\n"));
                }
                ActionEvent::Output(line) => {
                    self.terminal_text.push_str(&line);
                    self.terminal_text.push_str("\r\n");
                }
                ActionEvent::Finished { code } => {
                    log::info!("Action finished: code={code:?}");
                    self.terminal_text
                        .push_str(&format!("Terminé (code {:?})\r\n", code));
                }
                ActionEvent::Failed(error) => {
                    log::error!("Action failed: {error}");
                    self.terminal_text.push_str(&format!("ERREUR: {error}\r\n"));
                }
                ActionEvent::AutomationsLoaded(text) => {
                    log::info!("Scheduled tasks loaded: {} characters", text.len());
                    set_control_text(self.automations, &text);
                }
            }
        }
        if self.terminal_text.len() > 250_000 {
            self.terminal_text.drain(..100_000);
        }
        self.refresh_terminal();
    }

    fn load_automations(&self) {
        let Some((sender, _)) = &self.events else {
            return;
        };
        let sender = sender.clone();
        std::thread::spawn(move || {
            let script = r#"
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$currentUser = [Environment]::UserName
$items = Get-ScheduledTask -ErrorAction SilentlyContinue | ForEach-Object {
    $task = $_
    $info = $null
    try { $info = Get-ScheduledTaskInfo -InputObject $task -ErrorAction Stop } catch {}
    $fullName = "$($task.TaskPath)$($task.TaskName)"
    $category = if ($task.TaskPath -like '\Microsoft\*' -or $task.Author -match 'Microsoft|Windows') {
        'WINDOWS / SYSTEME'
    } elseif ($task.TaskName -match 'Perso|WinFXStart' -or $task.Author -match [regex]::Escape($currentUser)) {
        'PERSONNELLES / WINFXSTART'
    } else {
        'LOGICIELS TIERS'
    }
    [pscustomobject]@{
        Category = $category
        Name = $fullName
        NextRun = if ($info -and $info.NextRunTime -gt [datetime]::MinValue) { $info.NextRunTime.ToString('dd/MM/yyyy HH:mm:ss') } else { 'Non planifiée' }
        State = [string]$task.State
        Author = [string]$task.Author
    }
}
$lines = [System.Collections.Generic.List[string]]::new()
foreach ($category in @('PERSONNELLES / WINFXSTART', 'LOGICIELS TIERS', 'WINDOWS / SYSTEME')) {
    [void]$lines.Add("=== $category ===")
    $group = @($items | Where-Object Category -eq $category | Sort-Object NextRun, Name)
    if ($group.Count -eq 0) { [void]$lines.Add('(aucune tâche)') }
    foreach ($item in $group) {
        [void]$lines.Add("[$($item.State)] $($item.NextRun)  $($item.Name)")
        if ($item.Author) { [void]$lines.Add("    Auteur : $($item.Author)") }
    }
    [void]$lines.Add('')
}
$lines -join "`r`n"
"#;
            let output = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .output()
                .map_err(|error| error.to_string());
            let text = match output {
                Ok(output) if output.status.success() => {
                    String::from_utf8_lossy(&output.stdout).into_owned()
                }
                Ok(output) => format!(
                    "Impossible de lire les tâches planifiées:\r\n{}",
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(error) => format!("Impossible de lancer PowerShell: {error}"),
            };
            let _ = sender.send(crate::windows::process::ActionEvent::AutomationsLoaded(
                text,
            ));
        });
    }

    fn refresh_terminal(&self) {
        set_control_text(self.terminal, &self.terminal_text);
    }

    /// Show the active view's container and hide the other two.
    ///
    /// Only the three container windows are toggled; their child controls
    /// (buttons, headers, EDITs) inherit visibility from their parent. Because
    /// each container is opaque (its class brush erases the background), the
    /// revealed view fully repaints its area — no residue from the previous
    /// view bleeds through.
    fn apply_view_visibility(&self) {
        let toggle = |hwnd: HWND, shown: bool| {
            if hwnd.0 != 0 {
                unsafe { ShowWindow(hwnd, if shown { SW_SHOW } else { SW_HIDE }) };
            }
        };
        toggle(
            self.actions_container,
            self.active_view == DashboardView::Actions,
        );
        toggle(
            self.terminal_container,
            self.active_view == DashboardView::Terminal,
        );
        toggle(
            self.automations_container,
            self.active_view == DashboardView::Automations,
        );
    }

    fn layout_dashboard(&self, width: u32, height: u32) {
        let nav_width = (width.saturating_sub(32) / 3).max(1);
        for (index, hwnd) in self.nav_buttons.iter().enumerate() {
            let _ = unsafe {
                SetWindowPos(
                    *hwnd,
                    HWND_TOP,
                    8 + index as i32 * (nav_width as i32 + 8),
                    8,
                    nav_width as i32,
                    36,
                    SWP_NOZORDER,
                )
            };
        }
        // The three view containers each fill the whole content area; only one
        // is visible at a time (see apply_view_visibility).
        let content_h = height.saturating_sub(CONTENT_TOP).max(1);
        for container in [
            self.actions_container,
            self.terminal_container,
            self.automations_container,
        ] {
            if container.0 != 0 {
                let _ = unsafe {
                    SetWindowPos(
                        container,
                        HWND_TOP,
                        0,
                        CONTENT_TOP as i32,
                        width as i32,
                        content_h as i32,
                        SWP_NOZORDER,
                    )
                };
            }
        }
    }

    /// Reload configuration and rebuild child controls in-place.
    ///
    /// This is the AC2 grid-update seam: after calling
    /// `storage::toggle_favorite` + `storage::save`, call this to reflect the
    /// change in the visible favorites grid.
    ///
    /// The subclass proc remains installed across reloads (installed once in
    /// `new`, removed in `Drop`).
    pub fn reload(&mut self) {
        // Free existing GDI handles.
        self.clear_bitmaps();
        // Clear control vecs. Child buttons are still Windows children of parent
        // but we lose our HWND references.  Because reload rebuilds from config
        // and creates fresh children, the old ones become orphaned.  To avoid
        // accumulating orphaned child controls, we destroy them explicitly.
        for hwnd in self.buttons.drain(..) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            }
        }
        for hwnd in self.headers.drain(..) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            }
        }

        let config = crate::storage::load().unwrap_or_else(|err| {
            log::warn!("reload: storage::load failed ({err}); rebuilding with empty commands");
            crate::storage::Config {
                version: "0.1.0".to_string(),
                default_settings: crate::storage::Settings {
                    show_categories: false,
                    icon_size: 80,
                    theme: "light".to_string(),
                    launch_at_startup: false,
                    show_descriptions: true,
                },
                categories: Vec::new(),
                commands: Vec::new(),
            }
        });

        let mut new_host = Self::build_from_config(self.parent, self.actions_container, &config);

        // Replace state atomically before showing new buttons.
        // Use std::mem::swap to move fields out of new_host without violating
        // the Drop constraint (we swap, then let new_host drop with the old/empty values).
        std::mem::swap(&mut self.buttons, &mut new_host.buttons);
        std::mem::swap(&mut self.headers, &mut new_host.headers);
        std::mem::swap(&mut self.bitmaps, &mut new_host.bitmaps);
        // For LayoutMode (not Clone), swap via the enum directly.
        std::mem::swap(&mut self.mode, &mut new_host.mode);
        std::mem::swap(&mut self.id_to_command, &mut new_host.id_to_command);
        self.cell_h_grouped = new_host.cell_h_grouped;

        // new_host drops here with the OLD (already-empty after clear_bitmaps)
        // bitmaps vec and old buttons/headers vecs (also empty after drain+destroy).
        // Drop for UiHost will call RemoveWindowSubclass(new_host.parent, ...) —
        // but new_host.parent is the SAME hwnd, so we'd double-remove.
        // Prevent this by dropping new_host manually WITHOUT running its Drop:
        // we use std::mem::forget since the old bitmaps in new_host are already
        // empty (cleared above), and old buttons/headers were explicitly destroyed.
        std::mem::forget(new_host);

        // Re-layout new controls.
        self.layout_children(800, 600);
        log::info!("UiHost reloaded");
    }
}

impl Drop for UiHost {
    /// Remove the WM_COMMAND subclass and free all GDI bitmap handles on drop.
    ///
    /// Restores tao's original subclass chain (Decision D1/D2).
    fn drop(&mut self) {
        // Remove the subclass BEFORE freeing handles so the callback cannot
        // fire on a partially-freed UiHost.
        let removed =
            unsafe { RemoveWindowSubclass(self.parent, Some(subclass_proc), SUBCLASS_ID) };
        if !removed.as_bool() {
            log::warn!("RemoveWindowSubclass failed (may already be removed)");
        } else {
            log::info!("WM_COMMAND subclass removed from parent HWND");
        }

        self.clear_bitmaps();
        log::debug!("UiHost dropped: all GDI handles freed");
    }
}

// ---------------------------------------------------------------------------
// WM_COMMAND subclass proc (issue #7 — Decision D1/D2)
// ---------------------------------------------------------------------------

/// Win32 subclass window procedure installed on the parent HWND.
///
/// Handles `WM_COMMAND` with `HIWORD(wParam) == BN_CLICKED` only.  All other
/// messages (and WM_COMMAND notifications that are NOT BN_CLICKED) are
/// forwarded via `DefSubclassProc` so tao's own message handling is
/// unaffected.
///
/// State access: the id→command map lives on the `UiHost` in the thread-local
/// `HOST`.  A short-lived `borrow()` (not `borrow_mut`) prevents re-entrant
/// `BorrowMutError` (Decision D2).
unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _uid_subclass: usize,
    _ref_data: usize,
) -> windows::Win32::Foundation::LRESULT {
    if msg == WM_COMMAND {
        let notif_code = (wparam.0 >> 16) as u16;
        let ctrl_id = wparam.0 as u16;

        // Menu commands carry HIWORD == 0 AND lParam == 0 (no control HWND).
        // Buttons also use HIWORD == 0 (BN_CLICKED) but always carry their HWND
        // in lParam, so the lParam == 0 test cleanly isolates the menu.
        if notif_code == 0 && lparam.0 == 0 {
            crate::ui::handle_menu(ctrl_id);
            return windows::Win32::Foundation::LRESULT(0);
        }

        if notif_code == BN_CLICKED as u16 {
            if matches!(
                ctrl_id,
                NAV_ACTIONS_ID | NAV_TERMINAL_ID | NAV_AUTOMATIONS_ID
            ) {
                crate::ui::switch_view(ctrl_id);
                return windows::Win32::Foundation::LRESULT(0);
            }
            // Route to handle_command via the thread-local HOST.
            crate::ui::handle_command(ctrl_id);
            // Return 0 to indicate the message was processed.
            return windows::Win32::Foundation::LRESULT(0);
        }
    }
    if msg == WM_TIMER {
        crate::ui::poll_action_events();
        return windows::Win32::Foundation::LRESULT(0);
    }

    // Forward all unhandled messages (and unmatched WM_COMMAND) to the next
    // subclass proc in the chain (ultimately tao's original wndproc).
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

// ---------------------------------------------------------------------------
// View containers (robust view isolation)
// ---------------------------------------------------------------------------

/// Window procedure for the per-view container windows.
///
/// Containers exist solely to give each dashboard view an opaque, isolated
/// surface: their class background brush erases the content area, which kills
/// the residue that previously bled through when switching back from the
/// terminal.  They own no command logic — `WM_COMMAND` from action buttons is
/// forwarded verbatim to the top-level parent so `subclass_proc` stays the
/// single command sink.
unsafe extern "system" fn container_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_COMMAND {
        let parent = GetParent(hwnd);
        if parent.0 != 0 {
            return SendMessageW(parent, msg, wparam, lparam);
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Register [`VIEW_CLASS`] once per process.
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
            // (COLOR_WINDOW + 1) is the classic "system color as brush" encoding
            // accepted by RegisterClass for hbrBackground.
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: VIEW_CLASS,
        };
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            log::warn!("RegisterClassW for view containers failed");
        } else {
            log::info!("View container class registered");
        }
    });
}

/// Create one opaque view container parented to `parent`.
fn create_container(parent: HWND, instance: HINSTANCE, visible: bool) -> HWND {
    let style = if visible {
        WS_CHILD | WS_VISIBLE
    } else {
        WS_CHILD
    };
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            VIEW_CLASS,
            PCWSTR::null(),
            style,
            0,
            0,
            0,
            0,
            parent,
            None,
            instance,
            None,
        )
    };
    if hwnd.0 == 0 {
        log::warn!("CreateWindowExW returned null for a view container");
    }
    hwnd
}

// ---------------------------------------------------------------------------
// Menu bar
// ---------------------------------------------------------------------------

/// Build and attach the native menu bar to `hwnd`.
///
/// Menu selections arrive as `WM_COMMAND` with `HIWORD == 0` and `lParam == 0`,
/// dispatched by `subclass_proc` → `ui::handle_menu` → `handle_menu_command`.
fn install_menu_bar(hwnd: HWND) {
    unsafe {
        let Ok(menubar) = CreateMenu() else {
            log::warn!("CreateMenu failed; no menu bar installed");
            return;
        };

        let add_item = |menu: HMENU, id: u16, label: PCWSTR| {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, label);
        };
        let add_popup = |label: PCWSTR| -> Option<HMENU> {
            match CreatePopupMenu() {
                Ok(m) => Some(m),
                Err(_) => {
                    log::warn!("CreatePopupMenu failed for a menu");
                    let _ = label;
                    None
                }
            }
        };

        if let Some(file) = add_popup(w!("Fichier")) {
            add_item(file, IDM_FILE_RELOAD, w!("Recharger la configuration"));
            let _ = AppendMenuW(file, MF_SEPARATOR, 0, PCWSTR::null());
            add_item(file, IDM_FILE_QUIT, w!("Quitter"));
            let _ = AppendMenuW(menubar, MF_POPUP, file.0 as usize, w!("Fichier"));
        }

        if let Some(view) = add_popup(w!("Affichage")) {
            add_item(view, IDM_VIEW_ACTIONS, w!("Actions"));
            add_item(view, IDM_VIEW_TERMINAL, w!("Terminal"));
            add_item(view, IDM_VIEW_AUTOMATIONS, w!("Automatisations"));
            let _ = AppendMenuW(menubar, MF_POPUP, view.0 as usize, w!("Affichage"));
        }

        if let Some(help) = add_popup(w!("Aide")) {
            add_item(help, IDM_HELP_ABOUT, w!("À propos"));
            let _ = AppendMenuW(menubar, MF_POPUP, help.0 as usize, w!("Aide"));
        }

        if SetMenu(hwnd, menubar).is_ok() {
            let _ = DrawMenuBar(hwnd);
            log::info!("Native menu bar installed");
        } else {
            log::warn!("SetMenu failed; no menu bar installed");
        }
    }
}

/// Show the "À propos" dialog.
fn show_about(owner: HWND) {
    unsafe {
        MessageBoxW(
            owner,
            w!("WinFXStart v0.1.0\nLanceur de commandes Windows 11.\n\nUI Win32 native (Rust + tao)."),
            w!("À propos de WinFXStart"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn create_text_control(
    parent: HWND,
    class_name: &str,
    text: &str,
    style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
    id: u16,
) -> Option<HWND> {
    let class_w: Vec<u16> = class_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
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

fn set_control_text(hwnd: HWND, text: &str) {
    if hwnd.0 == 0 {
        return;
    }
    let text_w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(hwnd, PCWSTR(text_w.as_ptr()))
    };
}

/// Create a BUTTON child control for a grid cell, applying icon if resolved.
///
/// `ctrl_id` is the `u16` control id to assign via the `CreateWindowExW`
/// `hMenu` parameter (per Decision D4).  `None` means no control id is set
/// (the system assigns 0, which is never dispatched as BN_CLICKED by our proc).
///
/// Returns `Some(HWND)` on success or `None` when `CreateWindowExW` fails.
fn create_button(
    parent_hwnd: HWND,
    cell: &crate::ui::xaml_gen::GridCell,
    icon_size: u32,
    icon_dirs: &[std::path::PathBuf],
    bitmaps: &mut Vec<HBITMAP>,
    ctrl_id: Option<u16>,
) -> Option<HWND> {
    let resolution = icons::resolve_icon(&cell.icon, icon_dirs);

    let button_label = match &resolution {
        icons::IconResolution::EmojiFallback(text) if !text.is_empty() => {
            format!("{text}\r\n{}", cell.label)
        }
        _ => cell.label.clone(),
    };

    let label_w: Vec<u16> = button_label
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let class_w: Vec<u16> = "BUTTON\0".encode_utf16().collect();

    // Cast the u16 control id to HMENU as required by CreateWindowExW.
    // Decision D4: ids start at CONTROL_ID_BASE (1000); 0 means "no id".
    let hmenu: HMENU = match ctrl_id {
        Some(id) => HMENU(id as isize),
        None => HMENU(0),
    };

    let hwnd_btn = unsafe {
        CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(label_w.as_ptr()),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                    (BS_PUSHBUTTON | BS_MULTILINE | BS_CENTER | BS_VCENTER) as u32,
                ),
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

    // Apply the Windows UI font explicitly. Without WM_SETFONT, raw Win32
    // controls use the legacy system font and render emoji/labels extremely
    // small on modern high-DPI displays.
    let ui_font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    unsafe {
        SendMessageW(hwnd_btn, WM_SETFONT, WPARAM(ui_font.0 as usize), LPARAM(1));
    }

    unsafe { ShowWindow(hwnd_btn, SW_SHOW) };

    if let icons::IconResolution::Image(path) = &resolution {
        match icons::decode_resize_file(path, icon_size) {
            Ok(decoded) => match icons::gdi::rgba_to_hbitmap(&decoded) {
                Ok(hbitmap) => {
                    icons::gdi::set_button_bitmap(hwnd_btn, hbitmap);
                    bitmaps.push(hbitmap);
                    log::info!("Icon applied to '{}': {:?}", cell.label, path);
                }
                Err(e) => {
                    log::warn!(
                        "rgba_to_hbitmap failed for '{}': {e}; using text label",
                        cell.label
                    );
                }
            },
            Err(e) => {
                log::warn!(
                    "decode_resize_rgba failed for '{}': {e}; using text label",
                    cell.label
                );
            }
        }
    }

    Some(hwnd_btn)
}

/// Flat-grid layout: uniform grid of button cells.  SetWindowPos-only.
///
/// `cell_h` is derived from `cell_size(icon_size).1` (AC3 — Decision D6).
fn layout_flat(grid: &GridModel, buttons: &[HWND], width: u32, height: u32, cell_h: u32) {
    let cols = grid.cols.max(1);
    let rows = grid.row_count().max(1);

    const PAD: u32 = 8;
    let avail_w = width.saturating_sub(PAD * (cols + 1));
    // `height` is the container's client height (content area); coords are
    // relative to the container origin (0, 0).
    let avail_h = height.saturating_sub(PAD * (rows + 1));
    // Width: distribute available width; height: use cell_size-derived value,
    // but if window is shorter than needed, fall back to the avail proportional height.
    let cell_w = (avail_w / cols).max(1);
    // Use the icon_size-derived cell_h, but do not exceed avail_h / rows.
    let effective_cell_h = cell_h.min(avail_h / rows).max(1);

    for (btn_hwnd, cell) in buttons.iter().zip(grid.cells.iter()) {
        let x = (PAD + cell.col * (cell_w + PAD)) as i32;
        let y = (PAD + cell.row * (effective_cell_h + PAD)) as i32;

        let _ = unsafe {
            SetWindowPos(
                *btn_hwnd,
                HWND_TOP,
                x,
                y,
                cell_w as i32,
                effective_cell_h as i32,
                SWP_NOZORDER,
            )
        };
    }
}

/// Grouped layout: stacks header rows and button rows vertically.
/// SetWindowPos-only (no bitmap creation — preserves issue #5 AC3 leak-safety).
///
/// `rows_snapshot` is `(is_header, cell_count_in_row)` for each SectionRow.
/// `cell_h` is derived from `cell_size(icon_size).1` (replaces hardcoded 80).
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
    const HEADER_H: u32 = 24; // fixed height for STATIC header controls
    let cols = cols.max(1);
    let avail_w = width.saturating_sub(PAD * (cols + 1));
    let cell_w = (avail_w / cols).max(1);

    // Coords are relative to the container origin (0, 0).
    let mut y: i32 = PAD as i32;
    let mut header_idx: usize = 0;
    let mut button_idx: usize = 0;

    for &(is_header, cell_count) in rows_snapshot {
        if is_header {
            if header_idx < headers.len() {
                let _ = unsafe {
                    SetWindowPos(
                        headers[header_idx],
                        HWND_TOP,
                        PAD as i32,
                        y,
                        (width.saturating_sub(PAD * 2)) as i32,
                        HEADER_H as i32,
                        SWP_NOZORDER,
                    )
                };
                header_idx += 1;
            }
            y += (HEADER_H + PAD) as i32;
        } else {
            // One Cells row: up to `cell_count` buttons.
            for col in 0..cell_count {
                if button_idx < buttons.len() {
                    let x = (PAD + col as u32 * (cell_w + PAD)) as i32;
                    let _ = unsafe {
                        SetWindowPos(
                            buttons[button_idx],
                            HWND_TOP,
                            x,
                            y,
                            cell_w as i32,
                            cell_h as i32,
                            SWP_NOZORDER,
                        )
                    };
                    button_idx += 1;
                }
            }
            y += (cell_h + PAD) as i32;
        }
    }
}
