//! Command form widget (Part 2 Phase 2) — a repeatable executable+arguments
//! row list that recomposes into a single command string, validated by
//! round-tripping it back through the real cross-platform tokenizer
//! ([`terminal_view::tokenize`]) before it is ever considered valid.
//!
//! Standalone widget — not yet wired into a real screen (Part 3 does
//! that). The widget has no save button of its own: a caller queries
//! [`CommandFormWidget::is_valid`] and gates its own save action on it.
//!
//! # Recomposition / round-trip contract
//!
//! [`terminal_view::tokenize`] (verified this session):
//! - A `"` character toggles an internal `in_quotes` flag and is itself
//!   dropped from the output.
//! - Whitespace splits tokens only when not inside quotes.
//! - A token that ends up empty is silently dropped.
//!
//! [`recompose`] therefore wraps any row containing whitespace in a pair
//! of `"` characters (the correct encoding for that tokenizer), but never
//! reimplements parsing — [`validate`] always re-tokenizes the recomposed
//! string via the real [`terminal_view::tokenize`] and compares the
//! result to the original rows. A row containing a literal `"` cannot be
//! encoded losslessly (the tokenizer drops quote characters on the way
//! back), so it is never special-cased here: the round-trip comparison
//! catches the mismatch and reports it as invalid, exactly like any other
//! lossy recomposition.
//!
//! An empty-string row is rejected by [`recompose`] itself, before a
//! tokenizer ever sees it — never silently dropped the way `tokenize`
//! drops empty tokens (Risk register).

use eframe::egui;

use super::terminal_view;

/// Why [`recompose`] refused to build a single command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecomposeError {
    /// Row at this 0-based index is an empty string.
    EmptyRow(usize),
    /// There are no rows at all (nothing to compose).
    NoRows,
}

/// Why [`validate`] considers the current rows invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// A row is empty (see [`RecomposeError::EmptyRow`]).
    EmptyRow(usize),
    /// There are no rows at all.
    NoRows,
    /// `terminal_view::tokenize` rejected the recomposed string outright
    /// (e.g. it trimmed down to nothing).
    TokenizeFailed(String),
    /// The recomposed string, re-tokenized, doesn't yield back the
    /// original rows (e.g. a row contains a literal `"`, which cannot be
    /// encoded losslessly through this tokenizer's quoting rule).
    RoundTripMismatch {
        recomposed: String,
        expected: Vec<String>,
        actual: Vec<String>,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::EmptyRow(i) => {
                write!(f, "la ligne {} est vide", i + 1)
            }
            ValidationError::NoRows => write!(f, "aucune ligne (exécutable manquant)"),
            ValidationError::TokenizeFailed(msg) => {
                write!(f, "commande invalide : {msg}")
            }
            ValidationError::RoundTripMismatch { .. } => write!(
                f,
                "cette valeur ne peut pas être encodée de façon fiable (guillemet non pris en charge ?)"
            ),
        }
    }
}

/// Recompose `rows` (row 0 = executable, rest = arguments) into a single
/// tokenizer-compatible string: any row containing whitespace is wrapped
/// in a pair of `"` characters, rows are joined with a single space.
///
/// Rejects an empty-string row (or an empty row list) before any
/// recomposition happens — never lets an empty token reach the tokenizer
/// to be silently dropped.
pub fn recompose(rows: &[String]) -> Result<String, RecomposeError> {
    if rows.is_empty() {
        return Err(RecomposeError::NoRows);
    }
    let mut parts = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        if row.is_empty() {
            return Err(RecomposeError::EmptyRow(i));
        }
        if row.chars().any(|c| c == ' ' || c == '\t') {
            parts.push(format!("\"{row}\""));
        } else {
            parts.push(row.clone());
        }
    }
    Ok(parts.join(" "))
}

/// Recompose `rows` and round-trip the result through the real
/// [`terminal_view::tokenize`], returning the recomposed string only if
/// re-tokenizing it yields back exactly `rows`.
pub fn validate(rows: &[String]) -> Result<String, ValidationError> {
    let recomposed = recompose(rows).map_err(|e| match e {
        RecomposeError::EmptyRow(i) => ValidationError::EmptyRow(i),
        RecomposeError::NoRows => ValidationError::NoRows,
    })?;

    let (program, args) =
        terminal_view::tokenize(&recomposed).map_err(ValidationError::TokenizeFailed)?;
    let mut actual = Vec::with_capacity(1 + args.len());
    actual.push(program);
    actual.extend(args);

    if actual != rows {
        return Err(ValidationError::RoundTripMismatch {
            recomposed,
            expected: rows.to_vec(),
            actual,
        });
    }

    Ok(recomposed)
}

