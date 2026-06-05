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
