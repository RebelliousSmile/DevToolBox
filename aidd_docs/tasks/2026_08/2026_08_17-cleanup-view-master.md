---
name: master_plan
status: pending
description: Parent plan orchestrating the transformation of the Applications view into a "Nettoyage" view — installed-apps report kept on top, plus a "Bibliothèques" section listing every winclean module with size/path, on-demand analysis (clean.py --json) and per-module safe cleaning (--only X --apply --json), all through scripts/winclean with zero scan/delete logic in Rust. Source: brainstorm + shadow-areas refinement session on 2026-08-17 (aidd_docs/tasks/2026_08/2026_08_17-cleanup-view-brief.md)
argument-hint: N/A
---

# Master Plan: Cleanup View (Nettoyage)

## Overview

- **Goal**: Rename the Applications tab to « Nettoyage » and add a « Bibliothèques » section fed exclusively by `scripts/winclean/clean.py`: an « Analyser » button runs `--json --level moderate` in the background and fills per-module rows (size, paths, level); each safe row gets a « Nettoyer » button that, after a blocking confirmation dialog, runs `--only <module> --apply --json` and refreshes the row from the run payload (size from `measured`, badge from `freed` plus the `locked_paths`/`operation_failures` counts). No scan or deletion logic in Rust; module names are never hardcoded (OS-agnostic).
- **Risk Score**: 4/10
  - 5+ files/modules affected (+3): `src/ui/egui_app.rs`, `src/ui/applications_view.rs`, `src/ui/mod.rs`, `src/cleanup/` (new), `src/ui/cleanup_view.rs` (new)
  - External process contract (+1): parsing `clean.py --json` stdout (multiple JSON documents possible in apply mode) — mitigated by fixture-driven unit tests in Part 1 and script-side fixes allowed by the brief's DRY principle.
  - No breaking public API change, no config schema migration, no dependency upgrade (serde/serde_json already in use for `config.json`).
- **Branch**: `feature/cleanup-view/`

## Source

- Refined request: `aidd_docs/tasks/2026_08/2026_08_17-cleanup-view-brief.md` (brainstorm approved; 9 shadow gaps closed, decisions folded into the brief's « Décisions » section)
- Shadow report: `aidd_docs/tasks/2026_08/2026_08_17-cleanup-view-brief-shadow-report.md` (status: clean)

## Child Plans

| #   | Plan                                        | File                                     | Status  | Validated |
| --- | ------------------------------------------- | ---------------------------------------- | ------- | --------- |
| 1   | winclean JSON client (spawn + model)        | `./2026_08_17-cleanup-view-part-1.md`    | implemented | [x]   |
| 2   | Cleanup view rendering (pure UI)            | `./2026_08_17-cleanup-view-part-2.md`    | pending | [ ]       |
| 3   | Integration into EguiApp (rename + wiring)  | `./2026_08_17-cleanup-view-part-3.md`    | pending | [ ]       |

<!-- RULE: Plan N+1 blocked until Plan N checkbox checked -->

## Validation Protocol

1. Complete Plan 1 (serde model, stdout parser, module aggregation, background spawns), run `cargo test --lib cleanup::`.
2. [x] Checkpoint 1: user confirms the JSON client parses a real `clean.py --json --level moderate` run on this machine before any UI builds on it. (2026-08-17: real run, 1067 candidates → 11 module rows, sizes/partial flags/sort verified; probe test removed afterwards.)
3. Unblock Plan 2 (pure render function + row states), run `cargo test --lib ui::cleanup_view::`.
4. [ ] Checkpoint 2: user confirms row layout/states (greyed moderate rows, badges, error banner) before wiring.
5. Unblock Plan 3 (tab rename, state/channels, confirmation dialog, concurrency guard), run full `cargo test` + manual click-through.
6. [ ] Final: Integration test — user opens « Nettoyage », clicks « Analyser », sees real module sizes, cleans one safe module after the confirmation dialog, and the row shows « Nettoyé : X libérés » with its size refreshed from `measured`, without restarting the app.

## Estimations

- **Confidence**: 8/10 — the two proven in-repo patterns (background report via `applications::spawn_report`, pure view module via `applications_view.rs`) cover most of the work; residual unknown is the exact multi-document stdout shape in apply mode, isolated in Part 1.
- **Duration**: Not estimated in wall-clock time — see each part's own confidence/risk register.
