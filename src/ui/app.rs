//! Application state: settings + command list loaded from `crate::storage`,
//! and the Win32 child-control host that renders them as a native button grid.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SetWindowPos, ShowWindow, HWND_TOP, SWP_NOZORDER, SW_SHOW,
    WS_CHILD, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::PCWSTR;

use crate::icons;
use crate::ui::xaml_gen::{GridEntry, GridModel, build_grid};

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
pub struct UiHost {
    #[allow(dead_code)]
    pub parent: HWND,
    pub buttons: Vec<HWND>,
    pub grid: GridModel,
    /// GDI bitmap handles owned by this host.  Freed in `clear_bitmaps` /
    /// `Drop`.  Never modified by `layout_children`.
    pub bitmaps: Vec<HBITMAP>,
}

impl UiHost {
    /// Load config via `crate::storage::load`, create Win32 BUTTON children
    /// parented to `parent_hwnd`.
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

        // Build grid entries from favorite commands, carrying icon + id.
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

        log::info!("Loaded {} favorite commands", entries.len());

        // Build the grid model (3 columns preferred).
        let grid = build_grid(&entries, 3);

        // Create one Win32 BUTTON per cell and apply icons where available.
        let mut buttons: Vec<HWND> = Vec::with_capacity(grid.cells.len());
        let mut bitmaps: Vec<HBITMAP> = Vec::new();

        for cell in &grid.cells {
            // Resolve the icon string: returns Image(path) or EmojiFallback.
            let resolution = icons::resolve_icon(&cell.icon, &icon_dirs);

            // Choose the button label: emoji fallback uses the icon string (or
            // the command name if icon is empty).
            let button_label = match &resolution {
                icons::IconResolution::EmojiFallback(text) if !text.is_empty() => text.clone(),
                _ => cell.label.clone(),
            };

            // Convert label to null-terminated UTF-16.
            let label_w: Vec<u16> = button_label
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            // BUTTON class name (UTF-16).
            let class_w: Vec<u16> = "BUTTON\0".encode_utf16().collect();

            // Safety: CreateWindowExW is unsafe; parent HWND is valid for the
            // duration of the event loop.
            let hwnd_btn = unsafe {
                CreateWindowExW(
                    windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                    PCWSTR(class_w.as_ptr()),
                    PCWSTR(label_w.as_ptr()),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                    0, // x — positioned later via layout_children
                    0, // y
                    0, // width
                    0, // height
                    parent_hwnd,
                    None,  // no menu / child id
                    None,  // hinstance — None lets Windows use the process instance
                    None,
                )
            };

            if hwnd_btn.0 == 0 {
                log::warn!("CreateWindowExW returned null for button '{}'", cell.label);
                continue;
            }

            unsafe { ShowWindow(hwnd_btn, SW_SHOW) };

            // Assign image bitmap when the icon resolves to an existing file.
            if let icons::IconResolution::Image(path) = &resolution {
                match icons::decode_resize_file(path, icon_size) {
                    Ok(decoded) => {
                        match icons::gdi::rgba_to_hbitmap(&decoded) {
                            Ok(hbitmap) => {
                                icons::gdi::set_button_bitmap(hwnd_btn, hbitmap);
                                bitmaps.push(hbitmap);
                                log::info!(
                                    "Icon applied to '{}': {:?}",
                                    cell.label,
                                    path
                                );
                            }
                            Err(e) => {
                                log::warn!(
                                    "rgba_to_hbitmap failed for '{}': {e}; using text label",
                                    cell.label
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "decode_resize_rgba failed for '{}': {e}; using text label",
                            cell.label
                        );
                    }
                }
            }

            buttons.push(hwnd_btn);
        }

        let mut host = UiHost {
            parent: parent_hwnd,
            buttons,
            grid,
            bitmaps,
        };

        // Perform initial layout with a default size — the first Resized event
        // will correct it with the real dimensions.
        host.layout_children(800, 600);
        Ok(host)
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

    /// Re-position child buttons to fill the parent client area as a grid.
    ///
    /// This method only calls `SetWindowPos` — it NEVER creates bitmaps.
    /// This is the guarantee that repeated resizes cannot leak GDI handles.
    pub fn layout_children(&mut self, width: u32, height: u32) {
        let cols = self.grid.cols.max(1);
        let rows = self.grid.row_count().max(1);

        // Padding around and between cells.
        const PAD: u32 = 8;
        let avail_w = width.saturating_sub(PAD * (cols + 1));
        let avail_h = height.saturating_sub(PAD * (rows + 1));
        let cell_w = (avail_w / cols).max(1);
        let cell_h = (avail_h / rows).max(1);

        for (btn_hwnd, cell) in self.buttons.iter().zip(self.grid.cells.iter()) {
            let x = (PAD + cell.col * (cell_w + PAD)) as i32;
            let y = (PAD + cell.row * (cell_h + PAD)) as i32;

            let _ = unsafe {
                SetWindowPos(
                    *btn_hwnd,
                    HWND_TOP,
                    x,
                    y,
                    cell_w as i32,
                    cell_h as i32,
                    SWP_NOZORDER,
                )
            };
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
