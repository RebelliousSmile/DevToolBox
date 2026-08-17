//! Curated icon picker widget (Part 2 Phase 1).
//!
//! Replaces free-text icon entry for both category and action forms with a
//! fixed, curated set of icon glyphs. Standalone widget — not yet wired
//! into a real screen (Part 3 does that); this module only needs to expose
//! [`CURATED_ICONS`], [`is_curated`], and [`show`].
//!
//! # Curated set provenance (Risk register)
//!
//! [`CURATED_ICONS`] is seeded from every icon literal already in use in
//! this codebase, so opening an existing category/action in the picker
//! shows a matching curated selection rather than looking like data loss:
//!
//! - `config/builtin-actions.json` categories/commands: 🔄 🧪 💼 🏠 🤖 📬 🚀
//!   🛠️ 🏗️ 🎵
//! - `src/ui/egui_app.rs`'s `fallback_config()` bundled default commands
//!   (Bloc-notes/Invite de commandes/Adresse IP): 📝 💻 🌐
//!
//! On top of that seed, a general-purpose set covers common categories
//! (files/folders, web/network, terminal/settings, media, communication,
//! status/misc) so the picker is useful for icons that don't yet exist
//! anywhere in this repo.
//!
//! # Out-of-set values (Risk register)
//!
//! A value that predates this feature (free-text icon field) may not be in
//! [`CURATED_ICONS`]. [`show`] must never silently reset such a value: it
//! displays it as-is, unselected in the grid, and only ever overwrites
//! `current` when the caller actually clicks a curated tile.

use eframe::egui;

/// Fixed curated icon set. See module docs for provenance. Order is
/// intentional (seeded/most-used icons first) but not semantically
/// meaningful — the grid just lays them out in this order.
pub const CURATED_ICONS: &[&str] = &[
    // --- Seeded from config/builtin-actions.json ---
    "🔄", "🧪", "💼", "🏠", "🤖", "📬", "🚀", "🛠️", "🏗️", "🎵",
    // --- Seeded from egui_app.rs::fallback_config bundled defaults ---
    "📝", "💻", "🌐", // --- General-purpose: files/folders ---
    "📁", "📂", "📄", "📋", "🗂️", // --- General-purpose: web/network ---
    "🔗", "🌍", "📡", "☁️", // --- General-purpose: terminal/code/settings ---
    "🖥️", "⚙️", "🔧", "🐚", "⌨️", // --- General-purpose: media ---
    "🎬", "🎧", "🖼️", "📷", "🎮", // --- General-purpose: communication ---
    "💬", "📧", "☎️", // --- General-purpose: status/misc ---
    "🔒", "🔑", "✅", "⚠️", "❓",
];

/// Whether `icon` is present in [`CURATED_ICONS`].
pub fn is_curated(icon: &str) -> bool {
    CURATED_ICONS.contains(&icon)
}

/// Number of icon tiles per grid row.
const COLUMNS: usize = 8;

