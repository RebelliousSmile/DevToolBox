//! `windows` module — native Windows API integrations.
//!
//! This module groups platform-specific helpers that wrap or complement
//! the `windows` crate. The sub-module name `windows` is an in-crate path
//! (`crate::windows::...`); the extern `windows` crate is reached via its
//! own extern-prelude path (`windows::Win32::...`) — no ambiguity at the
//! compiler level.

pub mod process;
pub mod registry;
