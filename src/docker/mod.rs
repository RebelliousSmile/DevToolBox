//! `docker` module — the Docker CLI bridge, on every OS.
//!
//! Unlike `crate::linux` and `crate::windows`, this module carries **no**
//! `cfg(target_os = ...)` gate: everything under it talks to Docker by
//! spawning the `docker` executable and parsing its output, which is the
//! same contract on Linux, Windows (Docker Desktop over
//! `npipe:////./pipe/dockerDesktopLinuxEngine`) and macOS. The few genuinely
//! OS-specific details — the `.exe` suffix when resolving the binary on
//! `PATH`, the `CREATE_NO_WINDOW` creation flag that keeps a console window
//! from flashing on Windows — are handled inside [`engine`], at the exact
//! point where they differ, rather than by duplicating a module per OS.
//!
//! Both submodules lived under `crate::linux` until the Windows port: the
//! backend was written and validated on the Linux reference machine, and the
//! `#![cfg(target_os = "linux")]` on `crate::linux` meant a Windows build
//! contained none of it, so `crate::ui::docker_view::available()` returned
//! `false` in hard code and the Docker tab was never drawn.
//!
//! The split between the two submodules mirrors the two CLIs:
//!
//! - [`engine`] — `docker ps` / `images` / `volume ls` / `inspect` /
//!   `system df`, plus the stop/rm/rmi actions.
//! - [`compose`] — `docker compose`, which is a separate CLI plugin that
//!   can be absent even when the daemon is healthy.
//! - [`compose_edit`] — the one path that *writes* to a compose file, to
//!   apply a port reassignment plan.
//!
//! None of them knows anything about the UI's state: they return the row types
//! declared in `crate::ui::docker_view` / `crate::ui::compose_view` and let
//! the views own presentation.

pub mod compose;
pub mod compose_edit;
pub mod engine;
