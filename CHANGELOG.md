# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.3.0] - 2026-08-15

### Added
- Click-to-launch on Actions cards: clicking a simple card's body (icon + name) now
  launches its resolved command through the same capture pipeline as the Terminal
  view, with success/failure status feedback
- Variant-group cards: commands sharing a `variant_group` (e.g. the 4 `sftp-sync`
  variants, `email-to-markdown`, `lyremember`) now render as a single card with an
  `egui::ComboBox` variant selector and a dedicated "Lancer" button, instead of one
  flat card per variant
- Application removal priority report: Windows/Linux application collectors, local
  usage-history tracking, and a recommendation/scoring engine surfaced in a new
  Applications view

### Fixed
- Accented characters (app names, size labels) in the Applications view no longer
  corrupt JSON parsing — the bundled Python recommendation subprocess now forces
  `PYTHONIOENCODING=utf-8`
- Automations view: a scheduled task with a genuinely null `Author` (several
  built-in Windows tasks) no longer breaks the whole refresh with a misleading
  "réponse PowerShell inattendue" error
- Category management moved out of the Actions view's `CollapsingHeader` into a
  dedicated Préférences nav tab, freeing vertical space for action cards

## [0.2.0] - 2026-08-05

### Added
- `scripts/winclean`: dry-run-first Windows disk cleaner, porting the safety model of
  the Linux [`sysclean`](https://github.com/RebelliousSmile/sysclean) onto Windows
  filesystem semantics. It reuses `scripts/system_inventory` as a **read-only**
  discovery layer and never modifies it. Nothing is removed without an explicit
  `--apply`, at every level.
- Three cleanup levels:
  - `safe` — dev build artefacts (`target/`, `node_modules`, `__pycache__`,
    package-manager caches), all regenerable
  - `moderate` — browser and editor caches, `%TEMP%`, `CrashDumps`,
    `docker system prune` (never volumes)
  - `aggressive` — `$Recycle.Bin` on every fixed volume behind a `--trash-days` age
    floor, and the MSI Package Cache behind its own dedicated confirmation
- Safety layer: protected-path matching on the resolved, casefolded path including
  subtrees; `\\?\` long-path prefixing confined to the removal primitive; path sanity
  thresholds; a `--max-delete-bytes` ceiling that aborts the whole plan before the
  first deletion; reparse points deleted as entries and never descended into; Recycle
  Bin routing through `SHFileOperationW`, with candidates marked `no-undo` when it
  cannot apply
- Locked files (`WinError 32`) reported as a first-class partial outcome with a
  `failed` byte count, never a crash and never a false "cleaned" claim
- Allowlisted JSON config file (`%APPDATA%\winclean\winclean.json`) that can only
  restrict behaviour: disable modules, add protected paths, lower the ceiling, raise
  the Recycle Bin age floor. An unknown key aborts before any discovery output
- JSONL history of destructive runs (`%LOCALAPPDATA%\winclean\history.jsonl`),
  queryable with `--history N`
- Estimated-vs-measured comparison report, from two independent walks, in both the
  printed table and the `--json` payload; unmeasurable is `null`, never `0`
- Process detection is informative only: a running browser is reported, never killed
- ADR `aidd_docs/memory/internal/decisions/winclean-separate-package.md` recording why
  winclean is a separate package rather than a mode of `system_inventory`

## [0.1.0] - 2026-07-06

### Added
- Initial WinFXStart scaffold: native Win32 UI host (closes #1)
- Command executor via the Win32 Process API (closes #2)
- JSON persistence (models + load/save) (closes #3)
- Registry `Run` key startup module with boot sync wiring (closes #4)
- Custom PNG icons on buttons (closes #5)
- Category/group system for actions (closes #6)
- Favorite toggling as a pure model operation with persistence round-trip (issue #7 Phase 1)
- `cell_size` / control-id pure helpers in `xaml_gen` (issue #7 Phase 2)
- Dashboard reworked into 3 isolated views with a native menu bar
- Actions view reworked in owner-draw Fluent style (Lot 1)
- Dependency cleanup audit script (`scripts/deps_audit`)
- `scripts/system_inventory`: read-only Windows dev-machine disk inventory tool
  covering registry uninstall entries, AppData/dotfolders/ProgramData, Scoop/Choco,
  the `PATH` environment variable (dead-entry flagging), and Docker/WSL `.vhdx` sizing

### Fixed
- `system_inventory`: avoid double-counting Docker/WSL bytes via a lexical exclude-path
  match (instead of a per-entry `Path.resolve()`) and by stripping the `\\?\` extended-length
  path prefix before comparison

### Removed
- SFTP directory retrieval script
