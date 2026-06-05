# Deployment

## Deployment Process

WinFXStart is a standalone native Windows binary. There is no server deployment; the app is built locally and run on the end-user machine.

- **Build**: `cargo build --release` (profile: `lto = true`, `opt-level = 3`)
- **Artifact**: single `.exe` under `target/release/`
- **Startup at login**: via Windows Registry Run Keys (Task Scheduler is an alternative under consideration)

## Prerequisites (target machine / build env)

- Windows 11 (22H2+)
- Visual Studio 2022 with C++ build tools
- Rust toolchain (nightly recommended)
- Windows SDK (10.0.22621.0+)

## Performance targets

| Metric              | Target        |
| ------------------- | ------------- |
| Binary size         | < 20 MB       |
| Startup             | < 3 s         |
| UI rendering        | 60 FPS (GPU)  |
| Memory              | < 100 MB      |
| Command exec overhead | < 50 ms     |

> No CI/CD pipeline, container, or remote infrastructure at this stage — local build and run only.
