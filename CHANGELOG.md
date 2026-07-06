# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
