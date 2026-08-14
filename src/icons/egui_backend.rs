//! `IconBackend` implementation for `egui`: converts a [`DecodedIcon`] into
//! an `egui::TextureHandle` via `egui::ColorImage`.
//!
//! Replaces `src/icons/gdi.rs` (Win32 `HBITMAP` bridge, deleted in Part 2)
//! as the single icon-rendering path shared by both OSes.

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::icons::backend::IconBackend;
use crate::icons::decode::DecodedIcon;

/// Converts decoded RGBA icons into `egui` textures, using an `egui::Context`
/// to allocate and cache texture handles.
pub struct EguiIconBackend {
    ctx: Context,
}

impl EguiIconBackend {
    /// Build a new backend bound to `ctx`. Cheap to construct — `egui::Context`
    /// is an `Arc`-backed handle, so cloning it (as callers typically do from
    /// `eframe::CreationContext::egui_ctx`) is inexpensive.
    pub fn new(ctx: Context) -> Self {
        Self { ctx }
    }
}

impl IconBackend for EguiIconBackend {
    type Handle = TextureHandle;

    /// Convert `icon`'s RGBA buffer into an `egui::ColorImage` and upload it
    /// as a texture via the bound `egui::Context`.
    ///
    /// `name` becomes the texture's debug name (visible in `egui`'s texture
    /// inspector) — it is not a cache key, so calling `load` twice with the
    /// same `name` allocates two independent textures.
    fn load(&mut self, name: &str, icon: &DecodedIcon) -> TextureHandle {
        let size = [icon.width as usize, icon.height as usize];
        let color_image = ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
        self.ctx
            .load_texture(name, color_image, TextureOptions::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::decode::decode_resize_rgba;

    /// Build an in-memory PNG using the `image` crate (mirrors the helper in
    /// `decode.rs`'s own tests, kept local to avoid a cross-module test dep).
    fn make_in_memory_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            if (x + y) % 2 == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode png");
        buf
    }

    #[test]
    fn load_uploads_a_texture_of_the_expected_size() {
        // egui::Context can allocate textures without a running eframe event
        // loop / windowing system — this proves the upload path is exercised
        // in a headless unit test, not just visually.
        let ctx = Context::default();
        let mut backend = EguiIconBackend::new(ctx);

        let png = make_in_memory_png(16, 16);
        let decoded = decode_resize_rgba(&png, 32).expect("decode test png");

        let texture = backend.load("test-icon", &decoded);
        assert_eq!(texture.size(), [32, 32]);
    }
}
