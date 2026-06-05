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
//! ## Deferred CRUD UI seam (issue #9)
//!
//! `crate::storage::{add_category, rename_category, remove_category}` form the
//! callable API seam for the future settings/alias-editor UI (issue #9).
//! No interactive widgets are built here; the logic + persistence are complete.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SetWindowPos, ShowWindow, HWND_TOP, SWP_NOZORDER, SW_SHOW,
    WS_CHILD, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::PCWSTR;

use crate::icons;
use crate::ui::xaml_gen::{
    GridEntry, GridModel, GridSection, SectionRow, SectionedModel, build_grid, build_sectioned,
};

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
pub struct UiHost {
    #[allow(dead_code)]
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
}

impl UiHost {
    /// Load config via `crate::storage::load`, create Win32 child controls
    /// parented to `parent_hwnd`.
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

        let icon_size = config.default_settings.icon_size;
        let icon_dirs = icons::icons_dirs();

        if config.default_settings.show_categories {
            // ----------------------------------------------------------------
            // GROUPED PATH (show_categories == true)
            // ----------------------------------------------------------------
            let groups = crate::storage::group_commands_by_category(&config);

            // Map groups to GridSections; synthetic Uncategorized uses a fixed label.
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

            // Create STATIC header controls and BUTTON controls.
            let mut buttons: Vec<HWND> = Vec::new();
            let mut headers: Vec<HWND> = Vec::new();
            let mut bitmaps: Vec<HBITMAP> = Vec::new();

            // Flatten all command entries in section-row order for button creation.
            // We need to create buttons in the exact same order that layout_children
            // will iterate them (row-by-row across sections).
            for row in &sectioned.rows {
                match row {
                    SectionRow::Header { label, .. } => {
                        // Create a STATIC control for the section header.
                        let label_w: Vec<u16> =
                            label.encode_utf16().chain(std::iter::once(0)).collect();
                        let class_w: Vec<u16> = "STATIC\0".encode_utf16().collect();

                        let hwnd_hdr = unsafe {
                            CreateWindowExW(
                                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                                PCWSTR(class_w.as_ptr()),
                                PCWSTR(label_w.as_ptr()),
                                WS_CHILD | WS_VISIBLE,
                                0,
                                0,
                                0,
                                0,
                                parent_hwnd,
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
                            let hwnd_btn =
                                create_button(parent_hwnd, cell, icon_size, &icon_dirs, &mut bitmaps);
                            if let Some(btn) = hwnd_btn {
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

            let mut host = UiHost {
                parent: parent_hwnd,
                buttons,
                headers,
                bitmaps,
                mode: LayoutMode::Grouped { model: sectioned },
            };
            host.layout_children(800, 600);
            Ok(host)
        } else {
            // ----------------------------------------------------------------
            // FLAT PATH (show_categories == false) — issue #1/#5 unchanged
            // ----------------------------------------------------------------
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

            let mut buttons: Vec<HWND> = Vec::with_capacity(grid.cells.len());
            let mut bitmaps: Vec<HBITMAP> = Vec::new();

            for cell in &grid.cells {
                if let Some(btn) = create_button(parent_hwnd, cell, icon_size, &icon_dirs, &mut bitmaps) {
                    buttons.push(btn);
                }
            }

            let mut host = UiHost {
                parent: parent_hwnd,
                buttons,
                headers: Vec::new(),
                bitmaps,
                mode: LayoutMode::Flat { grid },
            };
            host.layout_children(800, 600);
            Ok(host)
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
    /// In flat mode: positions buttons in a uniform grid.
    /// In grouped mode: stacks header rows and button rows vertically.
    pub fn layout_children(&mut self, width: u32, height: u32) {
        match &self.mode {
            LayoutMode::Flat { grid } => {
                layout_flat(grid, &self.buttons, width, height);
            }
            LayoutMode::Grouped { model } => {
                // We need a snapshot because we hold &self.mode (immutable) while
                // iterating self.headers and self.buttons (also behind &self).
                // Collect row descriptors first, then call SetWindowPos.
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
                    width,
                    height,
                    model.cols,
                );
            }
        }
    }
}

impl Drop for UiHost {
    /// Free all GDI bitmap handles on drop (AC3: no handle leak on exit).
    fn drop(&mut self) {
        self.clear_bitmaps();
        log::debug!("UiHost dropped: all GDI handles freed");
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Create a BUTTON child control for a grid cell, applying icon if resolved.
/// Returns `Some(HWND)` on success or `None` when `CreateWindowExW` fails.
fn create_button(
    parent_hwnd: HWND,
    cell: &crate::ui::xaml_gen::GridCell,
    icon_size: u32,
    icon_dirs: &[std::path::PathBuf],
    bitmaps: &mut Vec<HBITMAP>,
) -> Option<HWND> {
    let resolution = icons::resolve_icon(&cell.icon, icon_dirs);

    let button_label = match &resolution {
        icons::IconResolution::EmojiFallback(text) if !text.is_empty() => text.clone(),
        _ => cell.label.clone(),
    };

    let label_w: Vec<u16> = button_label
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let class_w: Vec<u16> = "BUTTON\0".encode_utf16().collect();

    let hwnd_btn = unsafe {
        CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(label_w.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            0,
            0,
            0,
            0,
            parent_hwnd,
            None,
            None,
            None,
        )
    };

    if hwnd_btn.0 == 0 {
        log::warn!("CreateWindowExW returned null for button '{}'", cell.label);
        return None;
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
fn layout_flat(grid: &GridModel, buttons: &[HWND], width: u32, height: u32) {
    let cols = grid.cols.max(1);
    let rows = grid.row_count().max(1);

    const PAD: u32 = 8;
    let avail_w = width.saturating_sub(PAD * (cols + 1));
    let avail_h = height.saturating_sub(PAD * (rows + 1));
    let cell_w = (avail_w / cols).max(1);
    let cell_h = (avail_h / rows).max(1);

    for (btn_hwnd, cell) in buttons.iter().zip(grid.cells.iter()) {
        let x = (PAD + cell.col * (cell_w + PAD)) as i32;
        let y = (PAD + cell.row * (cell_h + PAD)) as i32;

        let _ = unsafe {
            SetWindowPos(*btn_hwnd, HWND_TOP, x, y, cell_w as i32, cell_h as i32, SWP_NOZORDER)
        };
    }
}

/// Grouped layout: stacks header rows and button rows vertically.
/// SetWindowPos-only (no bitmap creation — preserves issue #5 AC3 leak-safety).
///
/// `rows_snapshot` is `(is_header, cell_count_in_row)` for each SectionRow.
fn layout_grouped(
    rows_snapshot: &[(bool, usize)],
    headers: &[HWND],
    buttons: &[HWND],
    width: u32,
    _height: u32,
    cols: u32,
) {
    const PAD: u32 = 8;
    const HEADER_H: u32 = 24; // fixed height for STATIC header controls
    const CELL_H: u32 = 80;   // fixed height for BUTTON cells
    let cols = cols.max(1);
    let avail_w = width.saturating_sub(PAD * (cols + 1));
    let cell_w = (avail_w / cols).max(1);

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
                            CELL_H as i32,
                            SWP_NOZORDER,
                        )
                    };
                    button_idx += 1;
                }
            }
            y += (CELL_H + PAD) as i32;
        }
    }
}

// ---------------------------------------------------------------------------
// Deferred call seam (issue #2 — click-binding deferred to issue #11)
// ---------------------------------------------------------------------------

/// Thin pass-through from the UI layer to the process executor.
///
/// Parses `command` and spawns it with `CREATE_NO_WINDOW`. Exposed here so
/// a later issue can bind it to button-click (`WM_COMMAND`) handling without
/// any architectural change.
///
/// The `command` field comes from [`crate::storage::Command::command`].
///
/// # Errors
/// Returns [`crate::windows::process::LaunchError`] on parse failure or
/// spawn error — the caller can surface the message via `Display`.
#[allow(dead_code)] // Remove this attribute when issue #11 wires click handling.
pub fn launch_command(
    command: &str,
) -> Result<crate::windows::process::LaunchOutcome, crate::windows::process::LaunchError> {
    crate::windows::process::launch(command)
}