/// Render the icon picker as a button that opens a popup grid on click.
/// `current` holds the field's current icon value (owned by the caller —
/// category form or command form scratch buffer) and is mutated in place
/// only when the user clicks a curated tile inside the popup. The popup
/// closes itself on any click (tile pick or elsewhere), per
/// [`egui::PopupCloseBehavior::CloseOnClick`]. Returns whether `current`
/// changed this frame.
///
/// If `current` isn't in [`CURATED_ICONS`] (e.g. a pre-existing free-text
/// value), it is shown as the button's own label and, once the popup is
/// open, listed below the grid, unselected, and left untouched unless the
/// user actively picks a curated tile.
pub fn show(ui: &mut egui::Ui, current: &mut String) -> bool {
    let mut changed = false;

    let button_label = if current.is_empty() {
        "Choisir…".to_string()
    } else {
        current.clone()
    };
    let toggle = ui.button(button_label);

    egui::Popup::from_toggle_button_response(&toggle)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            egui::Grid::new("devtoolbox-icon-picker-grid")
                .num_columns(COLUMNS)
                .spacing(egui::vec2(4.0, 4.0))
                .show(ui, |ui| {
                    for (i, icon) in CURATED_ICONS.iter().enumerate() {
                        let selected = current.as_str() == *icon;
                        if ui.selectable_label(selected, *icon).clicked() && !selected {
                            *current = (*icon).to_string();
                            changed = true;
                        }
                        if (i + 1) % COLUMNS == 0 {
                            ui.end_row();
                        }
                    }
                });

            if !current.is_empty() && !is_curated(current) {
                ui.horizontal(|ui| {
                    ui.label("Icône actuelle (hors liste) :");
                    ui.label(current.as_str());
                });
            }
        });

    changed
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    /// Every icon literal grepped from `config/builtin-actions.json`
    /// (Risk register / Phase 1 task 1).
    const BUILTIN_ACTIONS_ICONS: &[&str] =
        &["🔄", "🧪", "💼", "🏠", "🤖", "📬", "🚀", "🛠️", "🏗️", "🎵"];

    /// Every icon literal grepped from `egui_app.rs::fallback_config`'s
    /// bundled default commands (lines ~366/379/392).
    const FALLBACK_CONFIG_ICONS: &[&str] = &["📝", "💻", "🌐"];

    #[test]
    fn curated_icons_include_every_builtin_actions_icon() {
        for icon in BUILTIN_ACTIONS_ICONS {
            assert!(
                CURATED_ICONS.contains(icon),
                "CURATED_ICONS is missing builtin-actions.json icon {icon:?}"
            );
        }
    }

    #[test]
    fn curated_icons_include_every_fallback_config_icon() {
        for icon in FALLBACK_CONFIG_ICONS {
            assert!(
                CURATED_ICONS.contains(icon),
                "CURATED_ICONS is missing fallback_config bundled default icon {icon:?}"
            );
        }
    }

    #[test]
    fn curated_icons_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for icon in CURATED_ICONS {
            assert!(
                seen.insert(*icon),
                "duplicate icon in CURATED_ICONS: {icon:?}"
            );
        }
    }

    #[test]
    fn is_curated_true_for_seeded_icon_false_for_arbitrary_value() {
        assert!(is_curated("📝"));
        assert!(!is_curated("🦄🦄🦄-not-a-real-single-icon"));
        assert!(!is_curated(""));
    }

    struct TestState {
        current: String,
        changed: bool,
    }

    fn harness_for(current: &str) -> Harness<'static, TestState> {
        let state = TestState {
            current: current.to_string(),
            changed: false,
        };
        Harness::builder()
            .with_size(egui::vec2(600.0, 400.0))
            .build_ui_state(
                |ui, state: &mut TestState| {
                    state.changed = show(ui, &mut state.current);
                },
                state,
            )
    }

    #[test]
    fn clicking_a_curated_tile_selects_it() {
        let mut harness = harness_for("💻");
        harness.run();
        // The trigger button's label is the current icon; click it to
        // open the popup before the grid tiles exist in the tree.
        harness.get_by_label("💻").click();
        harness.run();
        harness.get_by_label("🌐").click();
        harness.step();
        assert_eq!(harness.state().current, "🌐");
        assert!(harness.state().changed);
    }

    #[test]
    fn clicking_the_already_selected_tile_does_not_report_a_change() {
        let mut harness = harness_for("💻");
        harness.run();
        harness.get_by_label("💻").click();
        harness.run();
        // Once open, both the trigger button and the selected grid tile
        // are labeled "💻" (trigger renders first in tree order).
        harness
            .get_all_by_label("💻")
            .nth(1)
            .expect("selected curated tile should be present in the open popup")
            .click();
        harness.step();
        assert_eq!(harness.state().current, "💻");
        assert!(!harness.state().changed);
    }

    #[test]
    fn out_of_set_current_value_is_preserved_and_not_cleared() {
        // A pre-existing free-text icon that predates this feature.
        let custom = "🦄";
        assert!(!is_curated(custom));

        let mut harness = harness_for(custom);
        harness.run();
        // No click on any curated tile — the widget must not have reset
        // or replaced the out-of-set value on its own.
        assert_eq!(harness.state().current, custom);
        assert!(!harness.state().changed);
    }

    #[test]
    fn out_of_set_current_value_survives_clicking_a_curated_tile_elsewhere() {
        let mut harness = harness_for("🦄");
        harness.run();
        // "🦄" isn't curated, so the trigger button's label stays
        // unambiguous even once the popup is open.
        harness.get_by_label("🦄").click();
        harness.run();
        // Picking a curated tile still works normally even when the
        // starting value was out-of-set.
        harness.get_by_label("🚀").click();
        harness.step();
        assert_eq!(harness.state().current, "🚀");
    }
}
