//! Freedesktop Icon Theme Specification lookup.
//!
//! Resolves a bare freedesktop icon *name* (e.g. `"firefox"`, no extension,
//! no directory) to a real raster icon file on disk, by walking the active
//! GTK/freedesktop icon theme and its `Inherits=` chain, per the
//! [Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/icon-theme-spec-latest.html).
//!
//! # How this plugs into the existing resolution flow
//!
//! [`crate::icons::resolve::resolve_icon`] (Part 1) stays untouched and OS
//! neutral: given a raw `icon` string, it only ever checks direct/absolute
//! paths and `dir.join(icon)` against a fixed candidate-directory list — it
//! has no notion of icon *names* (extension-less, looked up by convention
//! across per-size theme subdirectories). [`resolve_icon_with_theme`] in
//! this module composes over it rather than bypassing it: it calls
//! `resolve_icon` first (so direct paths, `.svg` descoping (Decision D1),
//! and any user-bundled override under `platform::data_dir()/icons` all keep
//! working exactly as before and take priority), and only when that returns
//! [`IconResolution::EmojiFallback`] *and* the icon string looks like a bare
//! freedesktop icon name (no path separators, no `.` extension, no
//! whitespace) does it fall through to a real theme lookup via
//! [`find_icon`].
//!
//! # What is NOT handled (documented per the Part 3 plan's Risk register —
//! "document as Amendment, don't block" rather than silently mishandling)
//!
//! - **Desktop-environment theme discovery**: only GNOME (`gsettings`) and
//!   the GTK3 `settings.ini` convention are read. Xfce (`xfconf`), KDE
//!   (`kdeglobals`), and other non-GTK desktop environments are not probed;
//!   [`active_theme_name`] falls back to `"hicolor"` when neither source is
//!   available, which is always a safe (if visually plain) default per the
//!   spec (`hicolor` is guaranteed to exist as the universal fallback
//!   theme).
//! - **HiDPI `@2x`/`@3x` scaled directories**: an index.theme's
//!   `ScaledDirectories=` key is not read (only `Directories=` is), so
//!   scale-2 variants are never preferentially chosen even when present —
//!   consistent with this app not implementing HiDPI-aware rendering
//!   anywhere else yet.
//! - **`Context=` filtering**: subdirectories are not filtered by their
//!   `Context=` (e.g. `Applications` vs `Actions`), so a name that happens
//!   to collide across contexts could theoretically resolve to the "wrong"
//!   context's icon. In practice this is a non-issue for the icon names
//!   this app looks up (application/command names), and no such collision
//!   was observed against the real theme installed on the reference Ubuntu
//!   22.04.5 LTS system used for verification.
//! - **SVG icons**: never returned, by construction (see [`RASTER_EXTENSIONS`]) —
//!   this crate's decode pipeline (`icons::decode`, backed by the `image`
//!   crate) has no SVG rasterizer (Decision D1, inherited from Part 1). A
//!   theme directory that ships only `.svg` files for a given icon name
//!   (e.g. this system's own `Tela-dark` theme, which only ships
//!   `firefox-symbolic.svg`) is therefore transparently skipped in favor of
//!   a raster hit further down the `Inherits=` chain (`hicolor` ships
//!   `firefox.png` at multiple sizes) — verified for real on this machine,
//!   see the module's tests.
//! - **Theme-merge across base directories for `index.theme` itself**: when
//!   the *same* theme name has an `index.theme` in more than one base
//!   directory, only the first one found is parsed for `Inherits=`/
//!   `Directories=`. Icon *files* are still searched across every base
//!   directory for that theme's declared subdirectories (this is what makes
//!   a `~/.local/share/icons/hicolor/48x48/apps/foo.png` user override work
//!   without needing its own `index.theme`), so this simplification only
//!   affects the rare case of two conflicting `index.theme`s for the same
//!   theme name declaring different `Inherits=`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::icons::resolve::{resolve_icon, IconResolution};

