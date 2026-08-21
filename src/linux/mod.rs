//! `linux` module — native Linux desktop integrations.
//!
//! Groups platform-specific helpers with no Windows equivalent: XDG
//! autostart (Part 3 Phase 1), freedesktop icon-theme lookup and systemd
//! Automations parsing (Part 3 Phase 2).
//!
//! The entire module body is gated on `#[cfg(target_os = "linux")]`
//! (module-level, via this inner attribute), mirroring the pattern already
//! used by `crate::windows` (see `src/windows/mod.rs`) — `mod linux;` in
//! `main.rs` is declared unconditionally, and this attribute makes the file
//! compile to nothing on non-Linux targets.
#![cfg(target_os = "linux")]

pub mod automations;
pub mod autostart;
pub mod icon_theme;
