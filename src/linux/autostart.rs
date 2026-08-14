//! XDG autostart integration.
//!
//! Writes/removes `$XDG_CONFIG_HOME/autostart/devtoolbox.desktop` (falling
//! back to `~/.config/autostart/devtoolbox.desktop`) per the freedesktop.org
//! [Desktop Application Autostart
//! Specification](https://specifications.freedesktop.org/autostart-spec/autostart-spec-latest.html),
//! wiring DevToolBox's "launch at startup" toggle into GNOME/Xfce and other
//! spec-compliant desktop environments.
//!
//! # Non-blocking failure mode (frozen master-plan decision 5)
//! Any failure here — an unwritable `autostart` directory, an I/O error, an
//! unresolvable current-executable path — is logged via `log::warn!` and
//! returned as `Err`, but this module never panics. The
//! [`crate::platform::linux::LinuxStartupProvider`] wrapper and
//! `main.rs`'s boot-sync call site (`platform::sync_startup`) only ever log
//! a warning on `Err`; they never `.unwrap()`/`.expect()` a result from this
//! module, so an autostart write failure can never block the app from
//! starting or running.
//!
//! # Testability
//! Mirrors the env-injectable `*_with_env` pattern already used in
//! `platform::linux` (`config_path_with_env`, `data_dir_with_env`, ...): the
//! public `register`/`unregister`/`is_registered`/`autostart_file_path`
//! functions read real process environment variables, and delegate to
//! `*_with_env` variants that take an injectable `name -> Option<String>`
//! lookup closure so tests can point the autostart directory at an isolated
//! temp directory instead of the real `$HOME`.

use std::fs;
use std::io;
use std::path::PathBuf;

const DESKTOP_FILE_NAME: &str = "devtoolbox.desktop";
const APP_DISPLAY_NAME: &str = "DevToolBox";

/// `name -> Option<value>` environment lookup, injectable for testing.
type EnvLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

