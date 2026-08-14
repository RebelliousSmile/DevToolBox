//! systemd Automations data source — the Linux counterpart to
//! `automations_view::fetch_impl`'s Windows `Get-ScheduledTask` PowerShell
//! path.
//!
//! Runs `systemctl list-timers --all --output=json`, deserializes the
//! result defensively (per the Part 3 plan's Risk register: "`systemctl
//! list-timers --output=json` output shape can vary across systemd
//! versions" — mitigated with `#[serde(default)]` on every field plus a
//! fixture-based unit test using a real JSON sample captured on this
//! Ubuntu 22.04.5 LTS system's systemd 249), and enriches each row with
//! `systemctl show <unit>` for fields `list-timers` doesn't provide
//! (description, unit source, live active/sub state).
//!
//! # Field mapping to [`AutomationRow`]
//!
//! | `AutomationRow` field | Source |
//! | --- | --- |
//! | `name` | `list-timers`' `unit` (e.g. `"apt-daily.timer"`) |
//! | `category` | `systemctl show -p FragmentPath`'s parent directory (e.g. `"/lib/systemd/system"`) — the closest systemd analogue to Windows Task Scheduler's folder-based `TaskPath`; empty if unavailable (e.g. a transient/generated unit with no on-disk unit file) |
//! | `next_run` | `list-timers`' `next` (microseconds since the Unix epoch), formatted as a UTC date/time string; `"n/a"` when absent or zero (mirrors `systemctl list-timers`'s own text-mode convention for timers with no scheduled next run) |
//! | `state` | `systemctl show -p ActiveState,SubState`, formatted `"<ActiveState> (<SubState>)"` (e.g. `"active (waiting)"`) — systemd has no single field equivalent to `Get-ScheduledTask`'s `.State` enum, so this is the closest faithful combination |
//! | `author` | `systemctl show -p Description` — per the Part 3 plan's explicit, if semantically loose, mapping instruction ("author field mapped from the unit's `Description=` or left empty"); systemd units have no true "author" concept |
//!
//! No new crate: JSON parsing uses the already-present `serde`/`serde_json`
//! dependency; timestamp formatting is done by hand (a standard
//! days-since-epoch civil-calendar conversion) rather than adding a
//! date/time crate, consistent with this Part's Stack section.

use std::collections::HashMap;
use std::process::Command;

use serde::Deserialize;

use crate::ui::automations_view::AutomationRow;

/// One row of `systemctl list-timers --output=json`. Every field is
/// `#[serde(default)]` (or itself `Option<_>`) so an unexpected shape from a
/// systemd version this wasn't tested against degrades to missing/blank
/// data instead of a hard parse failure for the *entire* list — see the
/// module doc table and the Risk register mitigation this implements.
#[derive(Debug, Clone, Deserialize, Default)]
struct TimerEntry {
    #[serde(default)]
    unit: String,
    #[serde(default)]
    next: Option<i64>,
    #[serde(default)]
    last: Option<i64>,
    #[serde(default)]
    activates: String,
}

