# Deployment

## Deployment Process

DevToolBox is a standalone native binary, built with a single cross-platform `eframe`/
`egui` UI targeting Windows and Linux (`platform::`/`src/windows/`/`src/linux/` split,
see `architecture.md`). There is no server deployment; the app is built locally and run
on the end-user machine, for either OS.

- **Build**: `cargo build --release` (profile: `lto = true`, `opt-level = 3`)
  — on Windows, a rebuild fails with "Accès refusé" (exit code 1) if a
  previously-launched `devtoolbox.exe` is still running and holding the
  executable file locked; `taskkill //IM devtoolbox.exe //F` before rebuilding
  clears it.
- **Artifact**: single `devtoolbox.exe` (Windows) or `devtoolbox` ELF binary (Linux)
  under `target/release/`
- **Startup at login**:
  - Windows: HKCU Registry Run key (`platform::windows::RegistryStartupProvider`). The
    value is the quoted `current_exe()` of the process performing boot sync, so refresh
    it by launching the intended release artifact with `launch_at_startup` enabled;
    running a debug artifact would persist the debug path instead. Both Windows build
    profiles use the GUI subsystem and must remain console-free.
  - Linux: XDG autostart `.desktop` file under `$XDG_CONFIG_HOME/autostart/`
    (`platform::linux::LinuxStartupProvider`, wraps `crate::linux::autostart`)

## Prerequisites (target machine / build env)

### Windows
- Windows 11 (22H2+)
- Visual Studio 2022 with C++ build tools
- Rust toolchain (edition 2021)
- Windows SDK (10.0.22621.0+)

### Linux (Ubuntu/Debian - other distributions untested)
- Rust toolchain (edition 2021)
- System libraries required by the `eframe`/`winit` backend:
  `libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
  libxkbcommon-dev libssl-dev`
- `systemd --user` for the Automations view (unit enumeration/enable/disable); if
  `systemctl` cannot be invoked, `crate::linux::automations::fetch()` returns `Err`
  and the view surfaces that message instead of the app failing to start
- **Not yet validated on a real, disposable Ubuntu LTS VM** — see the Part 5 plan's
  Phase 3/4 amendments (`aidd_docs/tasks/2026_08/2026_08_05-multi-os-transformation-part-5.md`):
  the automated test suite has been run and verified regression-free on a Linux dev
  machine, but the manual dry-run-then-execute validation pass this section's
  "Validated on Linux" claim would require has not been performed in any session so
  far, for lack of an available VM.

## Performance targets

| Metric              | Target        |
| ------------------- | ------------- |
| Binary size         | < 20 MB       |
| Startup             | < 3 s         |
| UI rendering        | 60 FPS (GPU)  |
| Memory              | < 100 MB      |
| Command exec overhead | < 50 ms     |

> No CI/CD pipeline, container, or remote infrastructure at this stage — local build and run only.
# Distribution and release

Rust 1.93.0 and dependency lockfiles are release inputs. `cargo-packager` 0.11.8
builds two DMGs (macOS arm64/Intel), NSIS x64, deb x64 and AppImage x64. CI uses
explicit `windows-2025`, `ubuntu-22.04`, `macos-15` and `macos-15-intel` runners;
third-party actions are pinned by full commit SHA and default permissions are read-only.

Development builds may omit updater keys and show a disabled updater. Stable builds
set `DEVTOOLBOX_RELEASE_BUILD=1` and point `DEVTOOLBOX_UPDATE_PUBLIC_KEYS` at the
protected public JSON keyring; `build.rs` refuses the build otherwise. Signing and
publication run only in the protected `production-release` environment. Assets first
enter a draft; `latest.json` is generated and uploaded last, and explicit protected
approval is required to clear the draft flag. See `docs/release-readiness.md` and
`docs/updater-key-operations.md`.
