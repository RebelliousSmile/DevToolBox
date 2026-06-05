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

// ---------------------------------------------------------------------------
// Sectioned layout model (Phase 2 / issue #6)
// ---------------------------------------------------------------------------

/// A section in the grouped view: one header label followed by its entries.
///
/// An empty `entries` vec means the section is omitted by [`build_sectioned`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridSection {
    /// Section header label (category name, or "Uncategorized").
    pub header: String,
    /// Ordered entries that belong to this section.
    pub entries: Vec<GridEntry>,
}

/// A marker inside a [`SectionedModel`] describing one row of the stacked
/// sectioned layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionRow {
    /// A section header row.  Contains the header label and the section index.
    Header {
        /// The category name or "Uncategorized".
        label: String,
        /// Index of the section in the original `sections` slice.
        section_idx: usize,
    },
    /// A row of button cells belonging to a section.
    Cells {
        /// Index of the section these cells belong to.
        section_idx: usize,
        /// Cells for this row (up to `cols` items).
        cells: Vec<GridCell>,
    },
}

/// The fully computed sectioned layout.
///
/// `rows` is the ordered vertical list of [`SectionRow`]s:
/// header → its button rows → next header → … (non-empty sections only).
#[derive(Debug, Clone)]
pub struct SectionedModel {
    /// Number of columns used per section's button grid.
    pub cols: u32,
    /// Ordered vertical rows (headers interleaved with cell rows).
    pub rows: Vec<SectionRow>,
}

