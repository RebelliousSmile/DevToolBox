#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! WinFXStart - Windows 11 Command Launcher
//!
//! Native Rust application using tao for windowing and Win32 child controls
//! for the command-grid UI.

mod icons;
mod storage;
mod ui;
mod windows;

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;

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
    let directory = base.join("WinFXStart");
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join("winfxstart.log");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    use std::io::Write as _;
    let _ = writeln!(
        file,
        "\n--- WinFXStart bootstrap pid={} ---",
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

    log::info!(
        "WinFXStart v0.1.0 starting; pid={}; log={:?}",
        std::process::id(),
        log_path
    );

    // Best-effort boot sync: align the registry startup entry with config.
    match storage::load() {
        Ok(cfg) => {
            if let Err(e) = windows::registry::sync_startup(cfg.default_settings.launch_at_startup)
            {
                log::warn!("boot sync_startup failed: {}", e);
            }
        }
        Err(e) => {
            log::warn!("could not load config for boot sync: {}", e);
        }
    }

    let event_loop = EventLoop::new();

    let window = WindowBuilder::new()
        .with_title("WinFXStart - Actions Windows")
        .with_inner_size(tao::dpi::LogicalSize::new(800u32, 600u32))
        .with_min_inner_size(tao::dpi::LogicalSize::new(400u32, 300u32))
        .build(&event_loop)
        .expect("Failed to create window");

    // Obtain the Win32 HWND from the tao window via raw-window-handle 0.6.
    let hwnd = ui::hwnd_from_window(&window);
    log::info!("HWND obtained: {:?}", hwnd);

    // Initialise the native UI host with the parent HWND.
    ui::host_init(hwnd).expect("Failed to initialise UI host");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                log::info!("Close requested — exiting event loop");
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                let hwnd = ui::hwnd_from_window(&window);
                ui::on_resize(hwnd, size.width, size.height);
            }
            _ => {}
        }
    });
}
