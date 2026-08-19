# Automations view scopes to user-created automations, plus a native-tool link

- Date: 2026-08-17
- Status: Accepted

## Context

The Automations view (`src/ui/automations_view.rs`) originally mirrored
Windows' `Get-ScheduledTask` / Linux's `systemctl list-timers --all`
one-for-one — every scheduled task/timer on the system, including ones
shipped by the OS or its packages (e.g. Windows' `.NET Framework NGEN`
tasks, Ubuntu's `apt-daily.timer`). That was flagged during a manual
walkthrough as low-value: both OSes already have a native tool for browsing
*everything* (Task Scheduler on Windows; `systemctl list-timers` on Linux),
so a read-only mirror of the same data adds nothing. The stated purpose of
this screen is narrower: let the user see what *they* (or software they
installed) added, without wading through the OS's own built-in scheduled
work.

## Decision

1. `fetch()` filters out OS/package-provided automations on both platforms:
   - **Windows**: rows whose `Category` (Task Scheduler `TaskPath`) is
     under `\Microsoft\...` are excluded
     (`automations_view::is_builtin_windows_task`). Third-party-software
     tasks (e.g. `\GoogleUpdate\`) are kept — the split is "shipped by
     Microsoft" vs. everything else, not "literally hand-created by the
     user".
   - **Linux**: rows whose unit `FragmentPath` (surfaced as `category`) is
     under `/usr/lib/systemd/system` or `/lib/systemd/system` (package-
     managed) are excluded
     (`crate::linux::automations::is_package_provided_category`); units
     under `/etc/systemd/system` (the conventional local-admin/user-added
     location) are kept.
2. The view gained an "Ouvrir l'outil natif" button
   (`automations_view::open_native_tool`) for reaching the full picture:
   - **Windows**: opens the Task Scheduler GUI (`mmc taskschd.msc`).
   - **Linux**: opens a terminal (`gnome-terminal`, matching the existing
     bundled "Terminal" action's direct-invocation convention) pre-loaded
     with `systemctl list-timers --all` — there is no single standard
     cross-desktop-environment GUI for systemd timers, so a terminal is the
     honest fallback rather than guessing at one that may not be installed.

## Alternatives

- **Hide the OS's native tool entirely and only ever show the filtered
  list** — rejected: the filtered view is deliberately narrower than "every
  automation on this machine"; users who need the full picture (e.g.
  diagnosing why a package timer fired) still need a path to it.
- **Try to detect and launch a third-party Linux scheduler GUI (e.g.
  `gnome-schedule`) if present, no-op otherwise** — rejected per direct
  user steer: fragile, depends on what happens to be installed, and
  degrades silently to nothing on most machines (verified empirically: none
  of the timers on this Ubuntu 22.04.5 LTS reference machine are user-
  created, so a genuinely empty Automations view is the correct, common
  case).

## Consequences

- On a fresh/typical machine (verified on this Ubuntu 22.04.5 LTS reference
  system: every configured timer is package-provided), the Automations view
  now legitimately renders its empty-state placeholder rather than a long
  list of OS internals — this is the intended, not a degraded, outcome.
- The Windows-side filter (`is_builtin_windows_task`) and its `fetch_impl`
  caller could not be compile-verified in this session (no
  `x86_64-pc-windows-gnu` rustup target installed here — same pre-existing
  limitation as the rest of `automations_view.rs`'s Windows path, see that
  file's module doc). `is_builtin_windows_task` itself is deliberately left
  un-gated (`#[cfg(windows)]`-free) so its string-matching logic is still
  unit-tested on Linux.
- "Ouvrir l'outil natif" spawns a real GUI/terminal process; the automated
  Linux smoke test (`egui_app::tests::automations_view_renders_real_systemd_rows_without_panicking_on_linux`)
  deliberately asserts the button renders without clicking it, to avoid
  spawning a stray `gnome-terminal` window on every test run.
