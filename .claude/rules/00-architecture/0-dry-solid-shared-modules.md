---
name: dry-solid-shared-modules
description: Apply when a second caller needs logic that already exists - extract it into a shared module instead of copying or cloning it, keep caller-varying values as parameters, and migrate the original caller in the same change.
paths:
  - src/**/*.rs
---

# DRY / SOLID shared modules

## Extraction over duplication

- Second caller triggers extraction
- Never copy-paste shared logic
- No parameterized twin implementation
- One definition per repository
- Shared UI helpers live in `src/ui/`

## Parameters, not internal constants

- Caller-varying values become parameters
- No branch on the calling view
- Module never knows its callers

## Module boundaries

- Depend on the base crate only
- No domain types, no application state
- Callers depend on the abstraction, never the reverse

## Migrate in the same change

- Original caller migrates immediately
- Coexisting implementations are a regression
- Tests move with the extracted code
