#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! DevToolBox - Windows 11 / Linux Command Launcher
//!
//! Cross-platform Rust application using `eframe`/`egui` for the UI (Part 2
//! of the multi-OS transformation). Phase 1 wires up a minimal `eframe::App`
//! bootstrap; the full card grid lands in Phase 2.

mod icons;
mod applications;
mod linux;
mod platform;
mod storage;
mod ui;
mod windows;

use eframe::egui;

use ui::egui_app::EguiApp;

struct FlushFile(std::fs::File);

impl std::io::Write for FlushFile {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = std::io::Write::write(&mut self.0, buffer)?;
        std::io::Write::flush(&mut self.0)?;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.0)
    }
}

fn init_logging() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let directory = base.join("DevToolBox");
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join("devtoolbox.log");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    use std::io::Write as _;
    let _ = writeln!(
        file,
        "\n--- DevToolBox bootstrap pid={} ---",
        std::process::id()
    );
    let _ = file.flush();
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    builder
        .filter_level(log::LevelFilter::Debug)
        .format_timestamp_millis()
        .target(env_logger::Target::Pipe(Box::new(FlushFile(file))))
        .init();
    Some(path)
}

fn main() {
    let log_path = init_logging();
    std::panic::set_hook(Box::new(|info| {
        log::error!(
            "panic: {info}\nbacktrace:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
    }));

    log::info!(
        "DevToolBox v0.1.0 starting; pid={}; log={:?}",
        std::process::id(),
        log_path
    );

    // Best-effort boot sync: align the registry startup entry with config.
    match storage::load() {
        Ok(cfg) => {
            if let Err(e) = platform::sync_startup(cfg.default_settings.launch_at_startup) {
                log::warn!("boot sync_startup failed: {}", e);
            }
        }
        Err(e) => {
            log::warn!("could not load config for boot sync: {}", e);
        }
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DevToolBox - Actions Windows")
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([400.0, 300.0]),
        ..Default::default()
    };

    let run_result = eframe::run_native(
        "DevToolBox",
        native_options,
        Box::new(|cc| Ok(Box::new(EguiApp::new(cc)))),
    );

    match run_result {
        Ok(()) => log::info!("eframe event loop exited cleanly"),
        Err(e) => log::error!("eframe::run_native returned an error: {e}"),
    }
}
