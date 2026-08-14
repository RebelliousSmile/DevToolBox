//! Automations view shell — closes the Part 2 plan gap flagged by Phase 2
//! (`egui_app.rs`'s own doc comment / the plan's `## Amendments` section):
//! the plan's Feature summary lists an "Automations view" but no Phase 1-4
//! task previously assigned it to a phase.
//!
//! It defines the row shape and the cross-platform `fetch()` entry point.
//! Windows uses `Get-ScheduledTask` via PowerShell; Linux (since Part 3
//! Phase 2) uses `crate::linux::automations::fetch()`, a real systemd
//! `list-timers` data source (see that module for the field-mapping
//! rationale).
//!
//! # Why this duplicates PowerShell-fetch logic instead of reusing
//! `src/ui/app.rs::load_scheduled_tasks`
//!
//! That function (and `crate::windows::process::ScheduledTask`, whose shape
//! [`AutomationRow`] mirrors field-for-field) lives in code this phase
//! deliberately leaves untouched: `app.rs` is slated for deletion in Phase 4
//! and is Windows-only, and this session has no `x86_64-pc-windows-gnu`
//! rustup target installed (`rustup target add` timed out — no/slow network),
//! so no edit to Windows-only code paths can be compile-verified here.
//! Duplicating the small, self-contained fetch script into this new,
//! from-scratch file keeps the (already Windows-only, already
//! compile-unverifiable) blast radius limited to code nothing else depends
//! on, rather than risking a working file.

use serde::Deserialize;

/// One row of the Automations view — mirrors
/// `crate::windows::process::ScheduledTask` field-for-field (name,
/// category, next run, state, author), redeclared here rather than reused
/// because that struct lives in the `#[cfg(windows)]`-gated `windows::process`
/// module and is unavailable on Linux.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AutomationRow {
    pub name: String,
    pub category: String,
    pub next_run: String,
    pub state: String,
    pub author: String,
}

/// Fetch the current list of scheduled automations for this OS: real
/// `Get-ScheduledTask` results on Windows, real `systemctl list-timers`
/// results on Linux (`crate::linux::automations::fetch`).
pub fn fetch() -> Result<Vec<AutomationRow>, String> {
    fetch_impl()
}

#[cfg(windows)]
fn fetch_impl() -> Result<Vec<AutomationRow>, String> {
    // Mirrors `src/ui/app.rs::load_scheduled_tasks`'s script shape (build a
    // `[pscustomobject]` per task from `Get-ScheduledTask` +
    // `Get-ScheduledTaskInfo`, then `ConvertTo-Json -Compress`), duplicated
    // here per the module doc comment above rather than calling into
    // `app.rs` directly.
    const SCRIPT: &str = r#"
$tasks = Get-ScheduledTask | ForEach-Object {
    $info = $_ | Get-ScheduledTaskInfo
    [pscustomobject]@{
        Name = $_.TaskName
        Category = $_.TaskPath
        NextRun = if ($info.NextRunTime) { $info.NextRunTime.ToString() } else { "" }
        State = $_.State.ToString()
        Author = $_.Author
    }
}
$tasks | ConvertTo-Json -Compress
"#;

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", SCRIPT])
        .output()
        .map_err(|error| format!("powershell introuvable: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    serde_json::from_str::<Vec<AutomationRow>>(trimmed)
        .or_else(|_| serde_json::from_str::<AutomationRow>(trimmed).map(|row| vec![row]))
        .map_err(|error| format!("réponse PowerShell inattendue: {error}"))
}

#[cfg(target_os = "linux")]
fn fetch_impl() -> Result<Vec<AutomationRow>, String> {
    crate::linux::automations::fetch()
}

/// Neither Windows nor Linux: no data source is wired for this OS. Returns
/// an explicit empty list rather than failing — the view already renders an
/// unambiguous "no automations found" placeholder for the empty case.
#[cfg(not(any(windows, target_os = "linux")))]
fn fetch_impl() -> Result<Vec<AutomationRow>, String> {
    Ok(Vec::new())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_row_deserializes_pascal_case_json() {
        let json = r#"{"Name":"Backup","Category":"\\Custom\\","NextRun":"2026-08-06 02:00:00","State":"Ready","Author":"SYSTEM"}"#;
        let row: AutomationRow = serde_json::from_str(json).unwrap();
        assert_eq!(row.name, "Backup");
        assert_eq!(row.category, "\\Custom\\");
        assert_eq!(row.next_run, "2026-08-06 02:00:00");
        assert_eq!(row.state, "Ready");
        assert_eq!(row.author, "SYSTEM");
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    #[test]
    fn fetch_returns_empty_vec_on_unsupported_os() {
        let rows = fetch().expect("fetch must not fail on an unsupported OS");
        assert!(rows.is_empty(), "no data source is wired for this OS");
    }

    /// On Linux, `fetch()` now goes through the real
    /// `crate::linux::automations::fetch()` (systemd `list-timers`) data
    /// source (Part 3 Phase 2). This system has real timers configured, so
    /// this asserts real, non-empty, populated results rather than the old
    /// "always Ok(vec![])" stub behavior — see
    /// `crate::linux::automations`'s own tests for the detailed real- and
    /// fixture-based verification of this data source.
    #[cfg(target_os = "linux")]
    #[test]
    fn fetch_returns_real_populated_rows_on_linux() {
        let rows = fetch().expect("fetch must not fail on this real Ubuntu LTS system");
        assert!(
            !rows.is_empty(),
            "expected real systemd timers on this reference system"
        );
        assert!(
            rows.iter().all(|row| !row.name.is_empty()),
            "every row must have a populated name"
        );
    }
}
