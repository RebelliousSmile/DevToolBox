# DESIGN.md

## Design Implementation

- **UI Framework**: `eframe`/`egui`, native Rust rendering without a WebView
- **Styling Method**: semantic tokens in `src/ui/theme.rs`, shared components and
  platform window materials behind `src/ui/native_window.rs`

## Design System

- **Theme**: light / dark (from `Settings.theme`, default `light`)
- **Icon size**: configurable (`Settings.icon_size`, default `80`)
- **Iconography**: emoji and custom PNG/SVG icons per command/category

## UI Patterns

- **Favorites grid**: visual grid of favorite commands (primary surface, `show_categories == false`)
- **Categories**: optional grouped view (`show_categories == true`), toggled by `Settings.show_categories`
- **Descriptions**: optional, toggled by `Settings.show_descriptions`
- **Search**: command search bar (planned)
- **Feedback**: visual success/error and execution feedback (planned)

## Category render rules (issue #6)

### Grouped-vs-flat render rule

The render mode is selected at `UiHost` construction time from `config.default_settings.show_categories`:

- `show_categories == false` (default): flat favorites-only button grid. Only commands with `is_favorite = true` are shown. This is the issue #1/#5 behavior, unchanged.
- `show_categories == true`: grouped view. ALL commands are shown, grouped under a per-category STATIC header label. Groups are ordered by `config.categories` order. Commands whose `category` id is empty or does not match any declared category are grouped under a synthetic "Sans catégorie" (Uncategorized) header at the end.

### Synthetic Uncategorized bucket

The Uncategorized group is a display-only, runtime-only concept:
- It is produced by `storage::group_commands_by_category` as a trailing `CategoryGroup { category: None, … }`.
- It is never added to `config.categories`.
- It is never serialized to JSON (`config.json` schema is unchanged — Decision D4).
- The header label "Sans catégorie" lives in the UI layer only.

### Orphan handling on remove_category (Decision D3)

When `storage::remove_category` is called, it:
1. Removes the `Category` from `config.categories`.
2. Clears (`= ""`) the `category` id of every command that referenced it.
3. Does NOT delete the commands — they re-bucket as Uncategorized on the next grouped render.

### Deferred CRUD UI seam

`storage::{add_category, rename_category, remove_category}` form the callable API for the future settings / alias-editor UI (issue #9). No interactive create/rename/delete widgets are built in issue #6. Callers persist mutations via `storage::save(&config)`.

## Accessibility

- **Keyboard navigation**: customizable per-command shortcuts (e.g. `Ctrl+N`)

The current measurable contract is `docs/visual-contract.md`: compact horizontal
navigation below 1024 px, 184 px sidebar above it, 160 ms meaningful transitions,
AA-oriented contrast assertions and no continuous repaint at idle. macOS uses system
fonts and vibrancy when available, Windows 11 uses Mica, and all targets retain a fully
opaque accessible fallback. `DEVTOOLBOX_REDUCE_TRANSPARENCY` provides a deterministic
qualification switch.
