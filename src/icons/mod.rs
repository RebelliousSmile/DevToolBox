//! Icon resolution and raster decode/resize pipeline.
//!
//! # Module layout
//!
//! - `resolve`  — pure, GDI-free: maps an `icon: String` value to either an
//!   on-disk image path or an emoji/text fallback (`IconResolution`).
//! - `decode`   — pure, GDI-free: decodes a raster image from bytes and
//!   resizes it to a fixed square, returning an RGBA pixel buffer.
//! - `gdi`      — Win32-only, not unit-tested: converts a decoded RGBA buffer
//!   to an `HBITMAP` and wires it to a native BUTTON (`BS_BITMAP`/`BM_SETIMAGE`).
//!
//! # SVG
//!
//! SVG is explicitly descoped (Decision D1). `.svg` paths resolve to
//! `EmojiFallback` and never reach the decode layer. A future SVG rasterizer
//! can feed the same `DecodedIcon` type into `gdi::rgba_to_hbitmap`.

pub mod decode;
pub mod resolve;

#[cfg(windows)]
pub mod gdi;

#[allow(unused_imports)]
pub use decode::{decode_resize_file, decode_resize_rgba, DecodeError, DecodedIcon};
#[allow(unused_imports)]
pub use resolve::{icons_dirs, resolve_icon, IconResolution};