/// Raster extensions this crate's decode pipeline can handle. Deliberately
/// mirrors `icons::resolve::ALLOWED_EXTENSIONS` (private to that module) —
/// duplicated here rather than exported, since the two lists exist for
/// different reasons (there: "what extension makes `icon` look like a
/// direct file path"; here: "what extension can a theme-lookup hit have")
/// and are only accidentally identical today.
const RASTER_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif"];

/// The universal fallback theme every spec-compliant icon theme setup must
/// have installed (ships with every desktop icon theme package).
const HICOLOR: &str = "hicolor";

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve `icon` the same way [`resolve_icon`] does, but with an additional
/// Linux-only fallback layer: if `icon` is not a direct/bundled file match,
/// and it looks like a bare freedesktop icon name, try a real freedesktop
/// icon-theme lookup (see module docs for ordering/precedence).
pub fn resolve_icon_with_theme(icon: &str, dirs: &[PathBuf], size: u32) -> IconResolution {
    match resolve_icon(icon, dirs) {
        IconResolution::Image(path) => IconResolution::Image(path),
        IconResolution::EmojiFallback(text) => {
            if looks_like_freedesktop_icon_name(icon) {
                if let Some(path) = find_icon(icon, size) {
                    return IconResolution::Image(path);
                }
            }
            IconResolution::EmojiFallback(text)
        }
    }
}

/// Look up `icon` (a bare freedesktop icon name, e.g. `"firefox"`) against
/// the active theme (see [`active_theme_name`]), its `Inherits=` chain, an
/// explicit `hicolor` fallback, and finally the unthemed `/usr/share/pixmaps`
/// convention. Returns `None` (never panics) if nothing matches at any
/// step — callers fall back to their own generic default.
pub fn find_icon(icon: &str, size: u32) -> Option<PathBuf> {
    let base_dirs = icon_theme_base_dirs();
    find_icon_with(icon, size, &active_theme_name(), &base_dirs)
}

// ---------------------------------------------------------------------------
// Core lookup (base dirs + theme name injectable for deterministic tests)
// ---------------------------------------------------------------------------

fn find_icon_with(icon: &str, size: u32, theme: &str, base_dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut visited = HashSet::new();
    if let Some(found) = find_icon_helper(icon, size, theme, base_dirs, &mut visited) {
        return Some(found);
    }
    // Per spec: always try `hicolor` explicitly, even if the active theme's
    // `Inherits=` chain never mentioned it (some minimal/malformed themes
    // omit it).
    if !visited.contains(HICOLOR) {
        if let Some(found) = find_icon_helper(icon, size, HICOLOR, base_dirs, &mut visited) {
            return Some(found);
        }
    }
    lookup_fallback_icon(icon)
}

fn find_icon_helper(
    icon: &str,
    size: u32,
    theme: &str,
    base_dirs: &[PathBuf],
    visited: &mut HashSet<String>,
) -> Option<PathBuf> {
    // Guards theme-inheritance cycles (e.g. a misconfigured theme that
    // inherits itself, directly or transitively).
    if !visited.insert(theme.to_string()) {
        return None;
    }

    let index = find_and_parse_index_theme(theme, base_dirs)?;

    if let Some(found) = lookup_icon_in_theme(icon, size, theme, &index, base_dirs) {
        return Some(found);
    }

    for parent in &index.inherits {
        if let Some(found) = find_icon_helper(icon, size, parent, base_dirs, visited) {
            return Some(found);
        }
    }

    None
}

/// Search every declared subdirectory of `theme` (across all `base_dirs`,
/// not just the one its `index.theme` was found in — see module docs) for a
/// raster file named `icon.<ext>`, returning the closest size match.
fn lookup_icon_in_theme(
    icon: &str,
    size: u32,
    theme: &str,
    index: &ThemeIndex,
    base_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let mut best: Option<(i64, PathBuf)> = None;

    for dir in &index.directories {
        for base in base_dirs {
            let subdir_path = base.join(theme).join(&dir.path);
            if !subdir_path.is_dir() {
                continue;
            }
            for ext in RASTER_EXTENSIONS {
                let candidate = subdir_path.join(format!("{icon}.{ext}"));
                if candidate.is_file() {
                    let distance = directory_size_distance(dir, size);
                    let is_better = match &best {
                        Some((best_distance, _)) => distance < *best_distance,
                        None => true,
                    };
                    if is_better {
                        best = Some((distance, candidate));
                    }
                }
            }
        }
    }

    best.map(|(_, path)| path)
}

