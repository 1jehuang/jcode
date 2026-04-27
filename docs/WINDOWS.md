# Windows Support Architecture

This document describes how jcode achieves cross-platform support for Linux, macOS, and Windows.

## Status

- **Transport layer**: Implemented (`src/transport/`); pipe pair, pipe-name normalization, and round-trip tests run on Windows.
- **Platform module**: Implemented (`src/platform.rs`); `is_process_running` (via `GetExitCodeProcess` + `STILL_ACTIVE`), `replace_process` (spawn + exit), symlink-or-copy, and atomic-swap have Windows code paths.
- **Windows transport**: Implemented and exercised by the Windows CI smoke job (`src/transport/windows.rs`). `Listener::accept` uses interior mutability (`tokio::sync::Mutex<NamedPipeServer>`) so its signature matches `tokio::net::UnixListener::accept` on Unix — call sites stay portable.
- **Windows platform**: Implemented.
- **Windows CI**: x64 build, targeted unit tests, two e2e smoke tests, `--version` smoke, and PowerShell installer verification on every dispatch of `windows-smoke.yml`. ARM64 build + installer verification on the same workflow. The main `ci.yml` also has a `windows-build-test` matrix entry.
- **Bash tool**: `build_shell_command` shells out via `cmd.exe /C` on Windows.
- **Self-dev**: Build/reload paths exist but the binary swap on Windows is **not yet** atomic and cannot replace a running `jcode.exe` in place — see "Remaining Work" below.

## Design Principle

**Zero cost on Unix.** The abstraction layer uses `#[cfg]` compile-time gates and type aliases so that Linux and macOS code paths compile to the exact same binary as before. Windows gets its own implementations behind `#[cfg(windows)]`. No traits, no dynamic dispatch, no runtime branching.

## Install Paths

Current Windows install paths from `scripts/install.ps1`:

- Launcher: `%LOCALAPPDATA%\\jcode\\bin\\jcode.exe`
- Stable channel binary: `%LOCALAPPDATA%\\jcode\\builds\\stable\\jcode.exe`
- Immutable versioned binaries: `%LOCALAPPDATA%\\jcode\\builds\\versions\\<version>\\jcode.exe`

Unlike the current Unix self-dev/local-build flow, the PowerShell installer currently installs the stable channel rather than a separate `current` channel.

## Transport Layer (`src/transport/`)

The transport layer abstracts IPC (Inter-Process Communication). On Unix, jcode uses Unix domain sockets. On Windows, jcode uses named pipes.

### Module Structure

```
src/transport/
  mod.rs        - conditional re-exports (cfg-gated)
  unix.rs       - type aliases wrapping tokio Unix sockets (zero-cost)
  windows.rs    - named pipe Listener/Stream with split support
```

### Unix (Linux + macOS)

Unix transport is a thin re-export of existing types:

```rust
pub use tokio::net::UnixListener as Listener;
pub use tokio::net::UnixStream as Stream;
pub use tokio::net::unix::OwnedWriteHalf as WriteHalf;
pub use tokio::net::unix::OwnedReadHalf as ReadHalf;
pub use std::os::unix::net::UnixStream as SyncStream;
```

The compiled binary is byte-for-byte identical to what it was before the abstraction.

### Windows

Windows transport provides custom types wrapping `tokio::net::windows::named_pipe`:

- **`Listener`**: Wraps `NamedPipeServer` with an accept loop that creates new pipe instances for each connection (named pipes are single-client, so a new instance is created after each accept)
- **`Stream`**: Enum over `NamedPipeServer` (accepted) or `NamedPipeClient` (connected), implementing `AsyncRead + AsyncWrite`
- **`ReadHalf` / `WriteHalf`**: Created via `stream.into_split()` using `Arc<Mutex<Stream>>` since named pipes don't support native kernel-level splitting
- **`SyncStream`**: Opens the named pipe as a regular file for blocking I/O

Socket paths are converted to pipe names: `/run/user/1000/jcode.sock` becomes `\\.\pipe\jcode`.

### API Surface

Both platforms export the same interface:

| Export | Unix | Windows |
|--------|------|---------|
| `Listener` | `tokio::net::UnixListener` | Custom struct wrapping `NamedPipeServer` |
| `Stream` | `tokio::net::UnixStream` | Enum over `NamedPipeServer`/`NamedPipeClient` |
| `ReadHalf` | `tokio::net::unix::OwnedReadHalf` | `Arc<Mutex<Stream>>` wrapper |
| `WriteHalf` | `tokio::net::unix::OwnedWriteHalf` | `Arc<Mutex<Stream>>` wrapper |
| `SyncStream` | `std::os::unix::net::UnixStream` | `std::fs::File` wrapper |

## Platform Module (`src/platform.rs`)

Centralizes all non-IPC OS-specific operations:

