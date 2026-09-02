# Testing Guidelines

This document outlines the testing strategy for DevToolBox.

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
- Run application-recommendation tests: `python3 -m unittest discover -s scripts/app_recommendations/tests -p 'test_*.py'`
- Run winclean script tests: `python -m unittest discover -s scripts/winclean/tests -t .` — the `-t .` is required: the package bootstraps its imports from the repo root, so discovery from elsewhere fails to import `scripts.winclean`
- Run local-model orchestrator tests: `python3 -m unittest discover -s scripts/model_orchestrator/tests`
- Release build sanity: `cargo build --release`
- Windows GUI-subsystem gate: after `cargo build` and `cargo build --release`, inspect
  both PE files directly. Read `e_lfanew` at offset `0x3c`, then the 16-bit `Subsystem`
  field at `e_lfanew + 0x5c`; `target/debug/devtoolbox.exe` and
  `target/release/devtoolbox.exe` must both equal `2` (`IMAGE_SUBSYSTEM_WINDOWS_GUI`),
  not `3` (`IMAGE_SUBSYSTEM_WINDOWS_CUI`). This PowerShell-compatible check avoids a
  dependency on `dumpbin`, `llvm-readobj`, or `objdump`.

## Local-model delivery matrix

- Linux/local CI: the Python suite, Python-generated schema/event fixtures parsed
  by Rust, `cargo fmt --check`, `cargo check`, `cargo clippy -- -D warnings`,
  `cargo test`, and `cargo build --release`.
- Pure egui tests cover the empty/partial catalog, filters, verified cache
  recommendation, manual priority, busy/interrupted/guided/protected/stale
  explanations, and the permanent Models navigation entry without touching a
  provider or model.
- Windows manual gate: `%LOCALAPPDATA%` defaults, custom volumes, junction/reparse
  refusal, Ollama/HF/LM native CLIs, Jan/LM settings discovery, hidden-console
  UTF-8 pipes, and cooperative cancellation that leaves no provider descendant.
- Distribution gate: ship `scripts/model_orchestrator` and `scripts/local_ai`
  beside the binary resources, or set `DEVTOOLBOX_HOME` to their parent root.

## Application report matrix

- Linux executable checks: Python report suite, affected `system_inventory` tests,
  `cargo fmt --check`, `cargo check`, `cargo clippy -- -D warnings`, `cargo test`,
  and a real read-only report opened in the egui view.
- Cross-platform fixtures: APT/Snap/Flatpak and Registry/AppX/Scoop/Chocolatey
  parsers run on Linux; the Python-generated schema-v1 fixture is deserialized by
  Rust to prevent producer/consumer drift.
- Windows delivery gate (cannot be inferred from Linux): validate registry 32/64
  views, MSIX protections, managers present/absent, `%LOCALAPPDATA%` history,
  process matching, partial-source display and clipboard copy without executing it.
- Distribution gate: ship `scripts/app_recommendations` and
  `scripts/system_inventory` beside the binary resources, or set
  `DEVTOOLBOX_HOME` to that root. A missing module must render an unavailable state.

## egui_kittest gotchas

- A spinner (busy state) requests continuous repaint, so `Harness::run()`
  panics ("still requesting repaints") — use `harness.run_steps(2)` instead
  when the UI under test can be busy.
- The **opposite** trap, and the one that bites first: a `Modal` centres
  itself only once it knows its size, so a click after `run_steps(n)` lands
  on the backdrop and reads as a dismissal — a dialog needs `run()`. Rule of
  thumb: `run()` for anything whose rect is computed over several frames
  (modals), `run_steps` for anything that never settles (spinners). An
  anchored `egui::Panel` is safe either way: its rect is final on frame 1.
- Only one tab's list is laid out (`docker_view::DockerList`), so a test
  whose subject is an image or a volume row must open that tab first —
  `State::with_snapshot(..).on_list(DockerList::Images)`. Querying a hidden
  list returns zero widgets, which looks like a rendering bug and is not one.
- Test state structs get new fields often (`selection`, `batch_report`,
  `active_list` each broke every literal at once with `E0063`). Build them
  through a constructor plus small `self`-returning setters, never by
  copy-pasting a literal into each test.
- One-frame-deferred actions (see `DeferredDockerAction` in
  `src/ui/egui_app.rs`) need an extra `harness.step()` before asserting their
  effect; tests gate real side effects behind a `docker_actions_enabled`
  flag (false in tests) and assert on an invocation counter.

> No CI integration yet; tests are run locally.