fn std_env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn home_dir(env: &EnvLookup) -> PathBuf {
    // No sensible fallback exists if HOME is unset; `/` keeps path-joining
    // well-defined rather than panicking, mirroring `platform::linux`'s
    // `home_dir`. HOME is always set in practice on Linux user sessions.
    env("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Directory holding autostart `.desktop` files:
/// `$XDG_CONFIG_HOME/autostart`, falling back to `~/.config/autostart` when
/// `XDG_CONFIG_HOME` is unset or empty (same XDG "unset or empty means use
/// the default" rule `platform::linux::xdg_base_dir` applies).
fn autostart_dir_with_env(env: &EnvLookup) -> PathBuf {
    let config_home = match env("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home_dir(env).join(".config"),
    };
    config_home.join("autostart")
}

/// Full path to the autostart `.desktop` file:
/// `~/.config/autostart/devtoolbox.desktop` (XDG-relative, see
/// [`autostart_dir_with_env`]).
///
/// Not yet called from production code (`register`/`unregister`/
/// `is_registered` each build the path via their own `*_with_env`
/// variant) — kept public for the manual real-`$HOME` verification tests
/// below and as a natural extension point for a future "reveal autostart
/// file" UI action.
#[allow(dead_code)]
pub fn autostart_file_path() -> PathBuf {
    autostart_file_path_with_env(&std_env_lookup)
}

fn autostart_file_path_with_env(env: &EnvLookup) -> PathBuf {
    autostart_dir_with_env(env).join(DESKTOP_FILE_NAME)
}

/// Build the `.desktop` entry content per the freedesktop autostart spec,
/// pointing `Exec=` at `exec_path` (expected to be an absolute path to the
/// running executable).
fn desktop_entry_contents(exec_path: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name={APP_DISPLAY_NAME}\n\
         Comment=Launch {APP_DISPLAY_NAME} at login\n\
         Exec={exec_path}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// Register DevToolBox to launch at login: write `devtoolbox.desktop` into
/// the XDG autostart directory, creating the directory if needed.
///
/// On any failure (unresolvable current-executable path, unwritable
/// directory, I/O error), logs a `log::warn!` and returns `Err` — never
/// panics. Overwrites any pre-existing `devtoolbox.desktop` at the target
/// path, making this idempotent (matches the idempotency approach already
/// used by the Windows Run-key registration in
/// `crate::windows::registry`).
pub fn register() -> io::Result<()> {
    register_with_env(&std_env_lookup)
}

fn register_with_env(env: &EnvLookup) -> io::Result<()> {
    let exec_path = std::env::current_exe().map_err(|e| {
        log::warn!("autostart register: could not resolve current executable path: {e}");
        e
    })?;

    let dir = autostart_dir_with_env(env);
    if let Err(e) = fs::create_dir_all(&dir) {
        log::warn!("autostart register: could not create directory {dir:?}: {e}");
        return Err(e);
    }

    let file_path = dir.join(DESKTOP_FILE_NAME);
    let contents = desktop_entry_contents(&exec_path.display().to_string());
    if let Err(e) = fs::write(&file_path, contents) {
        log::warn!("autostart register: could not write {file_path:?}: {e}");
        return Err(e);
    }

    log::info!("autostart register: wrote {file_path:?}");
    Ok(())
}

/// Remove DevToolBox's autostart `.desktop` file, if present.
///
/// Idempotent: a missing file is not an error (already-unregistered is a
/// success state). On any other failure (e.g. an unwritable directory),
/// logs a `log::warn!` and returns `Err` — never panics.
pub fn unregister() -> io::Result<()> {
    unregister_with_env(&std_env_lookup)
}

fn unregister_with_env(env: &EnvLookup) -> io::Result<()> {
    let file_path = autostart_file_path_with_env(env);
    match fs::remove_file(&file_path) {
        Ok(()) => {
            log::info!("autostart unregister: removed {file_path:?}");
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            log::warn!("autostart unregister: could not remove {file_path:?}: {e}");
            Err(e)
        }
    }
}

/// `true` iff the autostart `.desktop` file currently exists.
pub fn is_registered() -> bool {
    is_registered_with_env(&std_env_lookup)
}

fn is_registered_with_env(env: &EnvLookup) -> bool {
    autostart_file_path_with_env(env).is_file()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an env-lookup closure from a fixed set of pairs — variables not
    /// listed behave as unset (`None`), independent of the real process env.
    fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    /// A per-test isolated directory under the real temp dir (never the
    /// real `$HOME`), so these tests cannot touch the developer's actual
    /// `~/.config/autostart`.
    fn isolated_dir(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devtoolbox-autostart-test-{test_name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("failed to create isolated test dir");
        dir
    }

    #[test]
    fn autostart_dir_uses_xdg_config_home_when_set() {
        let env = env_map(&[
            ("XDG_CONFIG_HOME", "/custom/config"),
            ("HOME", "/home/someone"),
        ]);
        assert_eq!(
            autostart_dir_with_env(&env),
            PathBuf::from("/custom/config/autostart")
        );
    }

    #[test]
    fn autostart_dir_falls_back_to_home_config_when_xdg_unset() {
        let env = env_map(&[("HOME", "/home/someone")]);
        assert_eq!(
            autostart_dir_with_env(&env),
            PathBuf::from("/home/someone/.config/autostart")
        );
    }

    #[test]
    fn autostart_file_path_appends_desktop_file_name() {
        let env = env_map(&[("HOME", "/home/someone")]);
        assert_eq!(
            autostart_file_path_with_env(&env),
            PathBuf::from("/home/someone/.config/autostart/devtoolbox.desktop")
        );
    }

    #[test]
    fn desktop_entry_contents_is_spec_valid() {
        let contents = desktop_entry_contents("/usr/bin/devtoolbox");
        assert!(contents.starts_with("[Desktop Entry]\n"));
        assert!(contents.contains("Type=Application\n"));
        assert!(contents.contains("Name=DevToolBox\n"));
        assert!(contents.contains("Exec=/usr/bin/devtoolbox\n"));
        assert!(contents.contains("X-GNOME-Autostart-enabled=true\n"));
    }

    /// Acceptance criterion: registering writes a real, readable
    /// `.desktop` file into an isolated `XDG_CONFIG_HOME/autostart`
    /// directory, and it round-trips through `is_registered`/`unregister`.
    #[test]
    fn register_writes_readable_desktop_file_then_unregister_removes_it() {
        let dir = isolated_dir("register-roundtrip");
        let env = env_map(&[("XDG_CONFIG_HOME", dir.to_str().unwrap())]);

        assert!(!is_registered_with_env(&env), "must not pre-exist");

        register_with_env(&env).expect("register should succeed against a writable directory");

        let file_path = autostart_file_path_with_env(&env);
        assert!(file_path.is_file(), "expected {file_path:?} to exist");
        let contents = fs::read_to_string(&file_path).expect("file should be readable");
        assert!(contents.contains("[Desktop Entry]"));
        assert!(contents.contains("Type=Application"));
        assert!(contents.contains("Exec="));
        assert!(contents.contains("X-GNOME-Autostart-enabled=true"));
        assert!(is_registered_with_env(&env));

        unregister_with_env(&env).expect("unregister should succeed");
        assert!(!file_path.exists(), "file should be gone after unregister");
        assert!(!is_registered_with_env(&env));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unregister_of_never_registered_path_is_ok_not_error() {
        let dir = isolated_dir("unregister-missing");
        let env = env_map(&[("XDG_CONFIG_HOME", dir.to_str().unwrap())]);

        assert!(!is_registered_with_env(&env));
        let result = unregister_with_env(&env);
        assert!(
            result.is_ok(),
            "unregistering an already-absent entry must be Ok (idempotent), got {result:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Acceptance criterion: a write failure (read-only autostart-parent
    /// directory) must not panic. `register` returns `Err` (and logs a
    /// warning); it must return cleanly rather than aborting the process.
    ///
    /// Skipped when Unix permission bits are not actually being enforced
    /// (e.g. running as `root`, which bypasses them) — detected empirically
    /// by probing whether a write into the chmod'd directory is still
    /// possible, rather than special-casing uid 0, so the test degrades
    /// gracefully under any permission-bypassing environment.
    #[test]
    fn register_on_read_only_directory_logs_and_returns_err_without_panicking() {
        let dir = isolated_dir("register-readonly");
        // `dir` itself plays the role of "$XDG_CONFIG_HOME" — making *it*
        // read-only (not just the not-yet-created "autostart" subdirectory)
        // means `create_dir_all` cannot create the "autostart" child, which
        // is the failure this criterion targets (an unwritable
        // `~/.config/autostart`-equivalent location).
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o500); // r-x------: no write permission.
        fs::set_permissions(&dir, perms).expect("failed to chmod test dir read-only");

        if fs::write(dir.join("probe"), b"x").is_ok() {
            eprintln!(
                "skipping: permission bits are not enforced in this environment \
                 (e.g. running as root); cannot exercise a write-failure path"
            );
            let mut perms = fs::metadata(&dir).unwrap().permissions();
            perms.set_mode(0o700);
            let _ = fs::set_permissions(&dir, perms);
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        let env = env_map(&[("XDG_CONFIG_HOME", dir.to_str().unwrap())]);

        // The important assertion is simply that this call returns instead
        // of panicking; std::panic::catch_unwind makes that explicit.
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| register_with_env(&env)));
        assert!(
            outcome.is_ok(),
            "register_with_env must not panic on a write failure"
        );
        let result = outcome.unwrap();
        assert!(
            result.is_err(),
            "register against a read-only directory is expected to fail with an Err"
        );

        // Restore write permission so the isolated dir can be cleaned up.
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&dir, perms).expect("failed to restore perms for cleanup");
        let _ = fs::remove_dir_all(&dir);
    }

    use std::os::unix::fs::PermissionsExt;

    /// Exercise the real `std::env`-backed public functions at least once so
    /// the wiring from `std_env_lookup` through to each `*_with_env`
    /// function actually compiles and runs end-to-end, against a temporary
    /// override of `HOME`/`XDG_CONFIG_HOME` is NOT done here on purpose:
    /// `autostart_file_path()` and `is_registered()` are read-only/pure
    /// with respect to the real filesystem (no write), so calling them
    /// against the real environment is safe and touches no real files.
    #[test]
    fn public_read_only_functions_do_not_panic() {
        let _ = autostart_file_path();
        let _ = is_registered();
    }

    // -----------------------------------------------------------------
    // Manual, real-$HOME verification (NOT part of the automated suite)
    // -----------------------------------------------------------------
    //
    // The two tests below are `#[ignore]`d: they deliberately operate
    // against the REAL process environment (no env injection), so they
    // write to / remove the developer's actual
    // `~/.config/autostart/devtoolbox.desktop`. They exist purely to let a
    // human run `cargo test -- --ignored <name>` once, inspect the real
    // file with independent tools (e.g. `cat`, a `configparser` parse) in
    // between the two runs, and confirm real desktop-level behavior. They
    // must never run as part of `cargo test --workspace`.

    #[test]
    #[ignore = "writes to the real ~/.config/autostart/devtoolbox.desktop; run manually for one-off desktop verification"]
    fn manual_register_writes_real_autostart_file() {
        register().expect("register should succeed under a normal writable $HOME");
    }

    #[test]
    #[ignore = "removes the real ~/.config/autostart/devtoolbox.desktop written by the sibling manual test"]
    fn manual_unregister_removes_real_autostart_file() {
        unregister().expect("unregister should succeed");
    }
}
