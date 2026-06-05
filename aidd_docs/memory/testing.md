# Testing Guidelines

This document outlines the testing strategy for WinFXStart.

> Current state: no automated tests exist yet (MVP, Phase 1). The strategy below is the intended target.

## Tools and Frameworks

- Rust built-in test framework (`cargo test`, `#[test]` / `#[cfg(test)]`)

## Testing Strategy

- Types of tests planned:
  - **Unit Tests**: JSON load/save (serde models), command parsing, config defaults
  - **Integration Tests**: command executor (process spawn), Registry startup registration
  - **Performance Tests**: validate targets (startup < 3 s, exec overhead < 50 ms, memory < 100 MB)

## Test Execution Process

- Run all tests: `cargo test`
- Release build sanity: `cargo build --release`

> No CI integration yet; tests are run locally.
