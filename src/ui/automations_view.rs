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
//! # Scope: user-created automations only
//!
//! `fetch()` excludes automations shipped by the OS/its packages —
//! `\Microsoft\...`-folder tasks on Windows, package-provided systemd units
//! on Linux (see [`is_builtin_windows_task`] and
//! `crate::linux::automations::is_package_provided_category`) — since both
//! platforms already have their own native tool for browsing *every*
//! scheduled task/timer (see [`open_native_tool`]); this view exists to
//! surface what the user (or user-installed software) actually added, not
//! to duplicate that native tool wholesale. See
//! `aidd_docs/memory/internal/decisions/automations-user-scope.md`.
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
    // `Get-ScheduledTask`'s `Author` is genuinely `$null` (not an empty
    // string) for several built-in system tasks (e.g. the ".NET Framework
    // NGEN" family) — a plain `String` field rejects that `null` outright,
    // so this maps it to `""`, matching how `NextRun` already represents
    // "nothing to show" as an empty string rather than an absent field.
    #[serde(deserialize_with = "null_to_empty_string")]
    pub author: String,
}

fn null_to_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// Fetch the current list of user-created automations for this OS: real
/// `Get-ScheduledTask` results on Windows, real `systemctl list-timers`
/// results on Linux (`crate::linux::automations::fetch`) — in both cases
/// filtered to exclude OS/package-provided entries (see the module doc's
/// "Scope" section).
pub fn fetch() -> Result<Vec<AutomationRow>, String> {
    fetch_impl()
}

/// `true` when `category` (the Task Scheduler folder / `TaskPath`, e.g.
/// `\Microsoft\Windows\.NET Framework\`) is one of the built-in Microsoft
/// folders rather than a user- or third-party-software-installed task.
/// Deliberately not `#[cfg(windows)]`-gated — it's pure string logic with
/// no Windows API dependency, so it stays unit-testable on any host
/// (including this Linux dev machine) even though its only caller
/// (`fetch_impl` below) is Windows-only, hence the `allow(dead_code)` on
/// non-Windows builds.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_builtin_windows_task(category: &str) -> bool {
    category
        .trim_start_matches('\\')
        .to_ascii_lowercase()
        .starts_with("microsoft\\")
}

/// Open this OS's closest native tool for browsing *all* scheduled
/// automations (not just the user-created ones this view scopes to): the
/// Task Scheduler GUI on Windows, a terminal pre-loaded with `systemctl
/// list-timers` on Linux — there is no single standard cross-desktop-
/// environment GUI for systemd timers, so a terminal is the honest
/// fallback rather than guessing at a GUI that may not be installed.
pub fn open_native_tool() -> Result<(), String> {
    open_native_tool_impl()
}

#[cfg(windows)]
fn open_native_tool_impl() -> Result<(), String> {
    crate::windows::process::open_task_scheduler()
}

#[cfg(target_os = "linux")]
fn open_native_tool_impl() -> Result<(), String> {
    crate::linux::automations::open_native_tool()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_native_tool_impl() -> Result<(), String> {
    Err("Aucun outil natif disponible sur cet OS.".to_string())
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

    let rows = serde_json::from_str::<Vec<AutomationRow>>(trimmed)
        .or_else(|_| serde_json::from_str::<AutomationRow>(trimmed).map(|row| vec![row]))
        .map_err(|error| format!("réponse PowerShell inattendue: {error}"))?;

    Ok(rows
        .into_iter()
        .filter(|row| !is_builtin_windows_task(&row.category))
        .collect())
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

    #[test]
    fn automation_row_maps_a_null_author_to_an_empty_string() {
        let json = r#"{"Name":".NET Framework NGEN v4.0.30319","Category":"\\Microsoft\\Windows\\.NET Framework\\","NextRun":"","State":"Ready","Author":null}"#;
        let row: AutomationRow = serde_json::from_str(json).unwrap();
        assert_eq!(row.author, "");
    }

    #[test]
    fn a_json_array_mixing_null_and_populated_authors_deserializes_as_a_vec() {
        let json = r#"[{"Name":"A","Category":"\\","NextRun":"","State":"Ready","Author":null},{"Name":"B","Category":"\\","NextRun":"","State":"Ready","Author":"SYSTEM"}]"#;
        let rows: Vec<AutomationRow> = serde_json::from_str(json).unwrap();
        assert_eq!(rows[0].author, "");
        assert_eq!(rows[1].author, "SYSTEM");
    }

    // --- is_builtin_windows_task (user-scope filter) ----------------------

    #[test]
    fn builtin_windows_task_under_microsoft_folder_is_detected() {
        assert!(is_builtin_windows_task("\\Microsoft\\Windows\\.NET Framework\\"));
    }

    #[test]
    fn builtin_windows_task_detection_is_case_insensitive() {
        assert!(is_builtin_windows_task("\\MICROSOFT\\Windows\\UpdateOrchestrator\\"));
    }

    #[test]
    fn custom_folder_task_is_not_builtin() {
        assert!(!is_builtin_windows_task("\\Custom\\"));
    }

    #[test]
    fn root_folder_task_is_not_builtin() {
        assert!(!is_builtin_windows_task("\\"));
    }

    #[test]
    fn third_party_software_task_is_not_builtin() {
        assert!(!is_builtin_windows_task("\\GoogleUpdate\\"));
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    #[test]
    fn fetch_returns_empty_vec_on_unsupported_os() {
        let rows = fetch().expect("fetch must not fail on an unsupported OS");
        assert!(rows.is_empty(), "no data source is wired for this OS");
    }

    /// On Linux, `fetch()` goes through the real
    /// `crate::linux::automations::fetch()` (systemd `list-timers`) data
    /// source (Part 3 Phase 2), now filtered to user-created automations
    /// only. This reference machine's timers are entirely package-provided
    /// (verified manually: every entry's `FragmentPath` sits under
    /// `/lib/systemd/system`), so the *correct* result here is an empty
    /// list — this asserts that filtering, not raw non-emptiness (which
    /// `crate::linux::automations`'s own tests already cover pre-filter).
    /// A non-empty result is still tolerated (a machine with a real local
    /// unit under `/etc/systemd/system` would legitimately produce one),
    /// but every returned row must have cleared the filter.
    #[cfg(target_os = "linux")]
    #[test]
    fn fetch_excludes_package_provided_timers_on_linux() {
        let rows = fetch().expect("fetch must not fail on this real Ubuntu LTS system");
        assert!(
            rows.iter().all(|row| !row.name.is_empty()),
            "every row must have a populated name"
        );
        assert!(
            rows.iter()
                .all(|row| !crate::linux::automations::is_package_provided_category(&row.category)),
            "expected only user-created automations, got a package-provided one: {:?}",
            rows.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        assert!(
            !rows.iter().any(|row| row.name == "apt-daily.timer"),
            "apt-daily.timer is package-provided and must be filtered out"
        );
    }
}
