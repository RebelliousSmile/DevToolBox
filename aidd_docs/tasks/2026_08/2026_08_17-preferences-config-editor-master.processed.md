---
name: master_plan
status: implemented
description: Parent plan orchestrating a Preferences configuration editor for DevToolBox, so a user can create/edit/delete an action (and reassign its category, icon, favorite, shortcut) from the running app instead of hand-editing config.json and restarting. Source: brainstorm + challenge + shadow-areas refinement session on 2026-08-17 (aidd_docs/tasks/2026_08/2026_08_17-preferences-config-editor-spec.md)
argument-hint: N/A
---

# Master Plan: Preferences Configuration Editor

## Overview

- **Goal**: Add a unified categories+actions view to the Preferences screen with full CRUD on an action (name, executable+arguments, category, icon, favorite, shortcut), backed by new storage functions and a validated round-trip against the existing command tokenizer, so no action ever needs manual `config.json` editing again.
- **Risk Score**: 3/10
  - 5+ files/modules affected (+3): `src/ui/egui_app.rs`, `src/storage/commands.rs`, `src/storage/categories.rs`, `src/storage/mod.rs`, `src/storage/slug.rs` (new), `src/ui/icon_picker.rs` (new), `src/ui/command_form.rs` (new)
  - No breaking public API changes, no schema/data migration (`Command`/`Category` field sets are unchanged), no major refactor of existing working code, no external dependency upgrade
- **Branch**: `feature/preferences-config-editor/`

## Source

- Refined request: `aidd_docs/tasks/2026_08/2026_08_17-preferences-config-editor-spec.md` (brainstorm approved, 3 challenge deal-breakers corrected, 4 shadow-areas blockers corrected)
- Shadow report: `aidd_docs/tasks/2026_08/2026_08_17-preferences-config-editor-spec-shadow-report.md`

## Child Plans

| #   | Plan                                     | File                                                | Status  | Validated |
| --- | ----------------------------------------- | ---------------------------------------------------- | ------- | --------- |
| 1   | Storage layer & id generation             | `./2026_08_17-preferences-config-editor-part-1.md`   | done    | [x]       |
| 2   | UI building blocks (icon picker + form)   | `./2026_08_17-preferences-config-editor-part-2.md`   | done    | [x]       |
| 3   | Unified Preferences view integration      | `./2026_08_17-preferences-config-editor-part-3.md`   | done    | [x]       |

<!-- RULE: Plan N+1 blocked until Plan N checkbox checked -->

## Validation Protocol

1. Complete Plan 1 (storage CRUD + reserved-name guard + slug utility), run `cargo test --lib storage::`.
2. [x] Checkpoint 1: user confirms Plan 1's storage API is correct before UI work builds on it.
3. Unblock Plan 2 (icon picker + command form widgets, standalone/unit-tested), run `cargo test --lib ui::icon_picker:: ui::command_form::`.
4. [x] Checkpoint 2: user confirms the widgets' behavior (round-trip validation, curated icon set) before wiring them into a real screen.
5. Unblock Plan 3 (unified Preferences view, full wiring, confirmation dialog reuse), run full workspace `cargo test` + manual click-through.
6. [ ] Final: Integration test — a real user creates, edits, and deletes an action from the running app, confirmed live (no restart) and confirmed against a blocking confirmation dialog on delete.

## Estimations

- **Confidence**: 9/10
- **Duration**: Not estimated in wall-clock time — see each part's own confidence/risk register.
