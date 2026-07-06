---
name: master_plan
description: Parent plan orchestrating the read-only Windows dev-machine disk inventory tool (v1 - raw inventory, no verdict), split into three additive scanner lots
argument-hint: N/A
---

# Master Plan: System Inventory (Windows dev-machine disk usage, read-only v1)

## Overview

- **Goal**: Build a modular, offline, read-only Python tool under `scripts/system_inventory/` that scans the places where Windows dev tooling leaves traces and prints a single inventory sorted by descending on-disk size, with a grand total and an optional `--json` output. v1 is pure visibility: NO active/inactive classification, NO deletion, NO registry/PATH writes.
- **Risk Score**: 3/10
  - 5+ new cooperating modules created in a new package (+3)
  - No breaking API changes, no schema migration, no refactor of existing code, no dependency upgrade (0)
  - Actual blast radius is low: read-only, isolated new folder next to `scripts/deps_audit/`, stdlib only.
- **Branch**: `feature/system-inventory/`

## Frozen decisions (from validated brainstorm — do NOT revisit)

- v1 is a RAW INVENTORY. No "obsolete" / "cleanup candidate" notion. No active/inactive detection (that is v2: pivot files, AI-gen recognition rules, Ollama/LM Studio/jan.ai, 6-month inactivity threshold — OUT of scope here).
- READ-ONLY, OFFLINE. Never write files, never write the registry, never modify PATH. No `--apply` / `--fix` mode.
- Existing `scripts/deps_audit/audit.py` is UNCHANGED and out of scope (it audits repo-declared deps vs repo source; this tool is machine-wide).
- Abandoned: the Rust/cargo "pivot" idea and portable-app detection (no reliable method).
- Stdlib only. `winreg` (Windows-only stdlib) is the registry access path — validated present on the target Python 3.13.
- Follow the `scripts/deps_audit/` convention: single-purpose scripts, `argparse` CLI with `--json`, unit tests via `python -m unittest`, a `README.md`, invocation as `python scripts/system_inventory/<x>.py`.

## Package shape (target)

```
scripts/system_inventory/
  __init__.py
  common.py        # InventoryItem dataclass, human_size(), safe dir_size_on_disk(), sort + report + JSON
  inventory.py     # orchestrator + CLI entry point (aggregates all scanners)
  registry.py      # Source: Uninstall registry (HKLM+HKCU, 32/64 views)
  appdata.py       # Source: %LOCALAPPDATA% / %APPDATA% first-level + %USERPROFILE% dotfolders + %ProgramData% first-level
  packages.py      # Source: Scoop + Chocolatey trees
  path_env.py      # Source: PATH entries (user+system), dead-entry flagging
  docker_wsl.py    # Source: Docker/WSL2 .vhdx files + registered WSL distros
  README.md
  tests/
    __init__.py
    test_common.py test_registry.py test_appdata.py
    test_packages.py test_path_env.py test_docker_wsl.py test_inventory.py
```

- Every scanner returns `list[InventoryItem]` with a stable `source` tag: `registry | appdata | dotfolder | programdata | scoop-choco | path | docker-wsl`.
- `inventory.py` concatenates all scanners, sorts by `size_bytes` descending (unknown/None size sorts last), prints per-item source + human size, prints grand total. `--json` mirrors the same data.

## Child Plans

| #   | Plan                                   | File                                        | Status  | Validated |
| --- | -------------------------------------- | ------------------------------------------- | ------- | --------- |
| 1   | Core + orchestrator + registry source  | `./2026_07_06-system-inventory-part-1.md`   | done    | [ ]       |
| 2   | Filesystem-size sources (AppData/dotfolders/ProgramData, Scoop/Choco) | `./2026_07_06-system-inventory-part-2.md`   | done    | [ ]       |
| 3   | PATH audit + Docker/WSL vhdx + polish   | `./2026_07_06-system-inventory-part-3.md`   | pending | [ ]       |

<!-- Status values: pending, in-progress, done, blocked -->
<!-- RULE: Part N+1 blocked until Part N checkbox checked. Each part is independently runnable/shippable. -->

## Why three parts (independence)

