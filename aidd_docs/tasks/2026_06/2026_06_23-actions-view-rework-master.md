---
name: master_plan
description: Parent plan orchestrating the two-lot Actions-view rework (owner-draw visuals, then argument-variant split-button)
argument-hint: N/A
---

# Master Plan: Actions View Rework (owner-draw visuals + argument-variant split-button)

## Overview

- **Goal**: Rework the WinFXStart Actions view in two sequential lots — first a Win32 owner-draw Fluent-style visual rework (Lot 1), then a backward-compatible argument-variant model with a native split-button (Lot 2).
- **Risk Score**: 7/10
  - Major refactoring of `src/ui/app.rs` button + paint path (+2)
  - 3+ modules affected: `app.rs`, `xaml_gen.rs`, `models.rs`, `Cargo.toml` (+2)
  - Persisted-model (JSON schema) evolution with strict backward-compat constraint (+3) — schema-migration-equivalent
- **Branch**: `feature/actions-view-rework/`

## Frozen decisions (from validated brainstorm — do NOT revisit)

- Lot 1 ships before Lot 2; each is independently shippable.
- Light theme only; dark theme is out of scope.
- No on-the-fly parameter input.
- No automatic migration of existing duplicated commands.
- Default variant = the base action (`command` field, no extra args).

## Child Plans

| #   | Plan                          | File                                          | Status      | Validated |
| --- | ----------------------------- | --------------------------------------------- | ----------- | --------- |
| 1   | Owner-draw visual rework      | `./2026_06_23-actions-view-rework-part-1.md`  | pending     | [ ]       |
| 2   | Argument-variant split-button | `./2026_06_23-actions-view-rework-part-2.md`  | blocked     | [ ]       |

<!-- Status values: pending, in-progress, done, blocked -->
<!-- RULE: Part 2 blocked until Part 1 checkbox checked. Part 1 is shippable without Part 2. -->

## Why two parts (independence)

- **Part 1** touches only the Win32 rendering/interaction path (`app.rs` + `Cargo.toml` features). It changes no persisted data and no public model. It is shippable on its own: owner-draw cards replace push-buttons, with identical click/launch behavior.
- **Part 2** introduces a new optional `variants` field on `Command` (serde-default, backward-compatible JSON) plus split-button rendering/hit-testing/menu. It builds on Part 1's owner-draw `WM_DRAWITEM` paint path but is a strictly additive layer: a command with no variants renders and behaves exactly as a Part 1 card.

## Cross-cutting technical decisions (apply to both parts)

- **Owner-draw message routing (RESOLVED)**: `WM_DRAWITEM` is sent by Windows to the owner-draw control's IMMEDIATE PARENT. Action buttons are parented to `actions_container`, so this message lands on `container_proc`, NOT on the subclassed top-level window. `container_proc` currently forwards only `WM_COMMAND`. It MUST be extended to handle `WM_DRAWITEM` (paint the card), routing it to the host paint logic. **Note: `WM_MEASUREITEM` is NOT applicable to `BS_OWNERDRAW` buttons** — Windows reserves it for list-boxes, combo-boxes, and menus only; button size comes from `SetWindowPos`. Do not add `WM_MEASUREITEM` handling. This is the single most important seam.
- **Hover-state strategy (RESOLVED)**: Classic `BS_OWNERDRAW` buttons do NOT receive hover state via `ODS_HOTLIGHT` automatically (that flag requires hot-tracking the classic button class does not provide). `ODS_SELECTED` (pressed) and `ODS_FOCUS` (keyboard focus) ARE delivered reliably in `DRAWITEMSTRUCT.itemState`. Therefore: pressed + focus are read from `ODS_*` flags; HOVER is tracked manually by per-button subclass that arms `TrackMouseEvent` on `WM_MOUSEMOVE` and clears state on `WM_MOUSELEAVE`, storing a hot-flag keyed by control id on the host, then `InvalidateRect` to repaint. A per-button subclass (not the container) is the clean owner of hover because mouse messages go to the button itself.
- **Cargo.toml feature additions (REQUIRED)**: current features lack the owner-draw structs and the mouse-tracking API. Add:
  - `Win32_UI_Controls` — provides `DRAWITEMSTRUCT`, `ODS_*` flags, `ODT_BUTTON`. (`MEASUREITEMSTRUCT` is also in this feature but NOT used — `WM_MEASUREITEM` is not sent for `BS_OWNERDRAW` buttons.)
  - `Win32_UI_Input_KeyboardAndMouse` — provides `TrackMouseEvent`, `TRACKMOUSEEVENT`, `TME_LEAVE`.
  - All GDI primitives (`CreateFontW`, `RoundRect`, `CreateRoundRectRgn`, `CreatePen`, `CreateSolidBrush`, `FillRect`, `SetBkMode`, `SetTextColor`, `SelectObject`, `DrawTextW`) are already reachable under the enabled `Win32_Graphics_Gdi`. `TrackPopupMenu` (Lot 2) is already reachable under the enabled `Win32_UI_WindowsAndMessaging`.

## Validation Protocol

1. Complete Part 1, run its `success_condition` (`cargo build` + `cargo test`), visually verify owner-draw cards + states.
2. [ ] Checkpoint 1: User confirms Lot 1 shipped and acceptable.
3. Unblock Part 2, complete it, run its `success_condition` (`cargo test` incl. new serde round-trip tests).
4. [ ] Final: Integration check — a command with variants shows a split-button; a command without variants is unchanged; existing config JSON still loads losslessly.

## Estimations

- **Confidence**: 9/10
- **Duration**: Lot 1 ~1.5-2 days; Lot 2 ~1-1.5 days.