/// Unthemed fallback per spec's `LookupFallbackIcon`: a handful of
/// well-known, non-theme icon directories checked as a last resort.
fn lookup_fallback_icon(icon: &str) -> Option<PathBuf> {
    for dir in ["/usr/share/pixmaps", "/usr/local/share/pixmaps"] {
        for ext in RASTER_EXTENSIONS {
            let candidate = Path::new(dir).join(format!("{icon}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// index.theme parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum DirType {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Debug, Clone, PartialEq)]
struct ThemeDir {
    /// Relative subdirectory path, e.g. `"48x48/apps"` or (as this system's
    /// `Tela-dark` theme names them) `"scalable/apps"`.
    path: String,
    size: u32,
    min_size: u32,
    max_size: u32,
    threshold: u32,
    dir_type: DirType,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct ThemeIndex {
    inherits: Vec<String>,
    directories: Vec<ThemeDir>,
}

/// Freedesktop Icon Theme Spec `DirectorySizeDistance` pseudo-algorithm: 0
/// when `size` is an acceptable match for `dir`, otherwise a positive
/// "distance" used to pick the closest available size.
fn directory_size_distance(dir: &ThemeDir, size: u32) -> i64 {
    let size = size as i64;
    match dir.dir_type {
        DirType::Fixed => (dir.size as i64 - size).abs(),
        DirType::Scalable => {
            if size < dir.min_size as i64 {
                dir.min_size as i64 - size
            } else if size > dir.max_size as i64 {
                size - dir.max_size as i64
            } else {
                0
            }
        }
        DirType::Threshold => {
            let lower = dir.size.saturating_sub(dir.threshold) as i64;
            let upper = dir.size as i64 + dir.threshold as i64;
            if size < lower {
                lower - size
            } else if size > upper {
                size - upper
            } else {
                0
            }
        }
    }
}

/// Find `theme`'s `index.theme` in the first `base_dirs` entry that has one,
/// and parse it. Returns `None` if the theme is not installed anywhere in
/// `base_dirs` (e.g. an `Inherits=` entry naming an uninstalled theme).
fn find_and_parse_index_theme(theme: &str, base_dirs: &[PathBuf]) -> Option<ThemeIndex> {
    for base in base_dirs {
        let index_path = base.join(theme).join("index.theme");
        if let Ok(contents) = std::fs::read_to_string(&index_path) {
            return Some(parse_index_theme(&contents));
        }
    }
    None
}

#[derive(Default)]
struct RawDir {
    size: Option<u32>,
    min_size: Option<u32>,
    max_size: Option<u32>,
    threshold: Option<u32>,
    dir_type: Option<String>,
}

/// Minimal INI-style parser for `index.theme` files — no external crate
/// (Decision: this Part's Stack section explicitly avoids a new dependency
/// for icon-theme parsing). Handles `[Section]` headers and `key=value`
/// lines; unknown keys/sections are ignored rather than erroring, which
/// keeps this tolerant of the KDE-specific extra keys (`DisplayDepth=`,
/// `LinkOverlay=`, ...) seen in real themes on this system (e.g.
/// `Tela-dark`).
fn parse_index_theme(contents: &str) -> ThemeIndex {
    let mut inherits: Vec<String> = Vec::new();
    let mut directories_order: Vec<String> = Vec::new();
    let mut raw_dirs: HashMap<String, RawDir> = HashMap::new();
    let mut current_section: Option<String> = None;

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current_section = Some(line[1..line.len() - 1].to_string());
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        match current_section.as_deref() {
            Some("Icon Theme") => match key {
                "Inherits" => {
                    inherits = value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "Directories" => {
                    directories_order = value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            },
            Some(section) => {
                let entry = raw_dirs.entry(section.to_string()).or_default();
                match key {
                    "Size" => entry.size = value.parse().ok(),
                    "MinSize" => entry.min_size = value.parse().ok(),
                    "MaxSize" => entry.max_size = value.parse().ok(),
                    "Threshold" => entry.threshold = value.parse().ok(),
                    "Type" => entry.dir_type = Some(value.to_string()),
                    _ => {}
                }
            }
            None => {}
        }
    }

    let directories = directories_order
        .into_iter()
        .filter_map(|name| {
            let raw = raw_dirs.get(&name)?;
            // A directory declared in `Directories=` but missing a `Size=`
            // key is malformed per spec; default to 48 (a common desktop
            // default) rather than dropping it, so a slightly malformed
            // real-world theme still degrades to "found, wrong-ish size"
            // instead of "silently never searched".
            let size = raw.size.unwrap_or(48);
            let dir_type = match raw.dir_type.as_deref() {
                Some("Fixed") => DirType::Fixed,
                Some("Scalable") => DirType::Scalable,
                _ => DirType::Threshold, // spec default
            };
            Some(ThemeDir {
                path: name,
                size,
                min_size: raw.min_size.unwrap_or(size),
                max_size: raw.max_size.unwrap_or(size),
                threshold: raw.threshold.unwrap_or(2), // spec default
                dir_type,
            })
        })
        .collect();

    ThemeIndex {
        inherits,
        directories,
    }
}

// ---------------------------------------------------------------------------
// Active theme discovery
// ---------------------------------------------------------------------------

/// The active GTK/freedesktop icon theme name. Tries GNOME's `gsettings`
/// first (works for GNOME and most GTK-based desktop sessions, including
/// this system's — verified for real, see tests), then the GTK3
/// `settings.ini` convention, then falls back to `"hicolor"` (see module
/// docs for what is NOT probed: Xfce/KDE-native settings stores).
fn active_theme_name() -> String {
    gsettings_icon_theme()
        .or_else(gtk3_icon_theme_from_settings_ini)
        .unwrap_or_else(|| HICOLOR.to_string())
}

fn gsettings_icon_theme() -> Option<String> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let unquoted = raw.trim().trim_matches('\'').trim_matches('"');
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}

fn gtk3_icon_theme_from_settings_ini() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(home).join(".config/gtk-3.0/settings.ini");
    let contents = std::fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        let (key, value) = line.split_once('=')?;
        if key.trim() == "gtk-icon-theme-name" {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Base directory resolution (freedesktop Icon Theme Spec base dir list)
// ---------------------------------------------------------------------------

/// Candidate base directories under which `<theme>/index.theme` and
/// `<theme>/<subdir>/<icon>.<ext>` are searched, in spec order:
/// `$HOME/.icons` (legacy per-user override), `$XDG_DATA_HOME/icons`, then
/// each `$XDG_DATA_DIRS` entry's `icons` subdirectory.
fn icon_theme_base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".icons"));
    }

    dirs.push(xdg_data_home().join("icons"));

    for dir in xdg_data_dirs() {
        dirs.push(dir.join("icons"));
    }

    dirs
}

fn xdg_data_home() -> PathBuf {
    match std::env::var("XDG_DATA_HOME") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => match std::env::var("HOME") {
            Ok(home) => PathBuf::from(home).join(".local/share"),
            Err(_) => PathBuf::from("/"),
        },
    }
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    match std::env::var("XDG_DATA_DIRS") {
        Ok(value) if !value.is_empty() => value.split(':').map(PathBuf::from).collect(),
        _ => vec![
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ],
    }
}

// ---------------------------------------------------------------------------
// Heuristic: does `icon` look like a bare freedesktop icon name?
// ---------------------------------------------------------------------------

/// `true` for strings shaped like conventional freedesktop icon names
/// (`"firefox"`, `"org.gnome.Software"`, `"my-app_v2"`) — no path
/// separators, no whitespace, ASCII alphanumeric plus `.`/`-`/`_`. Excludes
/// bare emoji/text labels (e.g. `"📝"`) so those never trigger a pointless
/// filesystem-searching theme lookup.
fn looks_like_freedesktop_icon_name(icon: &str) -> bool {
    !icon.is_empty()
        && icon
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Minimal single-use temp-dir helper (mirrors `icons::resolve`'s own
    /// test helper — no `tempfile` crate, Decision D4).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let id = format!(
                "devtoolbox_icon_theme_test_{label}_{ts}_{:?}",
                std::thread::current().id()
            );
            let dir = std::env::temp_dir().join(id);
            fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        fs::write(path, contents).expect("write");
    }

    /// Build a tiny fake theme tree in a temp base dir:
    /// `<base>/<theme>/index.theme` plus one `48x48/apps/<icon>.png`.
    fn make_fake_theme(base: &Path, theme: &str, inherits: Option<&str>) {
        let index = base.join(theme).join("index.theme");
        let inherits_line = inherits
            .map(|p| format!("Inherits={p}\n"))
            .unwrap_or_default();
        write(
            &index,
            &format!(
                "[Icon Theme]\nName={theme}\n{inherits_line}Directories=48x48/apps\n\n[48x48/apps]\nSize=48\nContext=Applications\nType=Fixed\n"
            ),
        );
    }

    // --- parse_index_theme ---------------------------------------------------

    #[test]
    fn parse_index_theme_reads_inherits_and_directories() {
        let contents = "[Icon Theme]\nName=Test\nInherits=hicolor,Adwaita\nDirectories=48x48/apps,scalable/apps\n\n[48x48/apps]\nSize=48\nType=Fixed\n\n[scalable/apps]\nSize=48\nMinSize=8\nMaxSize=512\nType=Scalable\n";
        let index = parse_index_theme(contents);
        assert_eq!(index.inherits, vec!["hicolor", "Adwaita"]);
        assert_eq!(index.directories.len(), 2);
        assert_eq!(index.directories[0].path, "48x48/apps");
        assert_eq!(index.directories[0].dir_type, DirType::Fixed);
        assert_eq!(index.directories[1].dir_type, DirType::Scalable);
        assert_eq!(index.directories[1].min_size, 8);
        assert_eq!(index.directories[1].max_size, 512);
    }

    #[test]
    fn parse_index_theme_defaults_threshold_and_ignores_unknown_keys() {
        // Mirrors real KDE-flavored themes (e.g. this system's Tela-dark)
        // that add extra keys like DisplayDepth=/LinkOverlay=.
        let contents = "[Icon Theme]\nName=Test\nDirectories=32/actions\nDisplayDepth=32\nLinkOverlay=link_overlay\n\n[32/actions]\nSize=32\nContext=Actions\n";
        let index = parse_index_theme(contents);
        assert_eq!(index.directories.len(), 1);
        assert_eq!(index.directories[0].dir_type, DirType::Threshold); // spec default
        assert_eq!(index.directories[0].threshold, 2); // spec default
    }

    #[test]
    fn parse_index_theme_handles_missing_optional_sections_gracefully() {
        let index = parse_index_theme("");
        assert!(index.inherits.is_empty());
        assert!(index.directories.is_empty());
    }

    // --- directory_size_distance ---------------------------------------------

    #[test]
    fn directory_size_distance_fixed_exact_match_is_zero() {
        let dir = ThemeDir {
            path: "x".into(),
            size: 48,
            min_size: 48,
            max_size: 48,
            threshold: 2,
            dir_type: DirType::Fixed,
        };
        assert_eq!(directory_size_distance(&dir, 48), 0);
        assert_eq!(directory_size_distance(&dir, 64), 16);
    }

    #[test]
    fn directory_size_distance_threshold_within_range_is_zero() {
        let dir = ThemeDir {
            path: "x".into(),
            size: 48,
            min_size: 48,
            max_size: 48,
            threshold: 4,
            dir_type: DirType::Threshold,
        };
        assert_eq!(directory_size_distance(&dir, 46), 0); // within threshold
        assert_eq!(directory_size_distance(&dir, 60), 8); // 60 - (48+4)
    }

    #[test]
    fn directory_size_distance_scalable_within_range_is_zero() {
        let dir = ThemeDir {
            path: "x".into(),
            size: 48,
            min_size: 8,
            max_size: 512,
            threshold: 2,
            dir_type: DirType::Scalable,
        };
        assert_eq!(directory_size_distance(&dir, 256), 0);
        assert_eq!(directory_size_distance(&dir, 4), 4); // below min_size
    }

    // --- find_icon_with (deterministic, temp-dir based) -----------------------

    #[test]
    fn find_icon_with_hits_a_raster_icon_in_the_active_theme_itself() {
        let dir = TempDir::new("direct_hit");
        make_fake_theme(dir.path(), "MyTheme", None);
        write(&dir.path().join("MyTheme/48x48/apps/myicon.png"), "fake");

        let found = find_icon_with("myicon", 48, "MyTheme", &[dir.path().to_path_buf()]);
        assert_eq!(
            found,
            Some(dir.path().join("MyTheme/48x48/apps/myicon.png"))
        );
    }

    #[test]
    fn find_icon_with_falls_through_inherits_chain() {
        let dir = TempDir::new("inherits_chain");
        make_fake_theme(dir.path(), "Child", Some("Parent"));
        make_fake_theme(dir.path(), "Parent", None);
        // Only the parent theme actually ships the icon file.
        write(&dir.path().join("Parent/48x48/apps/inherited.png"), "fake");

        let found = find_icon_with("inherited", 48, "Child", &[dir.path().to_path_buf()]);
        assert_eq!(
            found,
            Some(dir.path().join("Parent/48x48/apps/inherited.png"))
        );
    }

    #[test]
    fn find_icon_with_skips_svg_only_entries_and_falls_back_to_hicolor() {
        // Reproduces this machine's real Tela-dark (svg-only "firefox") ->
        // hicolor (raster "firefox.png") situation in a controlled temp dir.
        let dir = TempDir::new("svg_skip");
        make_fake_theme(dir.path(), "Child", None); // no explicit Inherits=
        make_fake_theme(dir.path(), HICOLOR, None);
        write(&dir.path().join("Child/48x48/apps/app.svg"), "<svg/>");
        write(&dir.path().join("hicolor/48x48/apps/app.png"), "fake");

        let found = find_icon_with("app", 48, "Child", &[dir.path().to_path_buf()]);
        assert_eq!(found, Some(dir.path().join("hicolor/48x48/apps/app.png")));
    }

    #[test]
    fn find_icon_with_cycle_does_not_infinite_loop_or_panic() {
        let dir = TempDir::new("cycle");
        make_fake_theme(dir.path(), "A", Some("B"));
        make_fake_theme(dir.path(), "B", Some("A")); // A <-> B cycle

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            find_icon_with("nonexistent", 48, "A", &[dir.path().to_path_buf()])
        }));
        assert!(outcome.is_ok(), "a theme-inheritance cycle must not panic");
        assert_eq!(outcome.unwrap(), None);
    }

    #[test]
    fn find_icon_with_missing_theme_returns_none_without_panicking() {
        let dir = TempDir::new("missing_theme");
        let found = find_icon_with("anything", 48, "DoesNotExist", &[dir.path().to_path_buf()]);
        assert_eq!(found, None);
    }

    #[test]
    fn find_icon_with_picks_closest_size_when_multiple_match() {
        let base = TempDir::new("closest_size");
        let index = "[Icon Theme]\nName=Multi\nDirectories=16x16/apps,48x48/apps,128x128/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n\n[48x48/apps]\nSize=48\nType=Fixed\n\n[128x128/apps]\nSize=128\nType=Fixed\n";
        write(&base.path().join("Multi/index.theme"), index);
        write(&base.path().join("Multi/16x16/apps/thing.png"), "fake");
        write(&base.path().join("Multi/48x48/apps/thing.png"), "fake");
        write(&base.path().join("Multi/128x128/apps/thing.png"), "fake");

        let found = find_icon_with("thing", 44, "Multi", &[base.path().to_path_buf()]);
        assert_eq!(
            found,
            Some(base.path().join("Multi/48x48/apps/thing.png")),
            "44 is closest to the 48x48 variant"
        );
    }

    // --- looks_like_freedesktop_icon_name -------------------------------------

    #[test]
    fn looks_like_freedesktop_icon_name_accepts_conventional_names() {
        assert!(looks_like_freedesktop_icon_name("firefox"));
        assert!(looks_like_freedesktop_icon_name("org.gnome.Software"));
        assert!(looks_like_freedesktop_icon_name("my-app_v2"));
    }

    #[test]
    fn looks_like_freedesktop_icon_name_rejects_emoji_and_paths() {
        assert!(!looks_like_freedesktop_icon_name("📝"));
        assert!(!looks_like_freedesktop_icon_name("some/path.png"));
        assert!(!looks_like_freedesktop_icon_name(""));
        assert!(!looks_like_freedesktop_icon_name("has space"));
    }

    // --- resolve_icon_with_theme (composition contract) -----------------------

    #[test]
    fn resolve_icon_with_theme_prefers_direct_resolve_icon_hit() {
        let dir = TempDir::new("precedence");
        let png = dir.path().join("bundled.png");
        // Minimal 1x1 PNG bytes are irrelevant here — resolve_icon only
        // checks existence + extension, not decodability.
        fs::write(&png, [0u8; 8]).expect("write");

        let result = resolve_icon_with_theme("bundled.png", &[dir.path().to_path_buf()], 48);
        assert_eq!(result, IconResolution::Image(png));
    }

    #[test]
    fn resolve_icon_with_theme_falls_back_to_emoji_for_garbage_names() {
        // "totally-unknown-icon-name-xyz-987" is not installed in any real
        // theme on this machine or in any candidate dir — proves the "no
        // panic, generic fallback" half of the acceptance criterion using
        // the real, unmodified base-dir/theme discovery path.
        let result =
            resolve_icon_with_theme("totally-unknown-icon-name-xyz-987", &[], 48);
        assert_eq!(
            result,
            IconResolution::EmojiFallback("totally-unknown-icon-name-xyz-987".to_string())
        );
    }

    #[test]
    fn resolve_icon_with_theme_ignores_bare_emoji_without_filesystem_search() {
        let result = resolve_icon_with_theme("📝", &[], 48);
        assert_eq!(result, IconResolution::EmojiFallback("📝".to_string()));
    }

    // --- Real-machine verification (acceptance criterion) ---------------------
    //
    // These exercise the REAL active theme discovery + REAL
    // /usr/share/icons on this Ubuntu 22.04.5 LTS system — not a fixture.
    // Verified manually before writing this test: `gsettings get
    // org.gnome.desktop.interface icon-theme` -> 'Tela-dark', which
    // Inherits=hicolor,Adwaita,breeze; Tela-dark ships only
    // `firefox-symbolic.svg` (unsupported format, skipped), `breeze` is not
    // installed on this system (Inherits chain entry with no matching
    // theme, must not panic), and `hicolor` ships real
    // `48x48/apps/firefox.png` (among other sizes) — so this proves the
    // full active-theme -> inheritance -> hicolor raster fallback path
    // against real installed system state.

    #[test]
    fn real_system_resolves_firefox_to_a_real_icon_file() {
        let found = find_icon("firefox", 48);
        let found = found.unwrap_or_else(|| {
            panic!(
                "expected a real 'firefox' icon file to be found via the active theme \
                 ({:?}) or its hicolor fallback on this system — got None",
                active_theme_name()
            )
        });
        assert!(
            found.is_file(),
            "resolved path {found:?} must actually exist as a file"
        );
        let ext = found
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            RASTER_EXTENSIONS.contains(&ext.as_str()),
            "resolved icon must be a raster format this app can decode, got {found:?}"
        );
    }

    #[test]
    fn real_system_unknown_icon_name_falls_back_without_panicking() {
        let outcome = std::panic::catch_unwind(|| {
            find_icon("this-icon-name-almost-certainly-does-not-exist-anywhere-42", 48)
        });
        assert!(outcome.is_ok(), "an unknown icon name must never panic");
        assert_eq!(outcome.unwrap(), None);
    }

    #[test]
    fn real_active_theme_name_is_non_empty() {
        // Whatever the real desktop session reports (or the "hicolor"
        // fallback if none), this must never be an empty string.
        assert!(!active_theme_name().is_empty());
    }
}
