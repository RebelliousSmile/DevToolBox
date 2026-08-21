---
name: master_plan
status: pending
description: Parent plan extending the Docker tab with (A) a "Stacks" section listing every docker-compose file found under $HOME and driving them in detached mode, (B) a Ports column with cross-container/cross-stack conflict detection, and (C) a dormancy signal plus grouped deletion of unused containers/images/volumes. Source: brainstorm + shadow-areas session on 2026-08-21 (aidd_docs/tasks/2026_08/2026_08_21-docker-compose-ports-cleanup-brainstorm.md, 21 gaps closed).
argument-hint: N/A
---

# Master Plan: Docker Stacks, Ports and Dormancy

## Overview

- **Goal**: Turn the read-only Docker tab (v0.6.0) into an operational cockpit for parallel stacks —
  1. **Stacks**: a home-wide scan finds every compose file, `docker compose config --format json` reads its services and published ports without a daemon, and each stack can be started detached / stopped / destroyed with its output streamed into an inline log under its row.
  2. **Ports**: every container shows its published bindings, and any host-port collision — between two running containers, or between a running container and a stack about to start — is flagged before it bites.
  3. **Dormancy**: containers, images and volumes carry a `dormant` badge when their date is older than a configurable threshold (days, default 60), and dormant items can be multi-selected and deleted in one confirmed batch.
- **Risk Score**: 8/10
  - Config schema gains persisted fields (+3): `default_settings.dormant_after_days` (Part 1) and a top-level `docker_stacks` list (Part 2) — both additive, both `#[serde(default)]`, both covered by a lossless round-trip test; `version` stays untouched (Decision D4, no migration).
  - 5+ files/modules affected (+3): `src/linux/docker.rs`, `src/linux/compose.rs` (new), `src/ui/ports.rs` (new), `src/ui/docker_view.rs`, `src/ui/compose_view.rs` (new), `src/ui/egui_app.rs`, `src/ui/terminal_view.rs`, `src/storage/models.rs`, `config/default.json`.
  - External dependency (+2): `walkdir` — the first crate added since the multi-OS transformation (8 deps today).
- **Branch**: `feature/docker-stacks-ports/`

## Source

- Refined request: `./2026_08_21-docker-compose-ports-cleanup-brainstorm.md` — approved, with a `## Décisions (2026-08-21)` section that **takes precedence over the body** wherever they diverge (two body hypotheses are explicitly annulled: the ~6-level scan depth and in-process `.env` resolution).
- Shadow report: `./2026_08_21-docker-compose-ports-cleanup-brainstorm-shadow-report.md` — `status: clean`, 21/21 gaps closed.
- Prior scope still in force: `./2026_08_19-docker-tab-brainstorm.md` (targeted deletion only, never `--force`, manual refresh, tab hidden only when the `docker` binary is missing).

## Child Plans

| #   | Plan                                             | File                                                        | Status      | Validated |
| --- | ------------------------------------------------ | ----------------------------------------------------------- | ----------- | --------- |
| 1   | Ports, dates and dormancy on the existing lists  | `./2026_08_21-docker-compose-ports-cleanup-part-1.md`        | implemented | [ ]       |
| 2   | Compose stacks — discovery, ports, detached run  | `./2026_08_21-docker-compose-ports-cleanup-part-2.md`        | implemented | [ ]       |
| 3   | Grouped deletion of dormant resources            | `./2026_08_21-docker-compose-ports-cleanup-part-3.md`        | implemented | [ ]       |

<!-- RULE: Plan N+1 blocked until Plan N checkbox checked -->

Ordering rationale: Part 1 builds the three primitives the rest consumes — the pure port model in `src/ui/ports.rs`, the dormancy dates, and the grouped `docker inspect` pass — and ships alone as a useful increment (ports + conflicts + badges). Part 2 reuses the port model for declared-but-not-running stacks and **extends Part 1's inspect template** with the two compose labels rather than parsing `docker ps`'s flat `Labels` string. Part 3 reuses the dormancy flags as its selection criterion and the `used_by` mapping Part 1 exposes. No part depends on a later one; each later part extends an earlier seam that was designed for it, which is why the inspect helper is introduced as `inspect_containers` from the outset.

