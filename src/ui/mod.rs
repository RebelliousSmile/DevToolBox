//! `ui` module — cross-platform `egui`/`eframe` UI (Part 2).
//!
//! The former native Win32 UI host ([`app`], [`card`], plus the
//! `xaml_gen`-fed grid-model layer it consumed) has been deleted (Part 2
//! Phase 4) now that [`egui_app`] reaches feature parity with it (Phase
//! 2/3's interaction checklist, kept in [`egui_app`]'s module doc comment).
//! `egui_app` is the UI entry point, wired up from `main.rs` via
//! `eframe::run_native`. It does not need a raw window handle or a
//! host-init step — `eframe` manages its own window/handle plumbing — so
//! the former `hwnd_from_window`/`host_init` functions (which extracted a
//! Win32 `HWND` from a `tao` window) were already removed as part of the
//! eframe bootstrap swap (Part 2 Phase 1), and the `HWND`-keyed dispatch
//! functions that used to route Win32 window messages
//! (`on_resize`/`handle_command`/`handle_menu`/etc., all `#[cfg(windows)]`
//! and backed by a thread-local `UiHost`) went with `app.rs`/`card.rs` in
//! this phase, since nothing outside those two files ever called them.
//! `xaml_gen.rs` itself (the WinUI 3 grid-model seam, Decision D1) had no
//! remaining consumer once `app.rs` was gone, so it was deleted too rather
//! than left as dead code.

pub mod applications_view;
pub mod automations_view;
pub mod cleanup_view;
pub mod command_form;
pub mod compose_view;
pub mod dialogs;
pub mod docker_view;
pub mod egui_app;
pub mod fonts;
pub mod format;
pub mod icon_picker;
pub mod ports;
pub mod terminal_view;
