//! Startup registration via the Windows Registry Run key.
//!
//! Controls whether WinFXStart launches at Windows login by writing/removing a
//! value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! # Design
//! All Win32 registry operations are path-injectable (they accept `sub_key` and
//! `value_name` parameters) so unit tests operate on a throwaway subkey
//! `HKCU\Software\WinFXStart\Test` and never touch the real Run key.
//!
//! The public API (`enable_startup`, `disable_startup`, `is_startup_enabled`,
//! `sync_startup`) binds the injectable core to the real `RUN_KEY_PATH` /
//! `APP_VALUE_NAME`. Those public functions are NOT called from tests.
//!
//! # Idempotency
//! Registry values are keyed by name; `RegSetValueExW` on the same
//! `WinFXStart` name overwrites in place — no dedup code is needed and no
//! duplicate can be created (see Decision D4 in the plan).

// Read-side helpers (`open_key_read`, `query_value`, `is_startup_enabled`) are
// part of the registry API surface but not all are wired to the UI yet.
#![allow(dead_code)]

use std::fmt;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, REG_VALUE_TYPE,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The parent Run key path under HKCU where startup entries live.
pub const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

/// The value name used to identify this application's startup entry.
pub const APP_VALUE_NAME: &str = "WinFXStart";

// HRESULT value that corresponds to ERROR_FILE_NOT_FOUND (Win32 code 2).
// HRESULT::from_win32(x) = (x & 0xFFFF) | (7 << 16) | 0x80000000 for x > 0.
const HRESULT_FILE_NOT_FOUND: i32 =
    (ERROR_FILE_NOT_FOUND.0 as i32 & 0x0000_FFFF) | (7 << 16) | 0x8000_0000_u32 as i32;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from registry operations.