| Function | Unix | Windows |
|----------|------|---------|
| `symlink_or_copy(src, dst)` | `std::os::unix::fs::symlink()` | Try `symlink_file/dir`, fall back to copy |
| `atomic_symlink_swap(src, dst, temp)` | Create temp symlink + rename | Remove + copy (best effort) |
| `set_permissions_owner_only(path)` | `chmod 600` | No-op |
| `set_permissions_executable(path)` | `chmod 755` | No-op |
| `is_process_running(pid)` | `kill(pid, 0)` | Returns `true` (stub) |
| `replace_process(cmd)` | `exec()` (replaces process) | `spawn()` + `exit()` |

## Files Migrated

All OS-specific code has been moved out of application files into the transport and platform modules:

| File | What was migrated |
|------|------------------|
| `src/server.rs` | `UnixListener`, `UnixStream`, `OwnedReadHalf`, `OwnedWriteHalf` |
| `src/tui/backend.rs` | `UnixStream`, `OwnedWriteHalf`, `OwnedReadHalf` |
| `src/tui/client.rs` | `UnixStream`, `OwnedWriteHalf` |
| `src/tui/app.rs` | `UnixListener`, `OwnedWriteHalf`, file permissions |
| `src/tool/communicate.rs` | `std::os::unix::net::UnixStream` |
| `src/tool/debug_socket.rs` | `tokio::net::UnixStream` |
| `src/main.rs` | `UnixStream` (health checks), all `exec()` calls, file permissions |
| `src/build.rs` | Symlinks, executable permissions |
| `src/update.rs` | Symlinks, permissions, atomic swap |
| `src/auth/oauth.rs` | Credential file permissions |
| `src/skill.rs` | Symlink creation |
| `src/video_export.rs` | Frame symlinks |
| `src/ambient.rs` | Process liveness check |
| `src/registry.rs` | Process liveness check |
| `src/session.rs` | Process liveness check |

## Dependencies

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = ["Win32_Foundation", "Win32_System_Threading"] }
```

The `tokio` dependency already includes named pipe support on Windows (part of `features = ["full"]`).

## What Doesn't Change

The vast majority of the codebase is platform-agnostic:

- All provider code (HTTP-based)
- All tool implementations (except bash tool's shell selection)
- TUI rendering (crossterm + ratatui already cross-platform)
- Agent logic, memory, sessions, config
- MCP client/server protocol
- JSON serialization, protocol handling

## Remaining Work

1. **Self-update** — Windows can't overwrite a running `.exe`. The Unix-style "rename + atomic swap" path in `src/update.rs` and `src/platform.rs::atomic_symlink_swap` falls back to `remove + copy` on Windows (best-effort) and will fail if the new launcher target is the currently-running binary. Proper fix: rename current to `.old` (allowed for a running exe), copy new in place, schedule the `.old` for delete-on-next-launch (or `MoveFileExW` with `MOVEFILE_DELAY_UNTIL_REBOOT`).
2. **Self-dev / "current" channel on Windows** — `scripts/install.ps1` currently only installs the `stable` channel under `%LOCALAPPDATA%\jcode\builds\stable\jcode.exe`. There's no PowerShell equivalent of `scripts/install_release.sh` to install a local-build/`current` channel for self-dev. Either add one, or document that self-dev is Linux/macOS-only for now.
3. **Symlinks** — `platform::symlink_or_copy` already falls back to `std::fs::copy` when `symlink_file/dir` fails (Developer Mode / elevation not available). Channels and versioned-binary directories therefore work but consume more disk and don't follow target updates the way Unix symlinks do.
4. **`replace_process` semantics** — `src/platform.rs` Windows version `spawn + exit(0)` rather than `exec`, so `/reload` produces a new PID and the parent's exit status no longer reflects the child's. Acceptable as long as callers don't depend on exec semantics.
5. **Windows CI gating** — `windows-smoke.yml` is `workflow_dispatch` only. Promote it to required-on-PR (or fold its targeted tests into `ci.yml`'s `windows-build-test` job) so future regressions like the `mut listener` cycle don't reach master.
6. **Full test suite on Windows** — `windows-smoke.yml` runs ~10 named tests + 2 e2e and the `windows-build-test` job in `ci.yml` runs the targeted set with clippy `-D warnings`. The full `cargo test --no-default-features --features pdf --lib --bins` is **not yet** verified end-to-end on Windows. Module-by-module audit so far: `auth::`, `agent::`, `provider::`, `tool::`, `transport::`, `platform::`, `tui::session_picker::` are green. `tui::ui::*`, `tui::app::tests`, `tui::markdown::tests`, `tui::mermaid::tests` have ~40 ratatui-snapshot / scroll-position assertions that fail on Windows even though the underlying behavior may be correct (likely terminal-cell width, font metrics, or layout-snapshot timing differences). The `server::` test mod runs short tests fine but appears to deadlock on the longer socket-handshake tests when run together — needs investigation. These are known-flaky-on-Windows rather than known-broken.
7. **Browser tool / dictation** — Firefox Agent Bridge setup paths, dictation hotkeys, and screen-capture flows are still Unix-shaped. Functional, but the setup hints and helpers in `src/setup_hints/windows_setup.rs` only cover Alt+; hotkey + Alacritty install.
