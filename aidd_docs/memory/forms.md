# Forms

DevToolBox is a native WinUI 3 desktop app — there are no web forms. The only form-like surface is the **command/alias editor** (Phase 2, planned), used to create and edit commands.

## State Management

- App state held in-process (planned `src/ui/app.rs`); edits persisted to JSON via `serde_json`.

## Validation

- Command string presence and basic validity checked before save/execution.

## Form Flow

```mermaid
flowchart LR
    EDIT[Inline alias editor] --> VALIDATE[Validate name + command + shortcut]
    VALIDATE --> SAVE[Persist to JSON - serde_json]
    SAVE --> REFRESH[Refresh favorites/category view]
```

> Planned for Phase 2 (Personnalisation); not yet implemented.