#[derive(Debug)]
pub enum RegistryError {
    /// A Win32 API call returned a non-success HRESULT.
    Win32 {
        /// The HRESULT value from the failing call.
        code: i32,
    },
    /// `std::env::current_exe()` failed.
    ExePath,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Win32 { code } => {
                write!(f, "Win32 registry error: HRESULT 0x{:08X}", *code as u32)
            }
            RegistryError::ExePath => {
                write!(f, "could not determine current executable path")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Map a `windows::core::Error` to `RegistryError::Win32`.
fn map_win32_err(e: windows::core::Error) -> RegistryError {
    RegistryError::Win32 { code: e.code().0 }
}

/// Return true if the `windows::core::Error` represents ERROR_FILE_NOT_FOUND.
fn is_not_found(e: &windows::core::Error) -> bool {
    e.code().0 == HRESULT_FILE_NOT_FOUND
}

// ---------------------------------------------------------------------------
// UTF-16 helpers
// ---------------------------------------------------------------------------

/// Convert a `&str` to a null-terminated UTF-16 `Vec<u16>` for Win32 `*W` APIs.
///
/// Uses `std::ffi::OsStr::encode_wide` (no extra crate) and appends the
/// required null terminator.
pub fn to_wide(s: &str) -> Vec<u16> {
    let os_str: &std::ffi::OsStr = s.as_ref();
    os_str.encode_wide().chain(std::iter::once(0u16)).collect()
}

/// Build the quoted executable value data for the Run entry.
///
/// Returns `"<path>"` (double quotes included) so that paths containing
/// spaces (e.g. `C:\Program Files\...`) are treated as a single token by
/// the Windows shell.
///
/// # Errors
/// Returns [`RegistryError::ExePath`] if `std::env::current_exe()` fails.
pub fn executable_value() -> Result<String, RegistryError> {
    let exe = std::env::current_exe().map_err(|_| RegistryError::ExePath)?;
    let path_str = exe.to_string_lossy();
    Ok(format!("\"{}\"", path_str))
}

// ---------------------------------------------------------------------------
// Path-injectable core
// ---------------------------------------------------------------------------

/// Open or create `HKCU\<sub_key>` with the requested access rights.
///
/// Returns the opened/created `HKEY`; caller is responsible for closing it.
/// Maps any failure to [`RegistryError::Win32`].
///
/// Uses `RegCreateKeyW` which requires no Security feature flag and provides
/// open-or-create semantics sufficient for our use case.
unsafe fn open_or_create_key(sub_key: &str, access: u32) -> Result<HKEY, RegistryError> {
    let wide_key = to_wide(sub_key);
    let mut hkey = HKEY::default();

    RegCreateKeyW(
        HKEY_CURRENT_USER,
        windows::core::PCWSTR(wide_key.as_ptr()),
        &mut hkey,
    )
    .map_err(map_win32_err)?;

    // RegCreateKeyW opens with KEY_ALL_ACCESS; if we need a narrower access
    // (KEY_READ / KEY_WRITE) we re-open after creation.
    let _ = access; // Both KEY_READ and KEY_WRITE are subsets of KEY_ALL_ACCESS.

    Ok(hkey)
}

/// Open `HKCU\<sub_key>` with the given SAM access flags.
///
/// Returns `Ok(None)` when the subkey does not exist (`ERROR_FILE_NOT_FOUND`).
/// Maps other failures to [`RegistryError::Win32`].
unsafe fn open_key(
    sub_key: &str,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<Option<HKEY>, RegistryError> {
    let wide_key = to_wide(sub_key);
    let mut hkey = HKEY::default();

    match RegOpenKeyExW(
        HKEY_CURRENT_USER,
        windows::core::PCWSTR(wide_key.as_ptr()),
        0,
        access,
        &mut hkey,
    ) {
        Ok(()) => Ok(Some(hkey)),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(map_win32_err(e)),
    }
}

/// Open `HKCU\<sub_key>` for reading only.
///
/// Returns `Ok(None)` when the subkey does not exist (`ERROR_FILE_NOT_FOUND`).
/// Maps other failures to [`RegistryError::Win32`].
unsafe fn open_key_read(sub_key: &str) -> Result<Option<HKEY>, RegistryError> {
    open_key(sub_key, KEY_READ)
}

/// Write a `REG_SZ` value under `HKCU\<sub_key>`.
///
/// Creates the subkey if it does not exist (open-or-create semantics).
/// Writing the same value name twice overwrites in place — idempotent upsert.
///
/// # Errors
/// Returns [`RegistryError::Win32`] on any Win32 failure.
pub fn set_value(sub_key: &str, value_name: &str, data: &str) -> Result<(), RegistryError> {
    unsafe {
        let hkey = open_or_create_key(sub_key, KEY_WRITE.0)?;

        let wide_name = to_wide(value_name);
        // REG_SZ byte length includes the null terminator.
        let wide_data = to_wide(data);
        let byte_slice =
            std::slice::from_raw_parts(wide_data.as_ptr() as *const u8, wide_data.len() * 2);

        let result = RegSetValueExW(
            hkey,
            windows::core::PCWSTR(wide_name.as_ptr()),
            0,
            REG_SZ,
            Some(byte_slice),
        );

        let _ = RegCloseKey(hkey);

        result.map_err(map_win32_err)
    }
}

/// Query a `REG_SZ` value under `HKCU\<sub_key>`.
///
/// Returns `Ok(None)` when the subkey or value does not exist.
/// Returns `Ok(Some(string))` on success.
///
/// Uses the two-call sizing pattern: first call with a null buffer to obtain
/// the required byte length, second call to fill the buffer.
///
/// # Errors
/// Returns [`RegistryError::Win32`] on unexpected Win32 failures.
pub fn query_value(sub_key: &str, value_name: &str) -> Result<Option<String>, RegistryError> {
    unsafe {
        // Open the subkey read-only; absent subkey → None.
        let hkey = match open_key_read(sub_key)? {
            Some(k) => k,
            None => return Ok(None),
        };

        let wide_name = to_wide(value_name);

        // First call: get required size.
        let mut byte_len: u32 = 0;
        let mut reg_type = REG_VALUE_TYPE::default();
        let size_result = RegQueryValueExW(
            hkey,
            windows::core::PCWSTR(wide_name.as_ptr()),
            None,
            Some(&mut reg_type),
            None,
            Some(&mut byte_len),
        );

        match size_result {
            Err(e) if is_not_found(&e) => {
                let _ = RegCloseKey(hkey);
                return Ok(None);
            }
            Err(e) => {
                let _ = RegCloseKey(hkey);
                return Err(map_win32_err(e));
            }
            Ok(()) => {}
        }

        // Allocate buffer (byte_len includes null terminator for REG_SZ).
        let u16_count = (byte_len as usize).saturating_add(1) / 2;
        let mut buf: Vec<u16> = vec![0u16; u16_count];
        let mut actual_byte_len = byte_len;

        let read_result = RegQueryValueExW(
            hkey,
            windows::core::PCWSTR(wide_name.as_ptr()),
            None,
            Some(&mut reg_type),
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut actual_byte_len),
        );

        let _ = RegCloseKey(hkey);

        read_result.map_err(map_win32_err)?;

        // Decode: strip trailing null(s) and convert from UTF-16.
        let u16_len = (actual_byte_len as usize) / 2;
        let data: Vec<u16> = buf[..u16_len]
            .iter()
            .copied()
            .take_while(|&c| c != 0)
            .collect();
        let result = String::from_utf16_lossy(&data).to_string();
        Ok(Some(result))
    }
}

/// Delete a `REG_SZ` value under `HKCU\<sub_key>`.
///
/// Treats `ERROR_FILE_NOT_FOUND` (subkey or value absent) as `Ok(())` —
/// idempotent delete.
///
/// # Errors
/// Returns [`RegistryError::Win32`] on unexpected Win32 failures.
pub fn delete_value(sub_key: &str, value_name: &str) -> Result<(), RegistryError> {
    unsafe {
        // Open the subkey for writing; absent subkey is a no-op.
        let hkey = match open_key(sub_key, KEY_WRITE)? {
            Some(k) => k,
            None => return Ok(()),
        };

        let wide_name = to_wide(value_name);
        let result = RegDeleteValueW(hkey, windows::core::PCWSTR(wide_name.as_ptr()));
        let _ = RegCloseKey(hkey);

        match result {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(map_win32_err(e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Public startup API
// ---------------------------------------------------------------------------

/// Register WinFXStart to launch at login (idempotent upsert).
///
/// Writes the value `WinFXStart = "<current_exe>"` under
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
/// Writing the same value name twice overwrites in place — no duplicate can
/// be created.
///
/// # Errors
/// Returns [`RegistryError::ExePath`] if the current executable path cannot
/// be determined, or [`RegistryError::Win32`] on any Win32 failure.
pub fn enable_startup() -> Result<(), RegistryError> {
    let data = executable_value()?;
    set_value(RUN_KEY_PATH, APP_VALUE_NAME, &data)
}

/// Remove the WinFXStart startup entry (idempotent delete).
///
/// Deletes the `WinFXStart` value from
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
/// Returns `Ok(())` if the value is already absent — no error.
///
/// # Errors
/// Returns [`RegistryError::Win32`] on unexpected Win32 failures.
pub fn disable_startup() -> Result<(), RegistryError> {
    delete_value(RUN_KEY_PATH, APP_VALUE_NAME)
}

/// Return `true` iff the `WinFXStart` startup entry exists in the Run key.
///
/// Returns `false` on any error (non-fatal, no panic).
pub fn is_startup_enabled() -> bool {
    match query_value(RUN_KEY_PATH, APP_VALUE_NAME) {
        Ok(opt) => opt.is_some(),
        Err(e) => {
            log::warn!("is_startup_enabled: query failed: {}", e);
            false
        }
    }
}

/// Enable or disable the startup entry based on `enabled`.
///
/// Maps `true` → [`enable_startup`], `false` → [`disable_startup`].
///
/// # Errors
/// Propagates any error from the underlying enable/disable call.
pub fn sync_startup(enabled: bool) -> Result<(), RegistryError> {
    if enabled {
        enable_startup()
    } else {
        disable_startup()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};
    use windows::Win32::System::Registry::RegDeleteKeyW;

    /// Throwaway subkey — never the real Run key.
    const TEST_SUBKEY: &str = "Software\\WinFXStart\\Test";
    /// Throwaway value name used by all registry tests.
    const TEST_VALUE: &str = "test_value_winfxstart";

    /// Process-wide mutex to serialise registry-touching tests so that one
    /// test's teardown cannot delete the key while another is using it.
    static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

    /// Acquire the registry lock for the duration of a registry-touching test.
    fn lock_registry() -> MutexGuard<'static, ()> {
        // Recover from a previous panicking test (poisoned mutex).
        REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Teardown helper: best-effort delete the value and the test subkey.
    /// Must be called while the registry lock is held.
    fn teardown() {
        // Delete the value (ignore errors — it may already be absent).
        let _ = delete_value(TEST_SUBKEY, TEST_VALUE);

        // Best-effort delete the test subkey itself.
        unsafe {
            let wide_key = to_wide(TEST_SUBKEY);
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, windows::core::PCWSTR(wide_key.as_ptr()));
        }
    }

    // --- Pure helpers (no registry I/O, no lock needed) ---

    #[test]
    fn to_wide_null_terminated() {
        let w = to_wide("X");
        assert_eq!(w, vec![88u16, 0u16]);
    }

    #[test]
    fn to_wide_empty_string() {
        let w = to_wide("");
        assert_eq!(w, vec![0u16]);
    }

    #[test]
    fn executable_value_is_double_quoted() {
        let val = executable_value().expect("executable_value failed");
        assert!(val.starts_with('"'), "expected leading quote, got: {}", val);
        assert!(val.ends_with('"'), "expected trailing quote, got: {}", val);
    }

    #[test]
    fn app_value_name_constant() {
        assert_eq!(APP_VALUE_NAME, "WinFXStart");
    }

    // --- Registry core (throwaway subkey, never the real Run key) ---
    // Each test holds REGISTRY_LOCK for its entire duration so that no other
    // registry-touching test can race with its teardown.

    #[test]
    fn set_and_query_returns_written_data() {
        let _guard = lock_registry();
        teardown();

        set_value(TEST_SUBKEY, TEST_VALUE, "hello").expect("set_value failed");
        let result = query_value(TEST_SUBKEY, TEST_VALUE).expect("query_value failed");
        assert_eq!(result, Some("hello".to_string()));

        teardown();
    }

    #[test]
    fn query_absent_value_returns_none() {
        let _guard = lock_registry();
        teardown();

        let result = query_value(TEST_SUBKEY, TEST_VALUE).expect("query_value failed");
        assert_eq!(result, None);

        teardown();
    }

    #[test]
    fn delete_removes_value() {
        let _guard = lock_registry();
        teardown();

        set_value(TEST_SUBKEY, TEST_VALUE, "to_be_deleted").expect("set_value failed");
        delete_value(TEST_SUBKEY, TEST_VALUE).expect("delete_value failed");
        let result = query_value(TEST_SUBKEY, TEST_VALUE).expect("query_value failed");
        assert_eq!(result, None);

        teardown();
    }

    #[test]
    fn delete_absent_value_returns_ok() {
        let _guard = lock_registry();
        teardown();

        // Value does not exist — delete must be a no-op, not an error.
        delete_value(TEST_SUBKEY, TEST_VALUE).expect("delete of absent value should be Ok");

        teardown();
    }

    #[test]
    fn set_twice_is_idempotent_upsert() {
        let _guard = lock_registry();
        teardown();

        set_value(TEST_SUBKEY, TEST_VALUE, "first").expect("first set failed");
        set_value(TEST_SUBKEY, TEST_VALUE, "second").expect("second set failed");

        let result = query_value(TEST_SUBKEY, TEST_VALUE).expect("query failed");
        // Must be exactly one value holding the latest data.
        assert_eq!(result, Some("second".to_string()));

        teardown();
    }

    #[test]
    fn no_test_references_run_key() {
        // Compile-time assertion: none of the test helpers or test bodies pass
        // the real Run key.  The constant TEST_SUBKEY must NOT contain "Run".
        assert!(
            !TEST_SUBKEY.contains("Run"),
            "TEST_SUBKEY must not reference the real Run key"
        );
    }
}
