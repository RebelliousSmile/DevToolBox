#![windows_subsystem = "windows"]

//! DevToolBox - cross-platform developer toolbox
//!
//! Cross-platform Rust application using `eframe`/`egui` for the UI (Part 2
//! of the multi-OS transformation). Phase 1 wires up a minimal `eframe::App`
//! bootstrap; the full card grid lands in Phase 2.

mod applications;
mod cleanup;
mod command_runner;
mod docker;
mod icons;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "macos", test))]
mod macos;
mod models;
mod net;
mod platform;
mod process_flags;
mod python_runtime;
mod storage;
mod ui;
#[cfg(windows)]
mod windows;

use eframe::egui;

use ui::egui_app::EguiApp;

struct FlushFile(std::fs::File);

impl std::io::Write for FlushFile {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = std::io::Write::write(&mut self.0, buffer)?;
        std::io::Write::flush(&mut self.0)?;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.0)
    }
}

/// Size past which [`rotate_if_oversized`] starts a fresh log file.
///
/// The log is append-only across every run, so without a ceiling it only ever
/// grows. 5 MB is roughly two thousand normal startups' worth of `info` lines,
/// while still being small enough to attach to a bug report.
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// One-generation rotation: when `path` is at or past `LOG_MAX_BYTES`, move it
/// to `<path>.old` (replacing any previous `.old`) so the next run starts from
/// an empty file. Keeping one generation rather than truncating outright means
/// a crash whose cause is in the *previous* run is still recoverable.
///
/// Every failure here is deliberately swallowed: a missing file (first run), a
/// rename refused because another instance holds the handle open, a read-only
/// directory — none of these are worth blocking startup over, and the caller
/// simply keeps appending to the existing file.
fn rotate_if_oversized(path: &std::path::Path, max_bytes: u64) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() < max_bytes {
        return;
    }
    let _ = std::fs::rename(path, path.with_extension("log.old"));
}

fn init_logging() -> Option<std::path::PathBuf> {
    let path = platform::state_log_path();
    let directory = path.parent()?.to_path_buf();
    std::fs::create_dir_all(&directory).ok()?;
    rotate_if_oversized(&path, LOG_MAX_BYTES);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    use std::io::Write as _;
    let _ = writeln!(
        file,
        "\n--- DevToolBox bootstrap pid={} ---",
        std::process::id()
    );
    let _ = file.flush();
    // No `filter_level` here, deliberately: it overrides the whole filter set
    // built from RUST_LOG above, which is how the default level silently
    // became Debug — that pulls in `wgpu_core`/`naga`'s per-shader trace and
    // grows this append-only, never-rotated file by ~490 KB per 10 idle
    // seconds. `info` is the default; `RUST_LOG=debug` still opts in.
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    builder
        .format_timestamp_millis()
        .target(env_logger::Target::Pipe(Box::new(FlushFile(file))))
        .init();
    Some(path)
}

fn main() {
    let log_path = init_logging();
    std::panic::set_hook(Box::new(|info| {
        log::error!(
            "panic: {info}\nbacktrace:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
    }));

    log::info!(
        "DevToolBox v{} starting; pid={}; log={:?}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        log_path
    );

    // Best-effort boot sync: align the registry startup entry with config.
    match storage::load() {
        Ok(cfg) => {
            if let Err(e) = platform::sync_startup(cfg.default_settings.launch_at_startup) {
                log::warn!("boot sync_startup failed: {}", e);
            }
        }
        Err(e) => {
            log::warn!("could not load config for boot sync: {}", e);
        }
    }

    let native_options = eframe::NativeOptions {
        viewport: ui::native_window::configure_viewport(
            egui::ViewportBuilder::default()
                .with_title("DevToolBox")
                .with_inner_size([800.0, 600.0])
                .with_min_inner_size([400.0, 300.0]),
        ),
        ..Default::default()
    };

    let run_result = eframe::run_native(
        "DevToolBox",
        native_options,
        Box::new(|cc| Ok(Box::new(EguiApp::new(cc)))),
    );

    match run_result {
        Ok(()) => log::info!("eframe event loop exited cleanly"),
        Err(e) => log::error!("eframe::run_native returned an error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{rotate_if_oversized, LOG_MAX_BYTES};
    use std::io::Write as _;

    /// A private temp directory for one test, named after the test and the
    /// pid so parallel runs never collide — the pattern already used by
    /// `icons::resolve` and `applications::usage`.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-log-rotation-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_log(path: &std::path::Path, bytes: usize) {
        let mut file = std::fs::File::create(path).expect("create log");
        file.write_all(&vec![b'x'; bytes]).expect("write log");
    }

    #[test]
    fn a_log_under_the_ceiling_is_left_alone() {
        let dir = temp_dir("under");
        let log = dir.join("devtoolbox.log");
        write_log(&log, 32);

        rotate_if_oversized(&log, 1024);

        assert_eq!(std::fs::metadata(&log).expect("still there").len(), 32);
        assert!(!dir.join("devtoolbox.log.old").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_at_or_past_the_ceiling_moves_to_dot_old() {
        let dir = temp_dir("over");
        let log = dir.join("devtoolbox.log");
        write_log(&log, 1024);

        rotate_if_oversized(&log, 1024);

        // The active file is gone: the caller re-creates it empty.
        assert!(!log.exists());
        assert_eq!(
            std::fs::metadata(dir.join("devtoolbox.log.old"))
                .expect("rotated")
                .len(),
            1024
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotating_twice_replaces_the_previous_generation_rather_than_piling_up() {
        let dir = temp_dir("twice");
        let log = dir.join("devtoolbox.log");

        write_log(&log, 1024);
        rotate_if_oversized(&log, 1024);
        write_log(&log, 2048);
        rotate_if_oversized(&log, 1024);

        assert_eq!(
            std::fs::metadata(dir.join("devtoolbox.log.old"))
                .expect("rotated")
                .len(),
            2048,
            "the second rotation must overwrite the first .old, not append a generation"
        );
        let count = std::fs::read_dir(&dir).expect("read dir").count();
        assert_eq!(
            count, 1,
            "exactly one file kept: the single .old generation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_log_is_a_no_op_rather_than_a_startup_failure() {
        let dir = temp_dir("missing");
        let log = dir.join("devtoolbox.log");

        rotate_if_oversized(&log, LOG_MAX_BYTES);

        assert!(!log.exists());
        assert!(!dir.join("devtoolbox.log.old").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