## Validation Protocol

1. Complete Plan 1 (ports parsing/conflicts, grouped `docker inspect` dates, threshold setting, badges), run `cargo fmt --check && cargo clippy -- -D warnings && cargo test`.
2. [ ] Checkpoint 1: on this machine, the Ports column shows the real bindings
   of the running containers; every orphan volume older than the threshold
   carries a `dormant · N j` badge while a more recent one does not; and the
   threshold set in Préférences survives an app restart.

   **Amended 2026-08-21 — the original "two running containers on the same
   host port flag each other" criterion was unverifiable and has been
   dropped.** The kernel refuses the second bind, so the second container
   never reaches `running`: a running/running collision cannot be produced on
   purpose, and none of this machine's 25 containers can exhibit one. The
   conflict badge is therefore exercised by unit tests only until Part 2
   supplies the case it was actually designed for — a *declared* stack whose
   compose file publishes a port a running container already holds
   (`OwnerKind::DeclaredStack`). Nothing in the port model changes; only this
   checkpoint's expectation was wrong.
3. Unblock Plan 2 (walkdir scan, `compose config`, Stacks section, streamed `up -d`/`stop`/`down`), run the same triple.
4. [ ] Checkpoint 2: the scan finds the 13 compose files known to exist under `$HOME`, a stack starts detached with its log streaming inline and no UI freeze, and its state reads `tourne` / `partielle` / `arrêtée` correctly — including the `lab-db-init` case (`Exited (0)` one-shot must not read `partielle`).
5. Unblock Plan 3 (multi-select, single confirmation, ordered batch, report), run the same triple.
6. [ ] Final: select a dormant container + its image + an orphan volume, confirm once, and get a batch report where a deliberate failure (e.g. a volume re-attached meanwhile) does not abort the rest.

## Risk register (cross-cutting)

| Risk | Impact | Mitigation |
| ---- | ------ | ---------- |
| `src/ui/egui_app.rs` carries ~649 uncommitted lines of an unrelated in-flight feature (card badges/`Command.info`), and all three parts modify that same file | merge conflicts, or a half-finished feature dragged into this branch | Recorded, not resolved by this plan: branch off the current working tree as-is, or commit that feature first — the user's call. Each part touches `egui_app.rs` in additive blocks (new methods, new match arms) rather than editing existing rendering code, to keep the conflict surface small. |
| `walkdir` unavailable offline / vendored builds | Part 2 cannot build | Single-crate, no-transitive-surprise dependency; if it must be avoided, a ~40-line `std::fs` recursive walk implements the same `filter_entry` contract — noted as the fallback in Part 2. |
| Three grouped `docker inspect` calls added to every refresh (measured need: `ps`/`images` return 12-char ids and no volume date at all, so the dates cannot come from the listings) | slower snapshot | Chunked (50 ids/call) and measured at Checkpoint 1; if the refresh regresses noticeably the inspect pass moves behind an explicit trigger, exactly like the existing `ComputeVolumeSizes` action. |
| Two additive schema touches in two parts | a config written by Part 2 is unreadable by a Part 1-only build | Both fields are `#[serde(default)]`/`skip_serializing_if`, so an older build ignores them and rewrites the file without loss of the keys it does know; the round-trip test in each part asserts exactly that. |

## Estimations

- **Confidence**: 8/10 — every external contract used here was measured on this machine during the shadow-areas pass (`compose config --format json` at 89 ms and daemon-independent; `ps -a` already carrying `Ports` and the compose labels; `dangling=true` as the volume-orphan signal). The residual unknowns are UI-shaped (inline log placement, selection ergonomics), not contract-shaped.
- **Duration**: Not estimated in wall-clock time — see each part's own confidence.
