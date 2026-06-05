//! Grid-model generator — the future XAML / WinUI 3 seam.
//!
//! Currently produces an in-memory layout description consumed by the Win32
//! child-control host in `app.rs`. When WinUI 3 / XAML Islands become
//! available in Rust, this module is the insertion point (Decision D1).

/// A single cell in the command grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridCell {
    /// Zero-based row index.
    pub row: u32,
    /// Zero-based column index.
    pub col: u32,
    /// Label displayed on the button (used as fallback when no icon resolves).
    pub label: String,
    /// The raw `icon` string from `Command.icon` — may be an emoji or a
    /// filename that resolves to an image via `crate::icons::resolve_icon`.
    /// Empty string means "no icon configured; use label only".
    pub icon: String,
    /// Optional stable command id (from `Command.id`) for future use.
    pub command_id: Option<String>,
}

/// A fully computed grid layout ready to be rendered.
#[derive(Debug, Clone)]
pub struct GridModel {
    /// Number of columns in the grid.
    pub cols: u32,
    /// Ordered list of cells (row-major).
    pub cells: Vec<GridCell>,
}

impl GridModel {
    /// Total number of rows (computed from cells and column count).
    pub fn row_count(&self) -> u32 {
        if self.cells.is_empty() || self.cols == 0 {
            return 0;
        }
        let n = self.cells.len() as u32;
        n.div_ceil(self.cols)
    }
}

/// Input descriptor for a single command in the grid.
///
/// Additive over the previous label-only interface: `icon` and `command_id`
/// default to empty/None when not provided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridEntry {
    /// Display label (command name).
    pub label: String,
    /// Raw icon string from `Command.icon` (emoji or filename).
    pub icon: String,
    /// Stable command id, if available.
    pub command_id: Option<String>,
}

impl GridEntry {
    /// Convenience constructor for label-only entries (backward compat).
    pub fn label_only(label: impl Into<String>) -> Self {
        GridEntry {
            label: label.into(),
            icon: String::new(),
            command_id: None,
        }
    }
}

/// Build a [`GridModel`] from a list of [`GridEntry`] items.
///
/// `preferred_cols` is the desired number of columns.  If fewer items are
/// provided than one full row, the grid will have `items.len()` columns
/// instead (never a column count of zero).
pub fn build_grid(entries: &[GridEntry], preferred_cols: u32) -> GridModel {
    let cols = if entries.is_empty() {
        preferred_cols.max(1)
    } else {
        preferred_cols.max(1).min(entries.len() as u32)
    };

    let cells = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| GridCell {
            row: i as u32 / cols,
            col: i as u32 % cols,
            label: entry.label.clone(),
            icon: entry.icon.clone(),
            command_id: entry.command_id.clone(),
        })
        .collect();

    GridModel { cells, cols }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(names: &[&str]) -> Vec<GridEntry> {
        names.iter().map(|s| GridEntry::label_only(*s)).collect()
    }

    fn cell(row: u32, col: u32, label: &str) -> GridCell {
        GridCell {
            row,
            col,
            label: label.into(),
            icon: String::new(),
            command_id: None,
        }
    }

    #[test]
    fn empty_labels_produces_empty_cells() {
        let model = build_grid(&[], 3);
        assert!(model.cells.is_empty());
        assert_eq!(model.row_count(), 0);
    }

    #[test]
    fn single_item_is_row_zero_col_zero() {
        let model = build_grid(&entries(&["Notepad"]), 3);
        assert_eq!(model.cells.len(), 1);
        assert_eq!(model.cells[0].row, 0);
        assert_eq!(model.cells[0].col, 0);
        assert_eq!(model.cells[0].label, "Notepad");
    }

    #[test]
    fn three_items_with_two_cols_gives_two_rows() {
        let model = build_grid(&entries(&["A", "B", "C"]), 2);
        assert_eq!(model.cols, 2);
        assert_eq!(model.row_count(), 2);
        // Row 0: A(0,0) B(0,1)
        assert_eq!(model.cells[0], cell(0, 0, "A"));
        assert_eq!(model.cells[1], cell(0, 1, "B"));
        // Row 1: C(1,0)
        assert_eq!(model.cells[2], cell(1, 0, "C"));
    }

    #[test]
    fn preferred_cols_clamped_to_item_count() {
        // Requesting 10 columns with only 3 items: cols should be 3.
        let model = build_grid(&entries(&["X", "Y", "Z"]), 10);
        assert_eq!(model.cols, 3);
        assert_eq!(model.row_count(), 1);
    }

    #[test]
    fn exact_fill_grid_is_rectangular() {
        // 4 items, 2 cols → 2 rows × 2 cols
        let model = build_grid(&entries(&["A", "B", "C", "D"]), 2);
        assert_eq!(model.cols, 2);
        assert_eq!(model.row_count(), 2);
        assert_eq!(model.cells.len(), 4);
    }

    #[test]
    fn grid_entry_preserves_icon_and_command_id() {
        let entry = GridEntry {
            label: "Notepad".into(),
            icon: "notepad.png".into(),
            command_id: Some("notepad".into()),
        };
        let model = build_grid(&[entry], 3);
        assert_eq!(model.cells[0].icon, "notepad.png");
        assert_eq!(model.cells[0].command_id, Some("notepad".into()));
        assert_eq!(model.cells[0].label, "Notepad");
    }

    #[test]
    fn label_only_entry_has_empty_icon() {
        let entry = GridEntry::label_only("Cmd");
        let model = build_grid(&[entry], 3);
        assert_eq!(model.cells[0].icon, "");
        assert_eq!(model.cells[0].command_id, None);
    }
}