- **Part 1** establishes the shared model (`InventoryItem`), the safe directory-sizing helper, the report/JSON/sort/total machinery, the CLI, and the first real source (registry — the only source with a native, non-heuristic size). After Part 1 the tool runs end-to-end: `python scripts/system_inventory/inventory.py [--json]` prints a registry-only inventory sorted by size with a total. No later part is required.
- **Part 2** adds the filesystem-walk sources that reuse Part 1's `dir_size_on_disk()`: AppData first-level + dotfolders, a first-level `%ProgramData%` scan (excluding the `chocolatey` entry, which is itemized per-app by the Scoop/Choco source to avoid double-counting), and Scoop/Choco trees. Strictly additive — new scanners wired into the orchestrator behind `--source` filtering; nothing in Part 1 changes semantics.
- **Part 3** adds the two "special" sources (PATH entries with dead-folder flagging; Docker/WSL `.vhdx` + registered distros) plus final polish (`--top N`, README, cross-source total wording). Additive again; the registry+filesystem inventory from Parts 1-2 keeps working unchanged, except for one required adjustment: the orchestrator runs `docker_wsl` first and threads its claimed `.vhdx` absolute paths into `scan_appdata()`'s `exclude_paths` parameter (already plumbed by Part 1/2), so the same bytes are never counted twice regardless of whether they live under `Docker\wsl\` (Docker's own utility VM) or `Packages\<PackageFamilyName>\LocalState\` (a Microsoft-Store-installed WSL distro).

## Validation Protocol

1. Complete Part 1, run its acceptance commands.
2. [ ] Checkpoint 1: User confirms Part 1 (tool runs, registry source correct, tests green).
3. Unblock Part 2, complete it, run its acceptance commands.
4. [ ] Checkpoint 2: User confirms Part 2 (AppData/dotfolders + Scoop/Choco sizes plausible, tests green).
5. Unblock Part 3, complete it, run its acceptance commands.
6. [ ] Final: `python -m unittest discover -s scripts/system_inventory/tests -v` exits 0 AND `python scripts/system_inventory/inventory.py --json` emits valid JSON sorted by descending size with all seven sources represented, and no byte range double-counted across sources.

## Cross-cutting risk register

| Risk | Impact | Mitigation |
| ---- | ------ | ---------- |
| Recursive directory sizing hangs / loops on junctions & symlinks (`.cargo`, WSL mounts) | Wrong totals or infinite walk | `dir_size_on_disk()` skips reparse points (`FILE_ATTRIBUTE_REPARSE_POINT`) and never follows them; unit-tested |
| `PermissionError` / long paths on protected AppData subtrees | Crash mid-scan | Every filesystem walk swallows `OSError`/`PermissionError` per-entry and continues; partial sizes flagged, never fatal |
| Registry `EstimatedSize` absent or in KB, `InstallDate` absent | Misreported or missing sizes | Treat `EstimatedSize` (DWORD) as KB→bytes; tolerate missing values as `size=None` (sorts last); never fabricate a size |
| Accidental write to registry/PATH | Violates read-only contract | Only `winreg.OpenKey`/`QueryValueEx` (read APIs); no `SetValueEx`/`CreateKey` anywhere; asserted by code review + test naming |
| `.vhdx` reported size (sparse allocated vs logical) is ambiguous | Confusing numbers | Report the file's on-disk size via `os.stat().st_size`; document in README that vhdx is a sparse allocated size |
| Non-Windows execution | `winreg` ImportError | Tool is Windows-only by design; guard `winreg` import and exit with a clear message on other platforms |
| `%ProgramData%` first-level scan overlaps with Chocolatey's own `lib/*` itemization | Duplicate bytes inflate the grand total | `scan_programdata()` explicitly excludes the `chocolatey` entry; the Scoop/Choco source remains the sole owner of that subtree |
| A `docker-wsl` `.vhdx` (Docker's own utility VM under `Docker\wsl\`, or a Microsoft-Store WSL distro under `Packages\<PackageFamilyName>\LocalState\`) is nested inside a `LOCALAPPDATA` first-level folder the generic `appdata` source also sums | Multi-GB `.vhdx` counted twice in the grand total | Part 1's `dir_size_on_disk()` accepts `exclude_paths`; Part 2's `scan_appdata()` forwards it; Part 3's orchestrator runs `docker_wsl` first and threads its claimed vhdx paths in as `exclude_paths` — `docker_wsl.py` is the sole owner of `.vhdx` bytes; unit-tested |

## Estimations

- **Confidence**: 9/10
- **Duration**: ~1 to 1.5 days total across the three parts.
