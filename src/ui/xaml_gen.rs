//! Grid-model generator — the future XAML / WinUI 3 seam.
//!
//! Currently produces an in-memory layout description consumed by the Win32
//! child-control host in `app.rs`. When WinUI 3 / XAML Islands become
//! available in Rust, this module is the insertion point (Decision D1).
//!
//! ## Layout helpers (issue #7)
//!
//! - [`cell_size`]: derives button (width, height) from `icon_size`, clamped to
//!   a sane non-degenerate range so dimensions are always non-zero.
//! - [`assign_control_ids`]: assigns unique `u16` ids (base 1000+) to grid cells
//!   that carry a `command_id`, returning an ordered `Vec<(u16, String)>`.
//! - [`command_for_id`]: resolves a `u16` control id back to a command id string.

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
    #[allow(dead_code)] // used by unit tests; kept as a public convenience.
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

#[allow(dead_code)] // convenience accessors exercised by unit tests.
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

// ---------------------------------------------------------------------------
// Layout helpers (issue #7)
// ---------------------------------------------------------------------------

/// Minimum cell dimension (width and height) in pixels.
///
/// Enforces a sane floor so buttons are never invisible regardless of the
/// `icon_size` value stored in config.
const CELL_MIN: u32 = 40;

/// Maximum cell dimension in pixels.
///
/// Prevents absurdly large buttons for out-of-range `icon_size` values.
const CELL_MAX: u32 = 256;

/// Fixed label-area padding added to `icon_size` to produce the cell height.
///
/// The height accommodates the icon plus a text label below it.
const LABEL_PAD: u32 = 20;

/// Fixed horizontal padding added to `icon_size` to produce the cell width.
///
/// Ensures the label is not clipped for typical command names.
const WIDTH_PAD: u32 = 16;

/// Derive button cell dimensions from `icon_size`.
///
/// Formula:
/// - width  = clamp(icon_size + WIDTH_PAD,  CELL_MIN, CELL_MAX)
/// - height = clamp(icon_size + LABEL_PAD,  CELL_MIN, CELL_MAX)
///
/// Both dimensions are guaranteed to be in `[CELL_MIN, CELL_MAX]` and thus
/// always non-zero (AC3 — Decision D6).
///
/// The result is monotonically non-decreasing in `icon_size` within the
/// unclamped range `[CELL_MIN − WIDTH_PAD, CELL_MAX − WIDTH_PAD]` for width
/// and `[CELL_MIN − LABEL_PAD, CELL_MAX − LABEL_PAD]` for height.
pub fn cell_size(icon_size: u32) -> (u32, u32) {
    let w = icon_size
        .saturating_add(WIDTH_PAD)
        .clamp(CELL_MIN, CELL_MAX);
    let h = icon_size
        .saturating_add(LABEL_PAD)
        .clamp(CELL_MIN, CELL_MAX);
    (w, h)
}

/// Private base for button control ids (avoids id 0 and low system ids).
///
/// Win32 uses 0 as "no id"; ids below 1000 may collide with dialog control
/// ids in other contexts.  Starting at 1000 gives a private, collision-free
/// space for this window's child buttons (Decision D4).
const CONTROL_ID_BASE: u16 = 1000;

/// Assign unique `u16` control ids to all grid cells that carry a `command_id`.
///
/// Ids are allocated sequentially starting from [`CONTROL_ID_BASE`] and are
/// stable within one build (i.e. the same list of cells always yields the same
/// mapping).  They are NOT stable across rebuilds — the map is rebuilt from
/// scratch on every `UiHost::new` / `UiHost::reload` (Decision D3).
///
/// Returns an ordered `Vec<(control_id, command_id)>` where the index in the
/// vec matches the button creation order in `app.rs`.  Cells without a
/// `command_id` are silently skipped.
pub fn assign_control_ids(cells: &[GridCell]) -> Vec<(u16, String)> {
    cells
        .iter()
        .filter_map(|cell| cell.command_id.as_deref().map(|cid| cid.to_string()))
        .enumerate()
        .map(|(i, cid)| {
            // Saturating add keeps us within u16 range; in practice the number
            // of buttons is far below 64 k so this never saturates.
            let id = CONTROL_ID_BASE.saturating_add(i as u16);
            (id, cid)
        })
        .collect()
}

