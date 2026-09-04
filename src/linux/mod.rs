//! `linux` module — native Linux desktop integrations.
//!
//! Groups platform-specific helpers with no Windows equivalent: XDG
//! autostart (Part 3 Phase 1), freedesktop icon-theme lookup and systemd
//! Automations parsing (Part 3 Phase 2).
//!
//! The entire module body is gated by the `#[cfg(target_os = "linux")]`
//! declaration in `main.rs` (`mod linux;`), mirroring the pattern already
//! used by `crate::windows` (see `src/windows/mod.rs`) — this file compiles
//! to nothing on non-Linux targets.

pub mod automations;
pub mod autostart;
pub mod icon_theme;
