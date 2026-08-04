# Codebase Structure

Current state (MVP, Phase 1): a single binary crate with one entry point. The modular layout below is the intended target documented in `README.md`; only `src/main.rs` exists today.

```mermaid
flowchart TD
    MAIN["src/main.rs - entry point, event loop, window"]
    CFG["config/default.json - seed config (categories, commands, settings)"]
    CARGO["Cargo.toml - crate + deps"]

    MAIN -.reads.-> CFG
    CARGO --> MAIN

    subgraph Planned["Planned modules (not yet created)"]
        WINMOD["src/windows/ - registry, task_scheduler, process"]
        UIMOD["src/ui/ - app state, xaml_gen"]
        STOREMOD["src/storage/ - models, json"]
        ASSETS["src/assets/ - custom icons"]
    end

    MAIN -.will use.-> WINMOD
    MAIN -.will use.-> UIMOD
    MAIN -.will use.-> STOREMOD
```

## Standalone scripts (`scripts/`)

Independent, stdlib-only Python utilities living alongside the Rust crate, each with its own `tests/` run via `python -m unittest discover`: `sftp_fetch/`, `deps_audit/` (repo-declared deps vs source audit), `system_inventory/` (read-only Windows dev-machine disk inventory: registry, AppData/dotfolders/ProgramData, Scoop/Choco, PATH, Docker/WSL vhdx), `winclean/` (dry-run-first disk cleaner; imports `system_inventory` as its **read-only** discovery layer and must never modify it — see `memory/internal/decisions/winclean-separate-package.md`).

### Gotchas shared by these scripts

- An argparse `help=` string is `%`-formatted at render time: a literal
  `%APPDATA%` must be written `%%APPDATA%%`, otherwise `--help` raises
  `ValueError` for the **whole** parser. No `parse_args` test catches it —
  formatting only happens on render, so assert on `format_help()`.
- A `--out <file>` option writes UTF-8, but **redirected stdout follows the
  console code page** (cp1252 here). Reading a script's piped output therefore
  needs `encoding='cp1252'`, and a `--json` pipe is not UTF-8-safe.
