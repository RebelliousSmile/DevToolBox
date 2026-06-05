# API Documentation

WinFXStart exposes **no network/HTTP API**. It is a standalone desktop app. Its "external interface" is the set of Windows OS APIs it calls and the CLI commands it spawns.

## Windows API surface (consumed, not exposed)

Via the `windows` crate (see `Cargo.toml`):

- **Process / Threading** (`Win32_System_Threading`, `Win32_System_ProcessStatus`): spawn and track launched CLI commands
- **Registry** (`Win32_System_Registry`): Run Keys for launch-at-startup
- **Shell** (`Win32_UI_Shell`): icon / shell integration
- **WindowsAndMessaging / GDI / Foundation**: native window and rendering

## Command execution contract

- A `Command` entry's `command` field (e.g. `notepad.exe`, `cmd.exe /c`, `ipconfig /all`) is passed to the Windows process API.
- Result surfaced to the user as visual success/error feedback (planned).
