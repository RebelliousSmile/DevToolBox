//! Icon resolution and raster decode/resize pipeline.
//!
//! # Module layout
//!
//! - `resolve`      — pure, GDI-free: maps an `icon: String` value to either
//!   an on-disk image path or an emoji/text fallback (`IconResolution`).
//! - `decode`       — pure, GDI-free: decodes a raster image from bytes and
//!   resizes it to a fixed square, returning an RGBA pixel buffer.
//! - `backend`      — render-agnostic `IconBackend` trait: converts a
//!   `DecodedIcon` into a renderer-specific handle.
//! - `egui_backend` — `IconBackend` impl for `egui` (`ColorImage` /
//!   `TextureHandle`). Shared by both OSes; superseded and replaced the
//!   Win32-only `gdi.rs` (`HBITMAP` bridge), which was deleted in Part 2
//!   Phase 4 once its last caller (`src/ui/app.rs`/`card.rs`) was deleted
//!   alongside it.
//!
//! # SVG
//!
//! SVG is explicitly descoped (Decision D1). `.svg` paths resolve to
//! `EmojiFallback` and never reach the decode layer. A future SVG rasterizer
//! can feed the same `DecodedIcon` type into any `IconBackend` impl.

pub mod backend;
pub mod decode;
pub mod egui_backend;
pub mod resolve;

#[allow(unused_imports)]
pub use backend::IconBackend;
#[allow(unused_imports)]
pub use decode::{decode_resize_file, decode_resize_rgba, DecodeError, DecodedIcon};
#[allow(unused_imports)]
pub use egui_backend::EguiIconBackend;
#[allow(unused_imports)]
pub use resolve::{icons_dirs, resolve_icon, IconResolution};