/// Repeatable executable+arguments row widget. Row 0 is implicitly the
/// executable; every later row is an argument.
pub struct CommandFormWidget {
    rows: Vec<String>,
}

impl Default for CommandFormWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandFormWidget {
    /// A fresh form with a single empty executable row.
    pub fn new() -> Self {
        Self {
            rows: vec![String::new()],
        }
    }

    /// Build a form pre-filled from rows directly. Part of the widget's
    /// public API (and directly unit-tested below) but not currently
    /// called from `egui_app.rs`'s Préférences wiring, which only ever
    /// needs `new`/`from_command` — kept `#[allow(dead_code)]` rather than
    /// removed since a future caller with already-split argv (e.g. a
    /// "duplicate action" feature) is a legitimate use, and the row-list
    /// constructor duality (`with_rows` vs `from_command`) is exactly the
    /// module's own recompose/tokenize contract this file documents.
    #[allow(dead_code)]
    pub fn with_rows(rows: Vec<String>) -> Self {
        if rows.is_empty() {
            Self::new()
        } else {
            Self { rows }
        }
    }

    /// Build a form pre-filled by tokenizing an existing single-string
    /// command (e.g. `Command.command` from storage). Preserves the
    /// existing value as a single row, untouched, if it fails to
    /// tokenize (e.g. an empty string) rather than discarding it.
    pub fn from_command(command: &str) -> Self {
        match terminal_view::tokenize(command) {
            Ok((program, args)) => {
                let mut rows = Vec::with_capacity(1 + args.len());
                rows.push(program);
                rows.extend(args);
                Self { rows }
            }
            Err(_) => Self {
                rows: vec![command.to_string()],
            },
        }
    }

    /// The current row values (row 0 = executable). Not currently called
    /// from `egui_app.rs` (the Préférences form only reads back the
    /// recomposed single string via `recomposed()`) — kept as public API
    /// and directly unit-tested below, same rationale as `with_rows`.
    #[allow(dead_code)]
    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    /// Whether the current rows recompose and round-trip cleanly through
    /// `terminal_view::tokenize`. The widget has no save button of its
    /// own — a caller (Part 3) gates its own save action on this.
    pub fn is_valid(&self) -> bool {
        validate(&self.rows).is_ok()
    }

    /// The recomposed, tokenizer-validated command string, or `None` if
    /// the current rows are invalid.
    pub fn recomposed(&self) -> Option<String> {
        validate(&self.rows).ok()
    }

    /// A human-readable inline error message for the current rows, or
    /// `None` if they're valid.
    pub fn error_message(&self) -> Option<String> {
        validate(&self.rows).err().map(|e| e.to_string())
    }

    /// Render the row list (text fields, add/remove/reorder controls)
    /// plus an inline error when the current rows don't validate. Returns
    /// whether any row value or the row list itself changed this frame.
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let len = self.rows.len();
        let mut swap: Option<(usize, usize)> = None;
        let mut remove_at: Option<usize> = None;
        let mut add_row = false;

