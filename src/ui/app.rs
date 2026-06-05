//! Application state: settings + command list loaded from `config/default.json`,
//! and the Win32 child-control host that renders them as a native button grid.

use std::path::Path;

use serde::Deserialize;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SetWindowPos, ShowWindow, HWND_TOP, SWP_NOZORDER, SW_SHOW,
    WS_CHILD, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::PCWSTR;

use crate::ui::xaml_gen::{build_grid, GridModel};

// ---------------------------------------------------------------------------
// JSON config structures
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub commands: Vec<CommandEntry>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CommandEntry {
    #[allow(dead_code)]
    pub id: String,
    pub name: String,
    #[allow(dead_code)]
    pub command: String,
    #[serde(default)]
    pub is_favorite: bool,
}

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
    /// Load config, create Win32 BUTTON children parented to `parent_hwnd`.
    pub fn new(parent_hwnd: HWND) -> Result<Self, Box<dyn std::error::Error>> {
        let config = load_config()?;
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
// Config loader
// ---------------------------------------------------------------------------

fn load_config() -> Result<AppConfig, Box<dyn std::error::Error>> {
    // Try the path relative to the executable first, then relative to cwd.
    let candidates = [
        Path::new("config/default.json").to_path_buf(),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("config/default.json")))
            .unwrap_or_default(),
    ];

    for path in &candidates {
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            let cfg: AppConfig = serde_json::from_str(&text)?;
            log::info!("Config loaded from {}", path.display());
            return Ok(cfg);
        }
    }

    // Fallback: return a hard-coded minimal config so the app still opens.
    log::warn!("config/default.json not found — using built-in fallback");
    Ok(AppConfig {
        commands: vec![
            CommandEntry {
                id: "notepad".into(),
                name: "Bloc-notes".into(),
                command: "notepad.exe".into(),
                is_favorite: true,
            },
            CommandEntry {
                id: "cmd".into(),
                name: "Invite de commandes".into(),
                command: "cmd.exe".into(),
                is_favorite: true,
            },
            CommandEntry {
                id: "ipconfig".into(),
                name: "Adresse IP".into(),
                command: "ipconfig /all".into(),
                is_favorite: true,
            },
        ],
    })
}