/// Resolve a `u16` control id to its `command_id` string.
///
/// `map` is the slice returned by [`assign_control_ids`].
/// Returns `None` when `control_id` is not present in `map`.
#[allow(dead_code)] // resolver kept for callers that don't hold the id→cmd map.
pub fn command_for_id(map: &[(u16, String)], control_id: u16) -> Option<&str> {
    map.iter()
        .find(|(id, _)| *id == control_id)
        .map(|(_, cid)| cid.as_str())
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
            section("Empty", &[]),    // idx=1, skipped
            section("Third", &["B"]), // idx=2
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

    // -----------------------------------------------------------------------
    // cell_size tests (issue #7 Phase 2)
    // -----------------------------------------------------------------------

    #[test]
    fn cell_size_normal_value_passes_through_with_padding() {
        // icon_size = 48: w = 48+16 = 64, h = 48+20 = 68 (both in [40,256])
        let (w, h) = cell_size(48);
        assert_eq!(w, 64);
        assert_eq!(h, 68);
    }

    #[test]
    fn cell_size_zero_clamps_to_min() {
        let (w, h) = cell_size(0);
        assert_eq!(w, CELL_MIN, "width must be CELL_MIN for icon_size=0");
        assert_eq!(h, CELL_MIN, "height must be CELL_MIN for icon_size=0");
    }

    #[test]
    fn cell_size_tiny_clamps_to_min() {
        // icon_size = 1: w = 1+16 = 17 < 40 → clamped; h = 1+20 = 21 < 40 → clamped
        let (w, h) = cell_size(1);
        assert_eq!(w, CELL_MIN);
        assert_eq!(h, CELL_MIN);
    }

    #[test]
    fn cell_size_huge_clamps_to_max() {
        let (w, h) = cell_size(u32::MAX);
        assert_eq!(
            w, CELL_MAX,
            "width must be CELL_MAX for very large icon_size"
        );
        assert_eq!(
            h, CELL_MAX,
            "height must be CELL_MAX for very large icon_size"
        );
    }

    #[test]
    fn cell_size_large_value_clamps_to_max() {
        // icon_size = 512: both padded values exceed 256 → clamped
        let (w, h) = cell_size(512);
        assert_eq!(w, CELL_MAX);
        assert_eq!(h, CELL_MAX);
    }

    #[test]
    fn cell_size_never_zero() {
        for icon_size in [
            0u32,
            1,
            8,
            16,
            24,
            32,
            48,
            64,
            80,
            96,
            128,
            256,
            512,
            u32::MAX,
        ] {
            let (w, h) = cell_size(icon_size);
            assert!(w > 0, "width must never be 0 (icon_size={icon_size})");
            assert!(h > 0, "height must never be 0 (icon_size={icon_size})");
        }
    }

    #[test]
    fn cell_size_monotonic_in_unclamped_range() {
        // In the unclamped range both w and h should grow with icon_size.
        // CELL_MIN=40, WIDTH_PAD=16 → icon_size >= 24 gives w unclamped at 40+.
        // CELL_MAX=256, WIDTH_PAD=16 → icon_size <= 240 keeps w <= 256.
        let sizes = [24u32, 32, 48, 64, 80, 96, 128, 200, 240];
        for i in 0..sizes.len() - 1 {
            let (w0, h0) = cell_size(sizes[i]);
            let (w1, h1) = cell_size(sizes[i + 1]);
            assert!(
                w1 >= w0,
                "width must be non-decreasing: cell_size({})={w0} > cell_size({})={w1}",
                sizes[i],
                sizes[i + 1]
            );
            assert!(
                h1 >= h0,
                "height must be non-decreasing: cell_size({})={h0} > cell_size({})={h1}",
                sizes[i],
                sizes[i + 1]
            );
        }
    }

    #[test]
    fn cell_size_within_bounds() {
        for icon_size in (0u32..=512).step_by(8) {
            let (w, h) = cell_size(icon_size);
            assert!(
                (CELL_MIN..=CELL_MAX).contains(&w),
                "width {w} out of [{CELL_MIN},{CELL_MAX}]"
            );
            assert!(
                (CELL_MIN..=CELL_MAX).contains(&h),
                "height {h} out of [{CELL_MIN},{CELL_MAX}]"
            );
        }
    }

    // -----------------------------------------------------------------------
    // assign_control_ids / command_for_id tests (issue #7 Phase 2)
    // -----------------------------------------------------------------------

    fn cell_with_id(label: &str, command_id: Option<&str>) -> GridCell {
        GridCell {
            row: 0,
            col: 0,
            label: label.to_string(),
            icon: String::new(),
            command_id: command_id.map(|s| s.to_string()),
        }
    }

    #[test]
    fn assign_control_ids_empty_cells_returns_empty() {
        let map = assign_control_ids(&[]);
        assert!(map.is_empty());
    }

    #[test]
    fn assign_control_ids_skips_cells_without_command_id() {
        let cells = vec![
            cell_with_id("No ID", None),
            cell_with_id("With ID", Some("notepad")),
        ];
        let map = assign_control_ids(&cells);
        assert_eq!(map.len(), 1, "only cells with command_id get an id");
        assert_eq!(map[0].1, "notepad");
    }

    #[test]
    fn assign_control_ids_starts_at_base() {
        let cells = vec![cell_with_id("A", Some("cmd_a"))];
        let map = assign_control_ids(&cells);
        assert_eq!(
            map[0].0, CONTROL_ID_BASE,
            "first id must be CONTROL_ID_BASE"
        );
    }

    #[test]
    fn assign_control_ids_are_unique_and_sequential() {
        let cells = vec![
            cell_with_id("Notepad", Some("notepad")),
            cell_with_id("Cmd", Some("cmd")),
            cell_with_id("Paint", Some("paint")),
        ];
        let map = assign_control_ids(&cells);
        assert_eq!(map.len(), 3);

        // Ids must be unique.
        let ids: Vec<u16> = map.iter().map(|(id, _)| *id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "control ids must be unique");

        // Ids must be sequential from base.
        assert_eq!(ids[0], CONTROL_ID_BASE);
        assert_eq!(ids[1], CONTROL_ID_BASE + 1);
        assert_eq!(ids[2], CONTROL_ID_BASE + 2);
    }

    #[test]
    fn assign_control_ids_order_matches_cell_order() {
        let cells = vec![
            cell_with_id("A", Some("cmd_a")),
            cell_with_id("B", Some("cmd_b")),
            cell_with_id("C", Some("cmd_c")),
        ];
        let map = assign_control_ids(&cells);
        assert_eq!(map[0].1, "cmd_a");
        assert_eq!(map[1].1, "cmd_b");
        assert_eq!(map[2].1, "cmd_c");
    }

    #[test]
    fn command_for_id_resolves_assigned_id() {
        let cells = vec![
            cell_with_id("Notepad", Some("notepad")),
            cell_with_id("Cmd", Some("cmd")),
        ];
        let map = assign_control_ids(&cells);
        assert_eq!(
            command_for_id(&map, CONTROL_ID_BASE),
            Some("notepad"),
            "first id must resolve to first command"
        );
        assert_eq!(
            command_for_id(&map, CONTROL_ID_BASE + 1),
            Some("cmd"),
            "second id must resolve to second command"
        );
    }

    #[test]
    fn command_for_id_unknown_id_returns_none() {
        let cells = vec![cell_with_id("Notepad", Some("notepad"))];
        let map = assign_control_ids(&cells);
        assert_eq!(
            command_for_id(&map, 9999),
            None,
            "unassigned id must resolve to None"
        );
        assert_eq!(command_for_id(&map, 0), None, "id 0 must resolve to None");
    }

    #[test]
    fn command_for_id_on_empty_map_returns_none() {
        let map: Vec<(u16, String)> = Vec::new();
        assert_eq!(command_for_id(&map, CONTROL_ID_BASE), None);
    }
}
