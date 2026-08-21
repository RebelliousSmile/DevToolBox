# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.7.0] - 2026-08-21

### Added
- Docker tab: compose stacks — `$HOME` walk for the four names compose itself
  looks for (`docker-compose.y{a,}ml`, `compose.y{a,}ml`), pruning
  `node_modules`, `target`, `.venv` and friends; per-stack state (running /
  partial / stopped / unknown) and `up -d` / `down` / `restart` launched
  detached with their output streamed into an anchored bottom log panel
- Docker tab: published-port column on containers, with conflict detection
  across running containers and compose declarations so two stacks fighting
  for the same host port are visible before launch
- Docker tab: dormancy badges (last-used date against the configurable
  `dormant_after_days` threshold, persisted in `config.json`) on containers,
  images and volumes, plus a cross-list batch selection and a single grouped
  « Supprimer la sélection » that reports per-item success or failure instead
  of aborting on the first error
- Full monochrome Noto Emoji font (`assets/fonts/NotoEmoji-Regular.ttf`, OFL)
  appended to egui's fallback chain: user-picked action and category icons
  (🧹, 🏗, 🤖, 🧪) no longer render as tofu

### Changed
- Docker tab: the three lists (Conteneurs / Images / Volumes) become tabs
  instead of one long stacked scroll; each tab label carries its row count and,
  when a batch spans several lists, its selected count
- Command-form reorder buttons use ⬆/⬇ and the collapsing headers ⏵/⏷, the
  plain ↑/↓ and ▸/▾ being covered by no font in the chain

### Fixed
- Windows: the temp path built from the libtest thread name is sanitized, its
  `:` separators being illegal in Windows filenames (OS error 123)
- `@python` action resolution delegates to `python_runtime::python_for_script`
  and so honours the host's venv layout (`.venv/bin/python` on POSIX,
  `.venv\Scripts\python.exe` on Windows)

## [0.6.0] - 2026-08-19

### Added
- Docker tab (Linux-only, shown when the `docker` binary is installed): local
  dashboard listing containers (stop/remove with state-based gating), images
  (used badge, remove by tag or id, never `--force`) and volumes (orphan badge,
  targeted removal only, no prune); a daemon that is down shows an in-tab
  retry message instead of hiding the tab
- Docker tab: removal confirmations state the space reclaimed — container
  writable layer (`docker ps -a --size`), image size with a sole-tag vs
  multi-tag distinction, volume size via the on-demand « Calculer les
  tailles » button (`docker system df -v` merged into the snapshot)
- winclean: Linux dev-caches module (`mod_linux_dev`), extended Linux
  system/package modules and registry contract coverage

### Changed
- Automations view/Linux data source: user-scope filtering refined and the
  view aligned with the automations-user-scope decision record
- Scroll bars switch from egui's invisible-until-hover floating style to
  `ScrollStyle::thin()` so overflowing content is discoverable

### Fixed
- Docker view scrolls vertically (sections were unreachable below the fold)
- Post-merge fallout from the Windows/Linux branches: duplicated
  `open_native_tool` deduplicated, clippy/rustfmt on merged files, stale doc
  references to the deleted `src/ui/app.rs` dropped

## [0.5.0] - 2026-08-18

### Added
- Nettoyage view: winclean JSON client (model, parser, rows, spawn) wired into a new
  `ActiveView::Cleanup` tab — module rows with measured-first sizes, safe-only
  "Nettoyer" buttons, run badges (success/partial failure/interrupted), error banner
  and stale marker, behind a blocking confirmation dialog and the shared
  command-busy guard
- Applications view: `DisplayVersion` is now read from the Windows Uninstall
  registry (same path as `Publisher`) and surfaced as a new Version column

### Fixed
- Automations: scheduled-task fetch now filters out `\Microsoft\*` system tasks,
  matching the view's own "OS tasks hidden" label instead of drowning user tasks in
  200+ system rows
- Applications grid, cleanup modules grid, automations grid and Terminal output
  switch to `ScrollArea::both()` so content wider than the window stays reachable
  via horizontal scroll instead of being clipped
- Nettoyage view: the Bibliothèques section now renders above the installed-apps
  report so a completed analysis is visible without scrolling past the window's
  bottom edge
- Nettoyage view: the "Nettoyer" action column now sits right after the size
  column instead of trailing past wide filesystem paths
- Automations: `open_native_tool` behind the view button is now implemented

## [0.4.0] - 2026-08-17

### Added
- Préférences view: full CRUD on an action (name, executable+arguments,
  category, curated icon picker, favorite, shortcut), reusable ⬆/⬇ move
  buttons on categories and actions, and a blocking confirmation dialog on
  delete
- Préférences view: commands sharing a `variant_group` now collapse into a
  single row (mirroring the Actions view's grouped cards) with an
  expand/collapse toggle revealing each variant for individual edit, move,
  or delete
- `storage`: `add_command`/`update_command`/`remove_command`/
  `remove_command_group` command CRUD, `move_category`/`move_command`/
  `move_command_group`/`move_variant` reordering, `generate_slug`
  collision-free id generation, and a reserved-name guard on the
  "Sans catégorie" pseudo-category

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