        for i in 0..len {
            ui.horizontal(|ui| {
                let row_label = ui.label(if i == 0 { "Exécutable" } else { "Argument" });

                // Without this, `get_by_label("Exécutable"/"Argument")`
                // resolves to the *static label* node (which silently
                // no-ops on `.focus()` rather than erroring — see
                // `egui_app.rs`'s test module docs on this exact footgun),
                // not the adjacent text field a caller actually means to
                // target.
                let response = ui
                    .text_edit_singleline(&mut self.rows[i])
                    .labelled_by(row_label.id);
                if response.changed() {
                    changed = true;
                }

                if self.rows[i].is_empty() {
                    ui.colored_label(egui::Color32::RED, "valeur vide non autorisée");
                }

                // Row indices are baked into these labels (rather than
                // reusing a bare "⬆"/"⬇"/"Supprimer" per row) so each
                // control has a unique accessible name — both for
                // egui_kittest's `get_by_label` in tests below and so an
                // assistive-tech user can tell rows apart. ⬆/⬇ (U+2B06/2B07)
                // rather than ↑/↓ (U+2191/2193): the plain arrows are covered
                // by no font in the chain (see `crate::ui::fonts`) and
                // rendered as tofu.
                if i > 0 && ui.button(format!("⬆ {}", i + 1)).clicked() {
                    swap = Some((i, i - 1));
                }
                if i + 1 < len && ui.button(format!("⬇ {}", i + 1)).clicked() {
                    swap = Some((i, i + 1));
                }
                // The executable row (index 0) is implicit and mandatory
                // — only argument rows can be removed.
                if i > 0 && ui.button(format!("Supprimer {}", i + 1)).clicked() {
                    remove_at = Some(i);
                }
            });
        }

        if ui.button("+ Argument").clicked() {
            add_row = true;
        }

        if let Some((a, b)) = swap {
            self.rows.swap(a, b);
            changed = true;
        }
        if let Some(i) = remove_at {
            self.rows.remove(i);
            changed = true;
        }
        if add_row {
            self.rows.push(String::new());
            changed = true;
        }

        if let Some(message) = self.error_message() {
            ui.colored_label(egui::Color32::RED, message);
        }

