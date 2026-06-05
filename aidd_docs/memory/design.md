# DESIGN.md

## Design Implementation

- **UI Framework**: WinUI 3 (Microsoft UI Library) — native Windows 11 look & feel, no WebView
- **Styling Method**: native XAML controls; dynamic XAML generation planned (`src/ui/xaml_gen.rs`)

## Design System

- **Theme**: light / dark (from `Settings.theme`, default `light`)
- **Icon size**: configurable (`Settings.icon_size`, default `80`)
- **Iconography**: emoji and custom PNG/SVG icons per command/category

## UI Patterns

- **Favorites grid**: visual grid of favorite commands (primary surface)
- **Categories**: optional grouping (System / Network / Maintenance seeded in `config/default.json`), toggled by `Settings.show_categories`
- **Descriptions**: optional, toggled by `Settings.show_descriptions`
- **Search**: command search bar (planned)
- **Feedback**: visual success/error and execution feedback (planned)

## Accessibility

- **Keyboard navigation**: customizable per-command shortcuts (e.g. `Ctrl+N`)

> Detailed design tokens / component specs not yet defined — UI is in Phase 1 (MVP).
