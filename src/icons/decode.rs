//! Pure raster decode and resize — no Win32, no GUI, fully unit-tested.
//!
//! Decodes a raster image from a byte slice (or a file path) using the
//! `image` crate, then resizes it to a fixed square and returns an RGBA pixel
//! buffer (`DecodedIcon`).

use std::io::Cursor;
use std::path::Path;

use image::imageops::FilterType;
use image::{ImageFormat, RgbaImage};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A decoded and resized icon: a square RGBA pixel buffer.
#[derive(Debug, Clone)]
pub struct DecodedIcon {
    /// Pixel width (== height; always equals the requested `size`).
    pub width: u32,
    /// Pixel height (== width; always equals the requested `size`).
    pub height: u32,
    /// Raw RGBA bytes, row-major, top-to-bottom. Length == width * height * 4.
    pub rgba: Vec<u8>,
}

/// Errors returned by the decode pipeline.
#[derive(Debug)]
pub enum DecodeError {
    /// An I/O error occurred while reading the file.
    Io(std::io::Error),
    /// The bytes could not be decoded as a supported raster image.
    Decode(image::ImageError),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(e) => write!(f, "icon I/O error: {e}"),
            DecodeError::Decode(e) => write!(f, "icon decode error: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecodeError::Io(e) => Some(e),
            DecodeError::Decode(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        DecodeError::Io(e)
    }
}

impl From<image::ImageError> for DecodeError {
    fn from(e: image::ImageError) -> Self {
        DecodeError::Decode(e)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Decode `bytes` as a raster image and resize it to `size × size` pixels.
///
/// Returns a [`DecodedIcon`] whose `rgba` vec has length `size * size * 4`.
///
/// # Errors
/// Returns [`DecodeError::Decode`] if the bytes are not a valid supported
/// raster image (e.g. non-image data, SVG, corrupted PNG).
pub fn decode_resize_rgba(bytes: &[u8], size: u32) -> Result<DecodedIcon, DecodeError> {
    let img = image::load_from_memory(bytes)?;
    let resized: RgbaImage = image::imageops::resize(&img, size, size, FilterType::Lanczos3);

    Ok(DecodedIcon {
        width: resized.width(),
        height: resized.height(),
        rgba: resized.into_raw(),
    })
}

/// Read a raster image file from `path`, decode it, and resize to `size × size`.
///
/// # Errors
/// Returns [`DecodeError::Io`] on file read errors or [`DecodeError::Decode`]
/// on image decode errors.
pub fn decode_resize_file(path: &Path, size: u32) -> Result<DecodedIcon, DecodeError> {
    let bytes = std::fs::read(path)?;
    decode_resize_rgba(&bytes, size)
}

// ---------------------------------------------------------------------------
// Internal helpers (exposed for `gdi.rs` but not for tests)
// ---------------------------------------------------------------------------

/// Attempt to guess the image format from `bytes` without fully decoding.
/// Used only in tests to verify the format of in-memory buffers.
#[cfg(test)]
fn guess_format(bytes: &[u8]) -> Option<ImageFormat> {
    image::guess_format(bytes).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build an in-memory PNG of the given dimensions using the `image` crate.
    fn make_in_memory_png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_fn(w, h, |x, y| {
            // Alternating red/blue checkerboard for visual variety.
            if (x + y) % 2 == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .expect("encode png");
        buf
    }

    // --- decode_resize_rgba tests -------------------------------------------

    #[test]
    fn decode_png_produces_correct_dimensions() {
        let png = make_in_memory_png(64, 48); // non-square source
        let target = 32u32;
        let icon = decode_resize_rgba(&png, target).expect("decode failed");
        assert_eq!(icon.width, target, "width should equal target size");
        assert_eq!(icon.height, target, "height should equal target size");
    }

    #[test]
    fn decode_png_rgba_buffer_length_is_correct() {
        let png = make_in_memory_png(16, 16);
        let target = 8u32;
        let icon = decode_resize_rgba(&png, target).expect("decode failed");
        let expected_len = (target * target * 4) as usize;
        assert_eq!(
            icon.rgba.len(),
            expected_len,
            "RGBA buffer must be width * height * 4"
        );
    }

    #[test]
    fn non_image_bytes_return_decode_error() {
        let garbage = b"this is not an image at all, just random text bytes 1234567890";
        let result = decode_resize_rgba(garbage, 32);
        assert!(
            result.is_err(),
            "non-image bytes must return an error, not panic"
        );
        // Must be a Decode variant, not Io.
        assert!(
            matches!(result.unwrap_err(), DecodeError::Decode(_)),
            "expected DecodeError::Decode"
        );
    }

    #[test]
    fn decode_large_source_to_small_target() {
        let png = make_in_memory_png(256, 256);
        let target = 16u32;
        let icon = decode_resize_rgba(&png, target).expect("decode failed");
        assert_eq!(icon.width, target);
        assert_eq!(icon.height, target);
        assert_eq!(icon.rgba.len(), (target * target * 4) as usize);
    }

    #[test]
    fn decode_small_source_to_large_target() {
        let png = make_in_memory_png(4, 4);
        let target = 64u32;
        let icon = decode_resize_rgba(&png, target).expect("decode failed");
        assert_eq!(icon.width, target);
        assert_eq!(icon.height, target);
        assert_eq!(icon.rgba.len(), (target * target * 4) as usize);
    }

    #[test]
    fn in_memory_png_is_recognized_as_png() {
        let png = make_in_memory_png(8, 8);
        let fmt = guess_format(&png);
        assert_eq!(fmt, Some(ImageFormat::Png));
    }

    // --- decode_resize_file tests -------------------------------------------

    #[test]
    fn decode_file_matches_in_memory_result() {
        let png = make_in_memory_png(32, 32);
        let dir = std::env::temp_dir();
        let path = dir.join("winfxstart_decode_test.png");
        std::fs::write(&path, &png).expect("write temp png");

        let from_file = decode_resize_file(&path, 16).expect("decode file");
        let from_mem = decode_resize_rgba(&png, 16).expect("decode mem");

        assert_eq!(from_file.width, from_mem.width);
        assert_eq!(from_file.height, from_mem.height);
        assert_eq!(from_file.rgba, from_mem.rgba);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn decode_missing_file_returns_io_error() {
        let path = std::path::PathBuf::from("/no/such/file/icon.png");
        let result = decode_resize_file(&path, 32);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), DecodeError::Io(_)),
            "missing file must return DecodeError::Io"
        );
    }
}