/// Fetch the current list of systemd timers as [`AutomationRow`]s.
///
/// Returns `Err` only if `systemctl` itself cannot be invoked or its
/// `list-timers` output cannot be parsed as JSON at all (e.g. `systemctl`
/// missing from `PATH`, or a systemd version that dropped `--output=json`
/// support entirely) — per-unit enrichment failures (a `systemctl show`
/// call failing for one unit) degrade that single row's fields to empty
/// strings rather than failing the whole fetch.
pub fn fetch() -> Result<Vec<AutomationRow>, String> {
    let output = Command::new("systemctl")
        .args(["list-timers", "--all", "--output=json"])
        .output()
        .map_err(|error| format!("systemctl introuvable: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let entries: Vec<TimerEntry> = serde_json::from_str(trimmed)
        .map_err(|error| format!("réponse systemctl inattendue: {error}"))?;

    Ok(entries.into_iter().map(build_row).collect())
}

fn build_row(entry: TimerEntry) -> AutomationRow {
    let details = unit_show_details(&entry.unit);

    let category = details
        .get("FragmentPath")
        .map(|path| {
            std::path::Path::new(path)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let state = format!(
        "{} ({})",
        details.get("ActiveState").map(String::as_str).unwrap_or(""),
        details.get("SubState").map(String::as_str).unwrap_or(""),
    );

    let author = details.get("Description").cloned().unwrap_or_default();

    // `activates` (the service this timer triggers) isn't a field on
    // AutomationRow today; kept available on `entry` for future use rather
    // than discarded silently, but there's no row field to put it in per
    // the Windows-mirrored shape this module targets.
    let _ = &entry.activates;

    // When a timer has no scheduled next run (`next` is null — e.g. a
    // disabled timer, or one systemd hasn't computed a next occurrence
    // for), fall back to reporting its last run instead of a bare "n/a",
    // which is more informative and mirrors why `last` is fetched at all.
    let next_run = match entry.next {
        Some(next) if next > 0 => format_timestamp(Some(next)),
        _ => match entry.last {
            Some(last) if last > 0 => {
                format!("(dernière exécution: {})", format_timestamp(Some(last)))
            }
            _ => "n/a".to_string(),
        },
    };

    AutomationRow {
        name: entry.unit,
        category,
        next_run,
        state,
        author,
    }
}

/// Run `systemctl show <unit> -p Description -p FragmentPath -p ActiveState
/// -p SubState` and parse its `Key=Value` lines into a map. Uses the
/// default (non-`--value`) output format deliberately: `systemctl show`
/// does not guarantee it prints multiple `-p`-selected properties back in
/// the order they were requested (verified empirically on this system's
/// systemd 249 — `--value` output order did not match `-p` argument order),
/// so relying on positional `--value` output would silently mismap fields.
/// `Key=Value` lines are unambiguous regardless of ordering.
///
/// Returns an empty map (never panics, never propagates an error) if the
/// command fails to run at all — a single unit's enrichment failing must
/// not take down the whole Automations view.
fn unit_show_details(unit: &str) -> HashMap<String, String> {
    if unit.is_empty() {
        return HashMap::new();
    }

    let output = Command::new("systemctl")
        .args([
            "show",
            unit,
            "-p",
            "Description",
            "-p",
            "FragmentPath",
            "-p",
            "ActiveState",
            "-p",
            "SubState",
        ])
        .output();

    let Ok(output) = output else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Format a `next`/`last` timestamp (microseconds since the Unix epoch) as
/// a human-readable UTC date/time string, or `"n/a"` when absent or zero
/// (systemd's own text-mode `list-timers` output uses `"n/a"` for timers
/// with no last/next run — e.g. this system's `snapd.snap-repair.timer`,
/// which has never run and reports `"last":0`).
fn format_timestamp(micros: Option<i64>) -> String {
    match micros {
        Some(us) if us > 0 => unix_micros_to_utc_string(us),
        _ => "n/a".to_string(),
    }
}

fn unix_micros_to_utc_string(us: i64) -> String {
    let total_seconds = us.div_euclid(1_000_000);
    let days = total_seconds.div_euclid(86_400);
    let secs_of_day = total_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// Howard Hinnant's `civil_from_days` algorithm: converts a day count
/// (days since 1970-01-01) into a proleptic-Gregorian `(year, month, day)`.
/// <http://howardhinnant.github.io/date_algorithms.html#civil_from_days>
/// Pure integer arithmetic, no date/time crate — consistent with this
/// Part's "no new external crate" Stack decision.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- civil_from_days / format_timestamp -----------------------------------

    #[test]
    fn civil_from_days_matches_known_reference_date() {
        // 1785928048 unix seconds == 2026-08-05 11:07:28 UTC (verified
        // against `date -u -d @1785928048` and Python's
        // `datetime.utcfromtimestamp` on this machine while implementing
        // this module).
        let days = 1_785_928_048_i64.div_euclid(86_400);
        assert_eq!(civil_from_days(days), (2026, 8, 5));
    }

    #[test]
    fn format_timestamp_renders_known_reference_micros() {
        assert_eq!(
            format_timestamp(Some(1_785_928_048_456_538)),
            "2026-08-05 11:07:28 UTC"
        );
    }

    #[test]
    fn format_timestamp_none_is_n_a() {
        assert_eq!(format_timestamp(None), "n/a");
    }

    #[test]
    fn format_timestamp_zero_is_n_a() {
        // Real captured value for a never-run timer (this machine's
        // dpkg-db-backup.timer / snapd.snap-repair.timer both report
        // "last":0).
        assert_eq!(format_timestamp(Some(0)), "n/a");
    }

    #[test]
    fn format_timestamp_negative_does_not_panic() {
        // Defensive: a malformed/future systemd JSON value should never
        // panic this formatter, even if it's nonsensical.
        let outcome = std::panic::catch_unwind(|| format_timestamp(Some(-1)));
        assert!(outcome.is_ok());
    }

    // --- TimerEntry deserialization defensiveness (Risk register) -------------

    #[test]
    fn timer_entry_deserializes_minimal_shape() {
        let json = r#"{"unit":"foo.timer"}"#;
        let entries: Vec<TimerEntry> = serde_json::from_str(&format!("[{json}]")).unwrap();
        assert_eq!(entries[0].unit, "foo.timer");
        assert_eq!(entries[0].next, None);
        assert_eq!(entries[0].last, None);
        assert_eq!(entries[0].activates, "");
    }

    #[test]
    fn timer_entry_tolerates_unknown_extra_fields() {
        // Simulates a hypothetical future systemd version adding a new
        // field (`serde` ignores unknown fields by default — no
        // `deny_unknown_fields` is used anywhere in this struct).
        let json = r#"[{"unit":"foo.timer","next":123,"occurrences":5,"future_field":"x"}]"#;
        let entries: Vec<TimerEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(entries[0].unit, "foo.timer");
        assert_eq!(entries[0].next, Some(123));
    }

    #[test]
    fn timer_entry_tolerates_null_next_and_last() {
        let json = r#"[{"unit":"snapd.snap-repair.timer","next":null,"left":null,"last":0,"passed":0,"activates":"snapd.snap-repair.service"}]"#;
        let entries: Vec<TimerEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(entries[0].next, None);
        assert_eq!(entries[0].last, Some(0));
    }

    #[test]
    fn timer_entry_missing_unit_defaults_to_empty_string_not_error() {
        let json = r#"[{"next":123}]"#;
        let entries: Vec<TimerEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(entries[0].unit, "");
    }

    /// Fixture: the exact `systemctl list-timers --all --output=json`
    /// output captured on this Ubuntu 22.04.5 LTS system (systemd 249)
    /// while implementing this module — the Risk register's explicit
    /// mitigation ("cover with a fixture-based unit test using a captured
    /// real `systemctl` JSON sample from the Ubuntu LTS reference
    /// environment") for the "JSON shape can vary across systemd versions"
    /// risk.
    const REAL_CAPTURED_FIXTURE: &str = r#"[{"next":1785928048456538,"left":1785928048456538,"last":1785926967075918,"passed":1785926967075918,"unit":"fwupd-refresh.timer","activates":"fwupd-refresh.service"},{"next":1785928140000000,"left":1785928140000000,"last":1785926342079109,"passed":1785926342079109,"unit":"phpsessionclean.timer","activates":"phpsessionclean.service"},{"next":1785967200000000,"left":1785967200000000,"last":0,"passed":0,"unit":"dpkg-db-backup.timer","activates":"dpkg-db-backup.service"},{"next":1785971134662269,"left":1785971134662269,"last":1785916879750698,"passed":1785916879750698,"unit":"apt-daily.timer","activates":"apt-daily.service"},{"next":null,"left":null,"last":0,"passed":0,"unit":"snapd.snap-repair.timer","activates":"snapd.snap-repair.service"}]"#;

    #[test]
    fn fixture_from_real_machine_deserializes_without_error() {
        let entries: Vec<TimerEntry> =
            serde_json::from_str(REAL_CAPTURED_FIXTURE).expect("fixture must parse");
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[3].unit, "apt-daily.timer");
        assert_eq!(entries[3].next, Some(1785971134662269));
        // The null-`next`/zero-`last` timer (a real edge case on this
        // machine) must parse cleanly, not error the whole batch.
        assert_eq!(entries[4].unit, "snapd.snap-repair.timer");
        assert_eq!(entries[4].next, None);
        assert_eq!(entries[4].last, Some(0));
    }

    #[test]
    fn fixture_from_real_machine_builds_rows_without_panicking() {
        // build_row() calls out to `systemctl show` for enrichment; on any
        // machine with systemd installed (guaranteed here) this succeeds,
        // and on one without it, unit_show_details degrades to an empty
        // map rather than panicking — either way this must not panic.
        let entries: Vec<TimerEntry> =
            serde_json::from_str(REAL_CAPTURED_FIXTURE).expect("fixture must parse");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            entries.into_iter().map(build_row).collect::<Vec<_>>()
        }));
        assert!(outcome.is_ok(), "build_row must never panic");
        let rows = outcome.unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[3].name, "apt-daily.timer");
        assert_eq!(rows[4].next_run, "n/a");
    }

    // --- unit_show_details ------------------------------------------------

    #[test]
    fn unit_show_details_empty_unit_returns_empty_map_without_invoking_systemctl() {
        assert!(unit_show_details("").is_empty());
    }

    // --- Real-machine verification (acceptance criterion) ----------------
    //
    // "The Automations view lists at least one real systemd timer (e.g.
    // apt-daily.timer) with name/next-run/state populated — verify against
    // this machine's real systemd, not a fixture-only test." Verified
    // manually before writing this test: `systemctl list-timers --all`
    // shows `apt-daily.timer` on this Ubuntu 22.04.5 LTS system, and
    // `systemctl show apt-daily.timer -p Description,ActiveState,SubState`
    // returns "Daily apt download activities" / "active" / "waiting".

    #[test]
    fn real_systemctl_fetch_lists_apt_daily_timer_with_populated_fields() {
        let rows = fetch().expect(
            "systemctl must be present and list-timers --output=json must succeed on this \
             Ubuntu 22.04.5 LTS reference system",
        );
        assert!(
            !rows.is_empty(),
            "expected at least one real systemd timer, got zero rows"
        );

        let apt_daily = rows
            .iter()
            .find(|row| row.name == "apt-daily.timer")
            .unwrap_or_else(|| {
                panic!(
                    "expected 'apt-daily.timer' among real fetched rows: {:?}",
                    rows.iter().map(|r| &r.name).collect::<Vec<_>>()
                )
            });

        assert_ne!(apt_daily.next_run, "", "next_run must be populated");
        assert_ne!(
            apt_daily.next_run, "n/a",
            "apt-daily.timer has a real next run"
        );
        assert!(
            apt_daily.state.contains("active"),
            "expected an active/waiting state, got {:?}",
            apt_daily.state
        );
        assert!(
            apt_daily.author.to_lowercase().contains("apt"),
            "expected apt-daily.timer's Description to mention apt, got {:?}",
            apt_daily.author
        );
    }

    #[test]
    fn real_systemctl_show_unit_details_returns_known_fields() {
        let details = unit_show_details("apt-daily.timer");
        assert_eq!(
            details.get("ActiveState").map(String::as_str),
            Some("active")
        );
        assert!(details.contains_key("Description"));
        assert!(details.contains_key("FragmentPath"));
    }
}