impl SectionedModel {
    /// Total number of rows (including header rows).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Collect all `Cells` rows in order.
    pub fn cell_rows(&self) -> Vec<&[GridCell]> {
        self.rows
            .iter()
            .filter_map(|r| {
                if let SectionRow::Cells { cells, .. } = r {
                    Some(cells.as_slice())
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Build a [`SectionedModel`] from an ordered list of [`GridSection`]s.
///
/// Empty sections (no entries) are omitted from the output.
/// Each non-empty section emits one header row followed by its button rows
/// (wrapped by `preferred_cols`, using the same clamping logic as
/// [`build_grid`]).
///
/// Global row indices are assigned top-to-bottom across sections.
pub fn build_sectioned(sections: &[GridSection], preferred_cols: u32) -> SectionedModel {
    let cols = preferred_cols.max(1);
    let mut rows: Vec<SectionRow> = Vec::new();

    for (section_idx, section) in sections.iter().enumerate() {
        // Skip empty sections.
        if section.entries.is_empty() {
            continue;
        }

        // Clamp columns to the actual number of entries in this section
        // (mirrors build_grid's per-section sizing).
        let sec_cols = cols.min(section.entries.len() as u32);

        // Emit the header row for this section.
        rows.push(SectionRow::Header {
            label: section.header.clone(),
            section_idx,
        });

        // Emit one Cells row per chunk of sec_cols entries.
        for chunk in section.entries.chunks(sec_cols as usize) {
            let cells: Vec<GridCell> = chunk
                .iter()
                .enumerate()
                .map(|(col_in_chunk, entry)| GridCell {
                    // row/col are LOCAL to this section's button grid;
                    // the global vertical position comes from the SectionRow
                    // position in `rows`.
                    row: 0, // filled in by layout_children using row position
                    col: col_in_chunk as u32,
                    label: entry.label.clone(),
                    icon: entry.icon.clone(),
                    command_id: entry.command_id.clone(),
                })
                .collect();
            rows.push(SectionRow::Cells { section_idx, cells });
        }
    }

    SectionedModel { cols, rows }
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

    // -----------------------------------------------------------------------
    // build_sectioned tests (Phase 2 / issue #6)
    // -----------------------------------------------------------------------

    fn section(header: &str, names: &[&str]) -> GridSection {
        GridSection {
            header: header.to_string(),
            entries: entries(names),
        }
    }

    #[test]
    fn build_sectioned_empty_sections_produces_no_rows() {
        let model = build_sectioned(&[], 3);
        assert!(model.rows.is_empty());
    }

    #[test]
    fn build_sectioned_single_section_emits_header_then_cells() {
        // 2 entries, 3 cols → clamped to 2 → 1 Cells row
        let model = build_sectioned(&[section("Système", &["Notepad", "Cmd"])], 3);

        assert_eq!(model.rows.len(), 2, "header + 1 cell row");
        assert!(matches!(
            &model.rows[0],
            SectionRow::Header { label, .. } if label == "Système"
        ));
        if let SectionRow::Cells { cells, .. } = &model.rows[1] {
            assert_eq!(cells.len(), 2);
            assert_eq!(cells[0].label, "Notepad");
            assert_eq!(cells[1].label, "Cmd");
        } else {
            panic!("row[1] must be Cells");
        }
    }

    #[test]
    fn build_sectioned_wraps_per_section_columns() {
        // 3 entries, 2 cols → 2 rows of cells (row0: 2 items, row1: 1 item)
        let model = build_sectioned(&[section("A", &["X", "Y", "Z"])], 2);

        // header + 2 cell rows
        assert_eq!(model.rows.len(), 3);
        if let SectionRow::Cells { cells, .. } = &model.rows[1] {
            assert_eq!(cells.len(), 2);
            assert_eq!(cells[0].col, 0);
            assert_eq!(cells[1].col, 1);
        } else {
            panic!("row[1] must be Cells");
        }
        if let SectionRow::Cells { cells, .. } = &model.rows[2] {
            assert_eq!(cells.len(), 1);
            assert_eq!(cells[0].col, 0);
        } else {
            panic!("row[2] must be Cells");
        }
    }

    #[test]
    fn build_sectioned_preserves_section_order() {
        let sections = vec![
            section("First", &["A"]),
            section("Second", &["B"]),
            section("Third", &["C"]),
        ];
        let model = build_sectioned(&sections, 3);

        // 3 headers + 3 cell rows = 6 rows total
        assert_eq!(model.rows.len(), 6);

        // Headers appear in order.
        let headers: Vec<&str> = model
            .rows
            .iter()
            .filter_map(|r| {
                if let SectionRow::Header { label, .. } = r {
                    Some(label.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(headers, vec!["First", "Second", "Third"]);
    }

    #[test]
    fn build_sectioned_omits_empty_sections() {
        let sections = vec![
            section("Non-empty", &["A"]),
            section("Empty", &[]),
            section("Also non-empty", &["B"]),
        ];
        let model = build_sectioned(&sections, 3);

        // Only 2 non-empty sections → 2 headers + 2 cell rows = 4 rows.
        assert_eq!(model.rows.len(), 4);
        let headers: Vec<&str> = model
            .rows
            .iter()
            .filter_map(|r| {
                if let SectionRow::Header { label, .. } = r {
                    Some(label.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(headers, vec!["Non-empty", "Also non-empty"]);
    }

    #[test]
    fn build_sectioned_section_idx_matches_original_position() {
        let sections = vec![
            section("First", &["A"]),
            section("Empty", &[]),       // idx=1, skipped
            section("Third", &["B"]),    // idx=2
        ];
        let model = build_sectioned(&sections, 3);

        // Headers must reference the original section indices.
        let idxs: Vec<usize> = model
            .rows
            .iter()
            .filter_map(|r| {
                if let SectionRow::Header { section_idx, .. } = r {
                    Some(*section_idx)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(idxs, vec![0, 2]);
    }

    #[test]
    fn build_sectioned_flat_path_unchanged_by_addition() {
        // Regression guard: build_grid must be identical before and after
        // adding the sectioned builder.
        let model = build_grid(&entries(&["A", "B", "C"]), 2);
        assert_eq!(model.cols, 2);
        assert_eq!(model.row_count(), 2);
        assert_eq!(model.cells[0], cell(0, 0, "A"));
        assert_eq!(model.cells[1], cell(0, 1, "B"));
        assert_eq!(model.cells[2], cell(1, 0, "C"));
    }
}
