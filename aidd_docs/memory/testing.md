# Testing Guidelines

This document outlines the testing strategy for WinFXStart.

> Current state: automated Rust unit tests cover the application modules. The standalone
> SFTP script has its own isolated Python unit-test suite.

## Tools and Frameworks

- Rust built-in test framework (`cargo test`, `#[test]` / `#[cfg(test)]`)
- Python built-in test framework (`unittest`) for standalone scripts

## Testing Strategy

- Types of tests planned:
  - **Unit Tests**: JSON load/save (serde models), command parsing, config defaults
  - **Integration Tests**: command executor (process spawn), Registry startup registration
  - **Performance Tests**: validate targets (startup < 3 s, exec overhead < 50 ms, memory < 100 MB)

## Test Execution Process

- Run all tests: `cargo test`
- Run SFTP script tests: `cd scripts/sftp_fetch && python -m unittest discover -s tests -v`
- Run deps-audit script tests: `python -m unittest discover -s scripts/deps_audit/tests -v`
- Run system-inventory script tests: `python -m unittest discover -s scripts/system_inventory/tests -v`
- Run winclean script tests: `python -m unittest discover -s scripts/winclean/tests -t .` — the `-t .` is required: the package bootstraps its imports from the repo root, so discovery from elsewhere fails to import `scripts.winclean`
- Release build sanity: `cargo build --release`

> No CI integration yet; tests are run locally.
