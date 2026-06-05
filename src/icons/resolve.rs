//! Pure icon-resolution rule — no Win32, no GUI, fully unit-tested.
//!
//! Maps the `icon: String` field of a `Command` or `Category` to either an
//! existing raster image file (`Image`) or an emoji/text fallback
//! (`EmojiFallback`).  The rule is intentionally pure and parameterised on the
//! candidate directories so tests can supply a temp dir without touching
//! `%APPDATA%`.

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of resolving a command's `icon` string.
#[derive(Debug, Clone, PartialEq)]
pub enum IconResolution {
    /// The icon string resolved to an existing raster image file.
    Image(PathBuf),
    /// The icon string is a bare emoji, a missing file, an unsupported format
    /// (e.g. `.svg`), or cannot otherwise be resolved — use as a text label.
    EmojiFallback(String),
}

// ---------------------------------------------------------------------------
// Resolution rule
// ---------------------------------------------------------------------------

/// Raster extensions that may be loaded by the `image` crate in this build.
///
/// `.svg` is deliberately absent (Decision D1 — descoped to a follow-up).
const ALLOWED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif"];

/// Resolve `icon` against each directory in `dirs` in order.
///
/// Resolution rule:
/// 1. If `icon` looks like an absolute path that exists with an allowed
///    extension — return `Image(icon)`.
/// 2. For each `dir` in `dirs`: if `dir.join(icon)` exists with an allowed
///    extension — return `Image(path)`.
/// 3. Otherwise — return `EmojiFallback(icon)`.
///
/// `.svg` paths always resolve to `EmojiFallback` regardless of existence,
/// enforcing Decision D1 at the resolution boundary.
///
/// # Parameters
/// - `icon`  — the raw value of `Command.icon` / `Category.icon`.
/// - `dirs`  — candidate search directories, tried in order (see [`icons_dirs`]).
pub fn resolve_icon(icon: &str, dirs: &[PathBuf]) -> IconResolution {
    // Guard: .svg is explicitly descoped regardless of file existence.
    if has_svg_extension(icon) {
        return IconResolution::EmojiFallback(icon.to_string());
    }

    // Check if icon is itself a valid absolute/relative path.
    let icon_path = Path::new(icon);
    if icon_path.is_absolute() && icon_path.exists() && has_allowed_extension(icon_path) {
        return IconResolution::Image(icon_path.to_path_buf());
    }

    // Search candidate directories.
    for dir in dirs {
        let candidate = dir.join(icon);
        if candidate.exists() && has_allowed_extension(&candidate) {
            return IconResolution::Image(candidate);
        }
    }

    // Nothing found — use as emoji / text label.
    IconResolution::EmojiFallback(icon.to_string())
}

