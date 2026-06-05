# Database

WinFXStart has no database engine. Persistence is local JSON files handled with `serde` / `serde_json`. The seed/default file is `config/default.json`.

```json
@config/default.json
```

## Main entities and relationships

```mermaid
erDiagram
    SETTINGS ||--|| CONFIG : "part of"
    CONFIG ||--o{ CATEGORY : contains
    CONFIG ||--o{ COMMAND : contains
    CATEGORY ||--o{ COMMAND : groups

    SETTINGS {
        bool show_categories
        int icon_size
        string theme
        bool launch_at_startup
        bool show_descriptions
    }
    CATEGORY {
        string id
        string name
        string icon
    }
    COMMAND {
        string id
        string name
        string command
        string category
        string icon
        bool is_favorite
        string shortcut
    }
```

- A `Command` references a `Category` by `id` (the `category` field stores the category's `id` string).
- `Settings` are app-level (theme, icon size, startup, etc.).

## Schema stability (issue #6)

The schema is intentionally unchanged by issue #6. Key invariants:

- `"uncategorized"` is a **synthetic grouping bucket**, never a stored `Category`. Commands with an empty or unknown `category` id are grouped under it at runtime only.
- The `category` field of a `Command` is an empty string `""` when the command has no category or its category was removed. This is the on-disk representation of "Uncategorized".
- No new JSON field was added in issue #6; the issue #3 lossless round-trip tests are unaffected.

## Migrations

None. Config schema is versioned via the top-level `version` field in the JSON. Schema evolution is handled in-code on load.

## Seeding

`config/default.json` ships as the default configuration (sample categories and commands).
