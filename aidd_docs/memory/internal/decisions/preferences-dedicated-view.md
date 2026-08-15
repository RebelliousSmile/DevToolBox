# Category management moves to a dedicated Préférences nav tab, not a dialog

- Date: 2026-08-15
- Status: Accepted

## Context

Category CRUD (rename/remove/add) lived inside a `CollapsingHeader` at the top
of the Actions view, eating vertical space away from the action cards it sits
above — flagged during manual Windows validation. The obvious alternative to
a dedicated view was extending the existing modal-dialog system
(`src/ui/dialogs.rs`'s `DialogKind`/`show`), since DevToolBox already has one.

## Decision

Category management became its own `ActiveView::Preferences` nav tab
(`src/ui/egui_app.rs`, alongside `Actions`/`Terminal`/`Automations`/
`Applications`) instead of a dialog. `render_categories_panel` was renamed to
`render_preferences_view` and moved out of `render_actions_view` entirely.

## Alternatives

- **Extend `dialogs.rs`'s `DialogKind` with a new variant carrying arbitrary
  widget content** — lost: `DialogKind` only supports fixed Info/Warn/Confirm
  text layouts today. Category management needs a text field per row (rename
  buffer), two buttons per row (rename/remove), and a 3-field add form — an
  arbitrary widget tree, not a text layout. Bending the dialog system to carry
  that would be a bigger, more invasive change than adding a nav tab.

## Consequences

- **This is the precedent for any future settings-like UI**, not just
  categories. The "Favori toggle should become a setting" feedback (still
  unactioned as of this decision) is the next candidate for the Préférences
  view rather than a dialog, for the same reason: it's an interactive control
  living inside a form, not a fixed confirmation/notice.
- `render_actions_view` keeps its "Afficher par catégories" checkbox (display
  toggle for card grouping) — only the CRUD panel moved. The checkbox and the
  CRUD panel are unrelated concerns that happened to share the same
  `CollapsingHeader` before this change.
