//! Application state: settings + command list loaded from `crate::storage`,
//! and the Win32 child-control host that renders them as a native button grid.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SetWindowPos, ShowWindow, HWND_TOP, SWP_NOZORDER, SW_SHOW,
    WS_CHILD, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::PCWSTR;

use crate::ui::xaml_gen::{build_grid, GridModel};

// ---------------------------------------------------------------------------
// Host state
// ---------------------------------------------------------------------------

/// Owns the parent HWND and the spawned child-button handles.
pub struct UiHost {
    #[allow(dead_code)]
    pub parent: HWND,
    pub buttons: Vec<HWND>,
    pub grid: GridModel,
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

        let favorites: Vec<String> = config
            .commands
            .iter()
            .filter(|c| c.is_favorite)
            .map(|c| c.name.clone())
            .collect();

        log::info!("Loaded {} favorite commands", favorites.len());

        // Build the grid model (3 columns preferred).
        let grid = build_grid(&favorites, 3);

        // Create one Win32 BUTTON per cell.
        let mut buttons: Vec<HWND> = Vec::with_capacity(grid.cells.len());

        for cell in &grid.cells {
            // Convert label to null-terminated UTF-16.
            let label_w: Vec<u16> = cell
                .label
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
                    None,  // no menu / child id needed for this placeholder
                    None,  // hinstance — None lets Windows use the process instance
                    None,
                )
            };

            if hwnd_btn.0 == 0 {
                log::warn!("CreateWindowExW returned null for button '{}'", cell.label);
            } else {
                unsafe { ShowWindow(hwnd_btn, SW_SHOW) };
                buttons.push(hwnd_btn);
            }
        }

        let mut host = UiHost {
            parent: parent_hwnd,
            buttons,
            grid,
        };

        // Perform initial layout with a default size — the first Resized event
        // will correct it with the real dimensions.
        host.layout_children(800, 600);
        Ok(host)
    }

    /// Re-position child buttons to fill the parent client area as a grid.
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
