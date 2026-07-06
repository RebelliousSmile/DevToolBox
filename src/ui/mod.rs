//! `ui` module — native Win32 UI host.
//!
//! Public entry points called from `main.rs`:
//! - [`hwnd_from_window`] — extract the Win32 HWND from a `tao::window::Window`.
//! - [`host_init`]        — create child controls; must be called once after the window is built.
//! - [`on_resize`]        — re-layout child controls when the window is resized.

pub mod app;
pub mod card;
pub mod xaml_gen;

use std::cell::RefCell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tao::window::Window;
use windows::Win32::Foundation::{HWND, LRESULT};
use windows::Win32::Graphics::Gdi::{SetBkMode, SetTextColor, TRANSPARENT};
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;

use app::UiHost;

// Thread-local storage for the UI host (single-threaded Win32 UI).
thread_local! {
    static HOST: RefCell<Option<UiHost>> = const { RefCell::new(None) };
}

fn with_detached<T, R>(
    cell: &RefCell<Option<T>>,
    operation: impl FnOnce(&mut T) -> R,
) -> Option<R> {
    let mut value = cell.borrow_mut().take()?;
    let result = catch_unwind(AssertUnwindSafe(|| operation(&mut value)));
    *cell.borrow_mut() = Some(value);
    match result {
        Ok(value) => Some(value),
        Err(payload) => resume_unwind(payload),
    }
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
pub fn handle_command(control_id: u16, source_hwnd: HWND) {
    HOST.with(|cell| {
        let handled = with_detached(cell, |host| host.handle_action(control_id, source_hwnd))
        .unwrap_or(false);
        if !handled {
            log::warn!("handle_command: no command found for control_id={control_id}");
        }
    });
}


pub fn handle_action_variant_menu(menu_id: u16) -> bool {
    HOST.with(|cell| {
        with_detached(cell, |host| host.handle_action_variant_menu(menu_id)).unwrap_or(false)
    })
}

pub fn switch_view(control_id: u16) {
    HOST.with(|cell| {
        with_detached(cell, |host| {
            host.switch_view_control(control_id);
        });
    });
}

/// Dispatch a menu-bar command id to the host.
///
/// Called from `subclass_proc` on `WM_COMMAND` menu selections.
pub fn handle_menu(id: u16) {
    HOST.with(|cell| {
        with_detached(cell, |host| {
            if !host.handle_menu_command(id) {
                log::warn!("handle_menu: unrecognized menu id={id}");
            }
        });
    });
}

pub fn poll_action_events() {
    HOST.with(|cell| {
        with_detached(cell, |host| {
            host.poll_events();
        });
    });
}

pub fn handle_automation(control_id: u16) {
    HOST.with(|cell| {
        with_detached(cell, |host| {
            host.handle_automation_control(control_id);
        });
    });
}

/// Handle `WM_CTLCOLORSTATIC` for a category section header (Phase 4).
///
/// Called from `container_proc` in `app.rs` when Windows asks for the text
/// and background colors for a STATIC child control (our category headers).
///
/// Sets a Fluent-style eyebrow text color (dark-gray, near-black) with
/// transparent background (matched to the container's white surface), and
/// returns the host's cached `header_bg_brush` as the background brush.
///
/// Returns `Some(LRESULT)` with the brush handle if the host is available,
/// or `None` to fall back to `DefWindowProcW` (default STATIC styling).
///
/// # Safety
/// `hdc` must be a valid Win32 device context for the duration of the call.
pub unsafe fn ctlcolor_static_for_container(
    hdc: windows::Win32::Graphics::Gdi::HDC,
) -> Option<LRESULT> {
    HOST.with(|cell| {
        if let Some(host) = cell.borrow().as_ref() {
            if host.header_bg_brush.0 != 0 {
                // Eyebrow style: Fluent dark-gray text (#505050), transparent bg.
                SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x00505050));
                SetBkMode(hdc, TRANSPARENT);
                return Some(LRESULT(host.header_bg_brush.0));
            }
        }
        None
    })
}

/// Update the hover state for a button identified by its control id.
///
/// Called from the per-button hover subclass proc (`hover_subclass_proc` in
/// `app.rs`) on `WM_MOUSEMOVE` (set `hot = true`) and `WM_MOUSELEAVE`
/// (set `hot = false`).
///
/// Returns `true` if the state actually changed (caller should then
/// `InvalidateRect` to repaint); `false` if the value was already the same
/// (avoids repaint storms on repeated `WM_MOUSEMOVE` while already hot).
///
/// # Reentrancy note
///
/// `WM_MOUSEMOVE` / `WM_MOUSELEAVE` are never delivered while a shared
/// `borrow()` from `draw_item_for_container` is held — `InvalidateRect`
/// only posts `WM_PAINT` which leads to `WM_DRAWITEM` later, after the
/// current borrow is released.  So `borrow_mut()` here is always safe.
pub fn set_hover(ctrl_id: u16, hot: bool) -> bool {
    HOST.with(|cell| {
        if let Some(host) = cell.borrow_mut().as_mut() {
            let current = host.id_to_hover.get(&ctrl_id).copied().unwrap_or(false);
            if current != hot {
                host.id_to_hover.insert(ctrl_id, hot);
                return true;
            }
        }
        false
    })
}

/// Dispatch a `WM_DRAWITEM` paint request to the host's placeholder paint
/// routine.
///
/// Called from `container_proc` in `app.rs` when `WM_DRAWITEM` arrives for
/// an action button.  Uses a shared borrow so re-entrant calls from nested
/// message dispatches do not trigger a `BorrowMutError`.
///
/// # Safety
/// `dis` must be a valid pointer to a `DRAWITEMSTRUCT` provided by Windows
/// for the duration of the `WM_DRAWITEM` message.
pub unsafe fn draw_item_for_container(dis: *const DRAWITEMSTRUCT) {
    HOST.with(|cell| {
        if let Some(host) = cell.borrow().as_ref() {
            // Safety: caller guarantees dis is valid.
            host.draw_item(dis);
        }
    });
}

#[cfg(test)]
mod reentrancy_tests {
    use super::with_detached;
    use std::cell::RefCell;

    #[test]
    fn detached_value_allows_reentrant_borrow() {
        let cell = RefCell::new(Some(41));
        let result = with_detached(&cell, |value| {
            assert!(cell.borrow().is_none());
            *value += 1;
            *value
        });
        assert_eq!(result, Some(42));
        assert_eq!(*cell.borrow(), Some(42));
    }

    #[test]
    fn detached_value_is_restored_after_panic() {
        let cell = RefCell::new(Some(7));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_detached(&cell, |_value| panic!("test panic"));
        }));
        assert_eq!(*cell.borrow(), Some(7));
    }
}
