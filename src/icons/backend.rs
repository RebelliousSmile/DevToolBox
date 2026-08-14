//! Render-agnostic icon backend trait.
//!
//! Different UI toolkits need different handle types for a decoded icon
//! (e.g. a GDI `HBITMAP` on Win32 — see the now-removed `gdi.rs` — or an
//! `egui::TextureHandle` for `egui`). This trait decouples the OS-neutral
//! decode pipeline in `src/icons/decode.rs` from any single rendering
//! backend, so a new backend can be added without touching `decode.rs`.

use crate::icons::decode::DecodedIcon;

/// Converts a [`DecodedIcon`] into a renderer-specific handle.
///
/// Implementations own whatever renderer/context state is required to
/// perform the conversion (e.g. an `egui::Context` for texture allocation).
pub trait IconBackend {
    /// The renderer-specific handle type produced from a decoded icon
    /// (e.g. `egui::TextureHandle`).
    type Handle;

    /// Upload/convert `icon` into a renderer-specific handle.
    ///
    /// `name` is a stable identifier for the icon (used as a debug label /
    /// cache key by some backends, e.g. `egui`'s texture manager).
    fn load(&mut self, name: &str, icon: &DecodedIcon) -> Self::Handle;
}
