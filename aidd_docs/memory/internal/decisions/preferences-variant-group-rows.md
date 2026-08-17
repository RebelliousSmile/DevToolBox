# Préférences view collapses variant-group commands into one row

- Date: 2026-08-17
- Status: Accepted

## Context

The preferences-config-editor feature (full action CRUD in the Préférences
view, see `memory/internal/decisions/preferences-dedicated-view.md`)
initially rendered one row per
`storage::Command`, flat. For commands sharing a `variant_group` (e.g.
Email to Markdown's Pro/Perso variants), that meant one Préférences row per
variant, while the Actions view already collapses the same group into a
single card (`partition_by_variant_group`). Reported by the user as
inconsistent: "pour une même application, il faudrait que ce soit une seule
ligne [...] et ensuite dans modifier, pouvoir gérer les options/arguments."

## Decision

Préférences rows are now built via `partition_preferences_rows`, returning
`PreferencesRow::Single(Command)` or `PreferencesRow::Group { key,
group_name, icon, variants }` — the Préférences-view analogue of the Actions
view's `CardData`. A group row starts collapsed (one line, group-level
⬆/⬇/Supprimer); an expand toggle (`EguiApp::expanded_groups`, session-only)
reveals each variant as its own sub-row reusing the existing single-command
edit/move/delete controls and `ActionForm`. Deleting a group row removes
every variant via the new `remove_command_group`.

## Consequences

- `move_command`/`move_command_group`/`move_variant` in
  `src/storage/commands.rs` treat a lone command or a whole variant group as
  one addressable "atom" (`effective_key`/`atom_span`) so group reordering
  moves the whole block, not individual variants.
- **Gotcha**: swapping an atom with its nearest same-bucket neighbor across
  intervening other-category commands is NOT a single `rotate_left`/
  `rotate_right` — for atoms `[N, J, M]` (N=neighbor, J=junk, M=moving) the
  desired result `[M, J, N]` needs explicit slice extraction + `Vec::splice`
  of the three blocks; a rotate produces `[J, M, N]` instead when block
  lengths differ. Caught by `cargo test`, not by review — re-verify with the
  full suite before touching `move_atom` again.
- A collapsed-by-default group row hides its variants' controls (star,
  Modifier, Supprimer) behind the expand toggle; any test clicking a
  variant-level control must expand the group first (see
  `egui_app.rs::tests::favorite_toggle_on_a_grouped_card_only_affects_the_selected_variant`).
