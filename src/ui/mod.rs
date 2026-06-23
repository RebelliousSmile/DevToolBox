//! `ui` module — native Win32 UI host.
//!
//! Public entry points called from `main.rs`:
//! - [`hwnd_from_window`] — extract the Win32 HWND from a `tao::window::Window`.
//! - [`host_init`]        — create child controls; must be called once after the window is built.
//! - [`on_resize`]        — re-layout child controls when the window is resized.

pub mod app;
pub mod xaml_gen;

use std::cell::RefCell;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tao::window::Window;
use windows::Win32::Foundation::HWND;

use app::UiHost;

// Thread-local storage for the UI host (single-threaded Win32 UI).
thread_local! {
    static HOST: RefCell<Option<UiHost>> = const { RefCell::new(None) };
}

/// Extract the Win32 HWND from a `tao::window::Window` using raw-window-handle 0.6.
///
/// # Panics
/// Panics if the window handle is not a Win32 handle (impossible on Windows).
pub fn hwnd_from_window(window: &Window) -> HWND {
    let handle = window
        .window_handle()
        .expect("Failed to obtain window handle");

    match handle.as_raw() {
        RawWindowHandle::Win32(h) => {
            // NonZeroIsize.get() → isize; cast to the windows crate's HWND (isize wrapper).
            HWND(h.hwnd.get())
        }
        other => panic!("Expected a Win32 window handle, got {:?}", other),
    }
}

/// Initialise the UI host: create Win32 child controls parented to `hwnd`.
///
/// Must be called exactly once, after the tao window is created.
pub fn host_init(hwnd: HWND) -> Result<(), Box<dyn std::error::Error>> {
    let host = UiHost::new(hwnd)?;
    HOST.with(|cell| {
        *cell.borrow_mut() = Some(host);
    });
    log::info!("UI host initialised");
    Ok(())
}

/// Re-layout child controls to match the new window dimensions.
///
/// Called from `main.rs` on every `WindowEvent::Resized`.
pub fn on_resize(hwnd: HWND, width: u32, height: u32) {
    HOST.with(|cell| {
        if let Some(host) = cell.borrow_mut().as_mut() {
            log::debug!("on_resize: hwnd={:?} {}x{}", hwnd, width, height);
            host.layout_children(width, height);
        }
    });
}

/// Resolve a clicked button's control id to its command string and launch it.
///
/// Called from the `subclass_proc` in `app.rs` on every `WM_COMMAND /
/// BN_CLICKED` event.  Uses a short-lived `borrow()` (not `borrow_mut`) to
/// avoid re-entrant `BorrowMutError` (Decision D2).
///
/// Errors from `launch_command` are logged at `warn` level; the app keeps
/// running (Decision D7 — feedback is issue #11).
pub fn handle_command(control_id: u16) {
    HOST.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        if let Some(host) = borrowed.as_mut() {
            if let Some(cmd_str) = host.id_to_command.get(&control_id) {
                let cmd_str = cmd_str.clone();
                log::info!("handle_command: launching control_id={control_id} cmd='{cmd_str}'");
                host.launch_action(&cmd_str);
                return;
            }
        }
        drop(borrowed);
        log::warn!("handle_command: no command found for control_id={control_id}");
    });
}

pub fn switch_view(control_id: u16) {
    HOST.with(|cell| {
        if let Some(host) = cell.borrow_mut().as_mut() {
            host.switch_view_control(control_id);
        }
    });
}

/// Dispatch a menu-bar command id to the host.
///
/// Called from `subclass_proc` on `WM_COMMAND` menu selections.
pub fn handle_menu(id: u16) {
    HOST.with(|cell| {
        if let Some(host) = cell.borrow_mut().as_mut() {
            if !host.handle_menu_command(id) {
                log::warn!("handle_menu: unrecognized menu id={id}");
            }
        }
    });
}

pub fn poll_action_events() {
    HOST.with(|cell| {
        if let Some(host) = cell.borrow_mut().as_mut() {
            host.poll_events();
        }
    });
}
