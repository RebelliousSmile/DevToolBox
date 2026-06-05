//! WinFXStart - Windows 11 Command Launcher
//!
//! Native Rust application using tao for windowing and Win32 child controls
//! for the command-grid UI.

mod storage;
mod ui;
mod windows;

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;

fn main() {
    env_logger::init();

    log::info!("WinFXStart v0.1.0 starting");

    // Best-effort boot sync: align the registry startup entry with config.
    match storage::load() {
        Ok(cfg) => {
            if let Err(e) =
                windows::registry::sync_startup(cfg.default_settings.launch_at_startup)
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
        .with_title("WinFXStart - Command Launcher")
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
