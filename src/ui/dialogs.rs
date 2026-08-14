//! Cross-platform modal dialogs — `egui`/`eframe` equivalents of the 3
//! `MessageBoxW` call sites formerly in `src/ui/app.rs`:
//!
//! - `show_about` (~L1579) — an "OK"-only informational box → [`info`]
//! - `show_message` (~L1585, `warning` flag) — "OK"-only info/warning box →
//!   [`info`] / [`warn`]
//! - `confirm_action` (~L1593, `MB_YESNO`) — a yes/no confirmation box →
//!   [`confirm`]
//!
//! # Blocking (Part 2 plan Risk register item 3)
//!
//! `MessageBoxW` blocks at the OS level; nothing else in the process can
//! receive input while it's up. `egui`'s modals are drawn in the same event
//! loop, so blocking has to be modeled explicitly. This module provides the
//! rendering half ([`show`]); the caller (`EguiApp::ui_content`) provides the
//! state half — an `Option<ActiveDialog>` field checked *first*, before any
//! other widget is built for the frame, so a stray click can neither reach a
//! widget behind the dialog (nothing behind it is even instantiated that
//! frame) nor dismiss the dialog itself (only its own Oui/Non/OK buttons, or
//! the backdrop/Escape via [`DialogOutcome`], resolve it). `egui::Modal`'s
//! own backdrop-click-blocking is a second, independent layer on top of that
//! state-level short-circuit, not a substitute for it.

use eframe::egui;

/// Which modal is currently queued/active, and its text. Constructed by
/// [`info`], [`warn`], or [`confirm`]; rendered by [`show`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogKind {
    /// OK-only informational box (former `MB_OK | MB_ICONINFORMATION`).
    Info { title: String, message: String },
    /// OK-only warning box (former `MB_OK | MB_ICONWARNING`).
    #[allow(dead_code)]
    Warn { title: String, message: String },
    /// Yes/No confirmation box (former `MB_YESNO | MB_ICONWARNING`).
    Confirm { title: String, message: String },
}

/// Build an OK-only informational dialog — former `show_about`/`show_message(warning=false)`.
pub fn info(title: impl Into<String>, message: impl Into<String>) -> DialogKind {
    DialogKind::Info {
        title: title.into(),
        message: message.into(),
    }
}

/// Build an OK-only warning dialog — former `show_message(warning=true)`.
#[allow(dead_code)]
pub fn warn(title: impl Into<String>, message: impl Into<String>) -> DialogKind {
    DialogKind::Warn {
        title: title.into(),
        message: message.into(),
    }
}

/// Build a Yes/No confirmation dialog — former `confirm_action`.
pub fn confirm(title: impl Into<String>, message: impl Into<String>) -> DialogKind {
    DialogKind::Confirm {
        title: title.into(),
        message: message.into(),
    }
}

/// Outcome of rendering a [`DialogKind`] for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogOutcome {
    /// Still open — no button pressed yet this frame.
    Pending,
    /// OK (info/warn) or Oui (confirm) was pressed.
    Accepted,
    /// Non was pressed (confirm), or the dialog was dismissed via the
    /// backdrop / Escape (info/warn/confirm) — former `IDNO`/`IDCANCEL`.
    Dismissed,
}

/// Render `kind` as a blocking, centered `egui::Modal` and return this
/// frame's [`DialogOutcome`]. The caller is expected to call this as the
/// very first thing in its update function (see module docs) and to clear
/// its own "active dialog" state on any non-`Pending` outcome.
pub fn show(ctx: &egui::Context, kind: &DialogKind) -> DialogOutcome {
    let (title, message, is_confirm) = match kind {
        DialogKind::Info { title, message } => (title.as_str(), message.as_str(), false),
        DialogKind::Warn { title, message } => (title.as_str(), message.as_str(), false),
        DialogKind::Confirm { title, message } => (title.as_str(), message.as_str(), true),
    };

    let mut outcome = DialogOutcome::Pending;
    let modal = egui::Modal::new(egui::Id::new("devtoolbox-active-dialog"));
    let modal_response = modal.show(ctx, |ui| {
        ui.set_min_width(280.0);
        ui.heading(title);
        ui.separator();
        ui.label(message);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if is_confirm {
                if ui.button("Oui").clicked() {
                    outcome = DialogOutcome::Accepted;
                }
                if ui.button("Non").clicked() {
                    outcome = DialogOutcome::Dismissed;
                }
            } else if ui.button("OK").clicked() {
                outcome = DialogOutcome::Accepted;
            }
        });
    });

    if outcome == DialogOutcome::Pending && modal_response.should_close() {
        outcome = DialogOutcome::Dismissed;
    }

    outcome
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    /// `show()`'s outcome depends on `&egui::Context`, not on any owned
    /// widget state, so — mirroring the pattern in the doc example for
    /// `HarnessBuilder::build_ui_state` — the dialog kind and the resolved
    /// outcome are threaded through as `Harness` state (`harness.state()`)
    /// rather than captured by a `build_ui` closure: a closure capturing a
    /// local `&mut outcome` would keep it mutably borrowed for as long as
    /// `harness` (which stores the closure) is alive, which conflicts with
    /// reading it back after `harness.run()`.
    struct TestState {
        kind: DialogKind,
        outcome: DialogOutcome,
    }

    fn harness_for(kind: DialogKind) -> Harness<'static, TestState> {
        let state = TestState {
            kind,
            outcome: DialogOutcome::Pending,
        };
        Harness::builder()
            .with_size(egui::vec2(400.0, 300.0))
            .build_ui_state(
                |ui, state: &mut TestState| {
                    state.outcome = show(ui.ctx(), &state.kind);
                },
                state,
            )
    }

    #[test]
    fn info_dialog_shows_ok_button_and_resolves_accepted_on_click() {
        let mut harness = harness_for(info("Titre", "Message"));

        harness.run();
        assert!(
            harness.query_by_label("Non").is_none(),
            "info dialog must not offer a Non button"
        );
        harness.get_by_label("OK").click();
        // `show()` recomputes a fresh `Pending` at the top of every call, by
        // design (see module docs: it reports *this frame's* outcome only,
        // so the real caller can act on Accepted/Dismissed once and clear
        // its own dialog state). `harness.run()` keeps stepping until the
        // UI settles, which would call the closure again on a later,
        // click-free frame and overwrite the state back to `Pending`. A
        // single `step()` processes exactly the queued click and reads back
        // that frame's outcome, matching how the real app consumes it.
        harness.step();
        assert_eq!(harness.state().outcome, DialogOutcome::Accepted);
    }

    #[test]
    fn confirm_dialog_non_button_resolves_dismissed() {
        let mut harness = harness_for(confirm("Titre", "Message"));

        harness.run();
        harness.get_by_label("Non").click();
        harness.step();
        assert_eq!(harness.state().outcome, DialogOutcome::Dismissed);
    }

    #[test]
    fn confirm_dialog_oui_button_resolves_accepted() {
        let mut harness = harness_for(confirm("Titre", "Message"));

        harness.run();
        harness.get_by_label("Oui").click();
        harness.step();
        assert_eq!(harness.state().outcome, DialogOutcome::Accepted);
    }
}
