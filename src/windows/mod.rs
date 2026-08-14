//! `windows` module — native Windows API integrations.
//!
//! This module groups platform-specific helpers that wrap or complement
//! the `windows` crate. The sub-module name `windows` is an in-crate path
//! (`crate::windows::...`); the extern `windows` crate is reached via its
//! own extern-prelude path (`windows::Win32::...`) — no ambiguity at the
//! compiler level.
//!
//! The entire module body is gated on `#[cfg(windows)]` (module-level, via
//! this inner attribute — equivalent to attaching `#[cfg(windows)]` to the
//! `mod windows;` declaration in `main.rs`, without having to touch that
//! declaration). The `windows` crate and its `raw-window-handle` companion
//! are only present in the dependency graph under
//! `[target.'cfg(windows)'.dependencies]` (see `Cargo.toml`), so on
//! non-Windows targets this file must compile to nothing.
#![cfg(windows)]

pub mod process;
pub mod registry;