        changed
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    fn rows(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    // --- recompose / validate: pure-function tests, no egui context ---

    #[test]
    fn recompose_joins_plain_rows_with_spaces() {
        let composed = recompose(&rows(&["cmd.exe", "--flag", "value"])).unwrap();
        assert_eq!(composed, "cmd.exe --flag value");
    }

    #[test]
    fn recompose_quotes_a_row_containing_a_space() {
        let composed = recompose(&rows(&["/opt/my app/run", "--flag", "value"])).unwrap();
        assert_eq!(composed, "\"/opt/my app/run\" --flag value");
    }

    #[test]
    fn recompose_rejects_empty_row_before_reaching_tokenizer() {
        let err = recompose(&rows(&["cmd.exe", "", "value"])).unwrap_err();
        assert_eq!(err, RecomposeError::EmptyRow(1));
    }

    #[test]
    fn recompose_rejects_empty_row_list() {
        let err = recompose(&[]).unwrap_err();
        assert_eq!(err, RecomposeError::NoRows);
    }

    #[test]
    fn a_row_containing_a_space_round_trips_through_recompose_and_tokenize() {
        // Demonstrates the full flow: recompose -> the *real*
        // terminal_view::tokenize -> equals the original rows.
        let original = rows(&["/opt/my app/run", "--flag", "value with space"]);
        let composed = recompose(&original).unwrap();

        let (program, args) = terminal_view::tokenize(&composed).unwrap();
        let mut actual = vec![program];
        actual.extend(args);

        assert_eq!(actual, original);
    }

    #[test]
    fn validate_accepts_clean_round_tripping_rows() {
        let r = rows(&["notepad.exe"]);
        assert_eq!(validate(&r).unwrap(), "notepad.exe");
    }

    #[test]
    fn validate_accepts_rows_with_whitespace_via_quoting() {
        let r = rows(&["/opt/my app/run", "value with space"]);
        assert_eq!(
            validate(&r).unwrap(),
            "\"/opt/my app/run\" \"value with space\""
        );
    }

    #[test]
    fn validate_rejects_empty_row_without_calling_tokenize() {
        let r = rows(&["cmd.exe", ""]);
        assert_eq!(validate(&r).unwrap_err(), ValidationError::EmptyRow(1));
    }

    #[test]
    fn validate_rejects_row_containing_a_literal_quote_as_round_trip_mismatch() {
        // A literal `"` can't be encoded losslessly through this
        // tokenizer's quoting rule (quotes are stripped, not escaped), so
        // this must be reported as invalid via the round-trip check, not
        // silently accepted or hand-escaped.
        let r = rows(&["echo", "say\"hi"]);
        match validate(&r) {
            Err(ValidationError::RoundTripMismatch {
                expected, actual, ..
            }) => {
                assert_eq!(expected, r);
                assert_ne!(actual, r);
            }
            other => panic!("expected RoundTripMismatch, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_row_containing_quote_and_whitespace_as_round_trip_mismatch() {
        let r = rows(&["echo", "foo bar\"baz"]);
        assert!(matches!(
            validate(&r),
            Err(ValidationError::RoundTripMismatch { .. })
        ));
    }

    // --- CommandFormWidget: construction / query API, no egui context ---

    #[test]
    fn new_widget_starts_with_a_single_empty_executable_row() {
        let widget = CommandFormWidget::new();
        assert_eq!(widget.rows(), &["".to_string()]);
        assert!(!widget.is_valid());
    }

    #[test]
    fn with_rows_is_valid_when_rows_round_trip() {
        let widget = CommandFormWidget::with_rows(rows(&["notepad.exe"]));
        assert!(widget.is_valid());
        assert_eq!(widget.recomposed().as_deref(), Some("notepad.exe"));
        assert!(widget.error_message().is_none());
    }

    #[test]
    fn with_rows_is_invalid_when_a_row_is_empty() {
        let widget = CommandFormWidget::with_rows(rows(&["notepad.exe", ""]));
        assert!(!widget.is_valid());
        assert!(widget.recomposed().is_none());
        assert!(widget.error_message().is_some());
    }

    #[test]
    fn with_rows_falls_back_to_one_empty_row_when_given_none() {
        let widget = CommandFormWidget::with_rows(vec![]);
        assert_eq!(widget.rows(), &["".to_string()]);
    }

    #[test]
    fn from_command_tokenizes_an_existing_command_string() {
        let widget = CommandFormWidget::from_command("notepad.exe --flag value");
        assert_eq!(
            widget.rows(),
            &[
                "notepad.exe".to_string(),
                "--flag".to_string(),
                "value".to_string()
            ]
        );
        assert!(widget.is_valid());
    }

    #[test]
    fn from_command_preserves_a_command_that_fails_to_tokenize_as_a_single_row() {
        // An empty/whitespace-only command fails `tokenize` outright — the
        // existing value must still be preserved (as a single row), not
        // discarded or replaced by an empty widget.
        let widget = CommandFormWidget::from_command("   ");
        assert_eq!(widget.rows(), &["   ".to_string()]);
    }

    // --- CommandFormWidget::show: egui_kittest interaction tests ---

    struct TestState {
        widget: CommandFormWidget,
        changed: bool,
    }

    fn harness_for(widget: CommandFormWidget) -> Harness<'static, TestState> {
        let state = TestState {
            widget,
            changed: false,
        };
        Harness::builder()
            .with_size(egui::vec2(600.0, 400.0))
            .build_ui_state(
                |ui, state: &mut TestState| {
                    state.changed = state.widget.show(ui);
                },
                state,
            )
    }

    #[test]
    fn clicking_add_argument_appends_an_empty_row() {
        let mut harness = harness_for(CommandFormWidget::with_rows(rows(&["notepad.exe"])));
        harness.run();
        harness.get_by_label("+ Argument").click();
        harness.step();
        assert_eq!(
            harness.state().widget.rows(),
            &["notepad.exe".to_string(), "".to_string()]
        );
        assert!(harness.state().changed);
    }

    #[test]
    fn empty_row_shows_inline_error_and_is_invalid() {
        let mut harness = harness_for(CommandFormWidget::with_rows(rows(&["notepad.exe", ""])));
        harness.run();
        assert!(!harness.state().widget.is_valid());
        assert!(harness
            .query_by_label("valeur vide non autorisée")
            .is_some());
    }

    #[test]
    fn clicking_supprimer_removes_the_targeted_argument_row() {
        let mut harness = harness_for(CommandFormWidget::with_rows(rows(&[
            "notepad.exe",
            "--flag",
        ])));
        harness.run();
        harness.get_by_label("Supprimer 2").click();
        harness.step();
        assert_eq!(harness.state().widget.rows(), &["notepad.exe".to_string()]);
    }

    #[test]
    fn valid_rows_show_no_inline_error() {
        let mut harness = harness_for(CommandFormWidget::with_rows(rows(&["notepad.exe"])));
        harness.run();
        assert!(harness
            .query_by_label("valeur vide non autorisée")
            .is_none());
        assert!(harness.state().widget.is_valid());
    }
}
