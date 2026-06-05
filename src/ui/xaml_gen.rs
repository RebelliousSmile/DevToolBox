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
    /// Label displayed on the button.
    pub label: String,
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

/// Build a [`GridModel`] from a list of command labels.
///
/// `preferred_cols` is the desired number of columns.  If fewer items are
/// provided than one full row, the grid will have `items.len()` columns
/// instead (never a column count of zero).
pub fn build_grid(labels: &[String], preferred_cols: u32) -> GridModel {
    let cols = if labels.is_empty() {
        preferred_cols.max(1)
    } else {
        preferred_cols.max(1).min(labels.len() as u32)
    };

    let cells = labels
        .iter()
        .enumerate()
        .map(|(i, label)| GridCell {
            row: i as u32 / cols,
            col: i as u32 % cols,
            label: label.clone(),
        })
        .collect();

    GridModel { cells, cols }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_labels_produces_empty_cells() {
        let model = build_grid(&[], 3);
        assert!(model.cells.is_empty());
        assert_eq!(model.row_count(), 0);
    }

    #[test]
    fn single_item_is_row_zero_col_zero() {
        let model = build_grid(&labels(&["Notepad"]), 3);
        assert_eq!(model.cells.len(), 1);
        assert_eq!(model.cells[0].row, 0);
        assert_eq!(model.cells[0].col, 0);
        assert_eq!(model.cells[0].label, "Notepad");
    }

    #[test]
    fn three_items_with_two_cols_gives_two_rows() {
        let model = build_grid(&labels(&["A", "B", "C"]), 2);
        assert_eq!(model.cols, 2);
        assert_eq!(model.row_count(), 2);
        // Row 0: A(0,0) B(0,1)
        assert_eq!(model.cells[0], GridCell { row: 0, col: 0, label: "A".into() });
        assert_eq!(model.cells[1], GridCell { row: 0, col: 1, label: "B".into() });
        // Row 1: C(1,0)
        assert_eq!(model.cells[2], GridCell { row: 1, col: 0, label: "C".into() });
    }

    #[test]
    fn preferred_cols_clamped_to_item_count() {
        // Requesting 10 columns with only 3 items: cols should be 3.
        let model = build_grid(&labels(&["X", "Y", "Z"]), 10);
        assert_eq!(model.cols, 3);
        assert_eq!(model.row_count(), 1);
    }

    #[test]
    fn exact_fill_grid_is_rectangular() {
        // 4 items, 2 cols → 2 rows × 2 cols
        let model = build_grid(&labels(&["A", "B", "C", "D"]), 2);
        assert_eq!(model.cols, 2);
        assert_eq!(model.row_count(), 2);
        assert_eq!(model.cells.len(), 4);
    }
}
