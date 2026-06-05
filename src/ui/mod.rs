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
            log::debug!(
                "on_resize: hwnd={:?} {}x{}",
                hwnd,
                width,
                height
            );
            host.layout_children(width, height);
        }
    });
}