/// Build the ordered list of candidate icon search directories.
///
/// Order (Decision D4):
/// 1. `%APPDATA%\WinFXStart\icons\`  — user-editable; overrides bundled icons.
/// 2. `<exe_dir>\assets\`            — bundled default icons shipped with the binary.
/// 3. `.\assets\`                    — dev/repo fallback (useful during development).
///
/// If `APPDATA` is unset, that candidate is skipped (no panic).
pub fn icons_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(3);

    // 1. %APPDATA%\WinFXStart\icons\
    if let Ok(appdata) = std::env::var("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("WinFXStart").join("icons"));
    }

    // 2. <exe_dir>\assets\
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            dirs.push(exe_dir.join("assets"));
        }
    }

    // 3. .\assets\
    dirs.push(PathBuf::from("assets"));

    dirs
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn has_allowed_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| ALLOWED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn has_svg_extension(icon: &str) -> bool {
    Path::new(icon)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("svg"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a temporary directory containing a fake PNG file and return both
    /// the dir path and the file path.
    fn make_temp_png() -> (tempfile_dir::TempDir, PathBuf) {
        let dir = tempfile_dir::TempDir::new();
        let file = dir.path().join("icon.png");
        // Write a minimal valid 1×1 PNG (67 bytes — the smallest possible PNG).
        fs::write(&file, MINIMAL_PNG).expect("write temp png");
        (dir, file)
    }

    /// Minimal valid 1×1 red PNG (hand-crafted; passes `image` crate decoding).
    const MINIMAL_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR length + type
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, // 8-bit RGB, no interlace
        0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, // IDAT length + type
        0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, // compressed pixel data
        0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, // continuation
        0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, // IEND length + type
        0x44, 0xAE, 0x42, 0x60, 0x82, // IEND data + CRC
    ];

    // --- resolve_icon tests -------------------------------------------------

    #[test]
    fn existing_png_file_resolves_to_image() {
        let (dir, file) = make_temp_png();
        // Pass only the filename (not the full path) plus the temp dir as
        // the search dir so it exercises the dir-join path.
        let filename = file.file_name().unwrap().to_str().unwrap();
        let result = resolve_icon(filename, &[dir.path().to_path_buf()]);
        assert!(
            matches!(result, IconResolution::Image(_)),
            "expected Image, got {result:?}"
        );
        drop(dir);
    }

    #[test]
    fn missing_file_resolves_to_emoji_fallback() {
        let result = resolve_icon("nonexistent.png", &[PathBuf::from("/no/such/dir")]);
        assert_eq!(
            result,
            IconResolution::EmojiFallback("nonexistent.png".to_string())
        );
    }

    #[test]
    fn bare_emoji_resolves_to_emoji_fallback() {
        let result = resolve_icon("📝", &[]);
        assert_eq!(result, IconResolution::EmojiFallback("📝".to_string()));
    }

    #[test]
    fn svg_path_resolves_to_emoji_fallback_descope_guard() {
        // Even if an SVG file exists it must NOT resolve to Image (D1 descope).
        let result = resolve_icon("icon.svg", &[]);
        assert_eq!(
            result,
            IconResolution::EmojiFallback("icon.svg".to_string())
        );
    }

    #[test]
    fn svg_path_existing_file_still_emoji_fallback() {
        // Create an actual .svg file — resolution must still return EmojiFallback.
        let dir = tempfile_dir::TempDir::new();
        let svg_file = dir.path().join("icon.svg");
        fs::write(&svg_file, "<svg/>").expect("write svg");

        let result = resolve_icon("icon.svg", &[dir.path().to_path_buf()]);
        assert_eq!(
            result,
            IconResolution::EmojiFallback("icon.svg".to_string()),
            "SVG must never resolve to Image (Decision D1)"
        );
    }

    #[test]
    fn first_matching_dir_wins() {
        // Two dirs: second has the file, first does not.
        // Expect Image pointing to the file in the second dir.
        let dir1 = tempfile_dir::TempDir::new();
        let dir2 = tempfile_dir::TempDir::new();
        let file = dir2.path().join("test.png");
        fs::write(&file, MINIMAL_PNG).expect("write png");

        let result = resolve_icon(
            "test.png",
            &[dir1.path().to_path_buf(), dir2.path().to_path_buf()],
        );
        assert!(matches!(result, IconResolution::Image(p) if p == file));
    }
}

// ---------------------------------------------------------------------------
// Minimal temp-dir helper (avoids adding `tempfile` crate — Decision D4)
// ---------------------------------------------------------------------------

/// Minimal single-use temp-dir helper that cleans up on drop.
/// We avoid pulling in the `tempfile` crate (Decision D4: no new crates for
/// this module).
#[cfg(test)]
mod tempfile_dir {
    use std::path::{Path, PathBuf};

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            use std::time::{SystemTime, UNIX_EPOCH};
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            // Use thread id for uniqueness when tests run in parallel.
            let id = format!("winfxstart_test_{ts}_{:?}", std::thread::current().id());
            let dir = std::env::temp_dir().join(id);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
