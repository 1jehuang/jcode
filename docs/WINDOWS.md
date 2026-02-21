# Windows Support Architecture

This document describes how jcode achieves cross-platform support for Linux, macOS, and Windows.

## Design Principle

**Zero cost on Unix.** The abstraction layer uses `#[cfg]` compile-time gates and type aliases so that Linux and macOS code paths compile to the exact same binary as before. Windows gets its own implementations behind `#[cfg(windows)]`. No traits, no dynamic dispatch, no runtime branching.

## Transport Layer (`src/transport/`)

The transport layer abstracts IPC (Inter-Process Communication). On Unix, jcode uses Unix domain sockets. On Windows, jcode uses named pipes.

### Module Structure

```
src/transport/
  mod.rs        - conditional re-exports (cfg-gated)
  unix.rs       - type aliases + helpers wrapping tokio Unix sockets
  windows.rs    - named pipe server/client using tokio
```

### Unix (Linux + macOS)

Unix transport is a thin re-export of existing types:

```rust
// transport/unix.rs
pub use tokio::net::UnixListener as Listener;
pub use tokio::net::UnixStream as Stream;
pub use tokio::net::unix::OwnedWriteHalf as WriteHalf;
pub use tokio::net::unix::OwnedReadHalf as ReadHalf;

// For synchronous IPC (used by communicate tool)
pub use std::os::unix::net::UnixStream as SyncStream;
```

The application code changes from `use tokio::net::UnixStream` to `use crate::transport::Stream` - same type, different import path.

### Windows

Windows transport uses named pipes via `tokio::net::windows::named_pipe`:

```rust
// transport/windows.rs
// Wraps NamedPipeServer/NamedPipeClient to match the Listener/Stream interface
```

Named pipe paths follow the convention:
- Main socket: `\\.\pipe\jcode-<name>`
- Debug socket: `\\.\pipe\jcode-<name>-debug`

### Address Abstraction

Socket paths on Unix vs pipe names on Windows:

```rust
// transport/mod.rs
#[cfg(unix)]
pub type Address = std::path::PathBuf;  // e.g. /run/user/1000/jcode.sock

#[cfg(windows)]
pub type Address = String;  // e.g. \\.\pipe\jcode-main
```

Conversion functions in `server.rs` and `registry.rs` produce the right address type per platform.

## Files Affected by Transport Migration

| File | What changes |
|------|-------------|
| `src/server.rs` | `UnixListener::bind` -> `transport::Listener::bind` |
| `src/tui/backend.rs` | `UnixStream::connect` -> `transport::Stream::connect`, `OwnedWriteHalf` -> `transport::WriteHalf` |
| `src/tui/client.rs` | `UnixStream` -> `transport::Stream` |
| `src/tui/app.rs` | `OwnedWriteHalf` -> `transport::WriteHalf` |
| `src/tool/communicate.rs` | `std::os::unix::net::UnixStream` -> `transport::SyncStream` |
| `src/tool/debug_socket.rs` | `tokio::net::UnixStream` -> `transport::Stream` |
| `src/main.rs` | `UnixStream::connect` -> `transport::Stream::connect` for health checks |

## Platform Module (`src/platform.rs`)

Non-IPC OS abstractions, all using `#[cfg]` gates:

### Process Replacement (`exec`)

Unix uses `CommandExt::exec()` to replace the current process. Windows spawns a child and exits.

```rust
pub fn replace_process(cmd: &mut std::process::Command) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        panic!("exec failed: {err}");
    }
    #[cfg(windows)]
    {
        let status = cmd.status().expect("failed to spawn process");
        std::process::exit(status.code().unwrap_or(1));
    }
}
```

### Signal Handling

Unix: `tokio::signal::unix` for SIGHUP, SIGTERM, SIGINT, SIGQUIT.
Windows: `tokio::signal::ctrl_c()` + `SetConsoleCtrlHandler` for close events.

### Symlinks

Unix: `std::os::unix::fs::symlink()`.
Windows: `std::fs::copy()` (symlinks require elevated privileges). Junction points for directories if needed.

### File Permissions

Unix: `PermissionsExt::set_mode(0o600)` for sensitive files.
Windows: No-op (or Windows DACL APIs for production hardening later).

### Process Liveness

Unix: `libc::kill(pid, 0)` to check if alive.
Windows: `OpenProcess` + `GetExitCodeProcess` via `windows-sys` crate.

## Dependencies

New dependencies for Windows support:

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = ["Win32_System_Threading", "Win32_Foundation"] }
```

The `tokio` dependency already includes named pipe support on Windows (part of `features = ["full"]`).

## What Doesn't Change

The vast majority of the codebase is platform-agnostic and requires no changes:

- All provider code (HTTP-based)
- All tool implementations (except bash tool's shell selection)
- TUI rendering (crossterm + ratatui are already cross-platform)
- Agent logic, memory, sessions, config
- MCP client/server protocol
- JSON serialization, protocol handling

## Shell Tool Considerations

The `bash` tool currently executes commands via `/bin/bash`. On Windows:
- Default to `cmd.exe` or `pwsh.exe` (PowerShell)
- Common Unix commands (`grep`, `cat`, `ls`) don't exist natively
- Path separators differ (`\` vs `/`)

This is handled at the tool level, not the transport level. The tool can detect the platform and choose the appropriate shell.

## Build & CI

Cross-compilation from Linux:
```bash
# Install Windows target
rustup target add x86_64-pc-windows-msvc

# Cross-compile (requires cross or cargo-xwin)
cargo xwin build --release --target x86_64-pc-windows-msvc
```

CI should run tests on both Linux and Windows runners to catch platform-specific regressions.

## Migration Strategy

1. **Phase 1: Transport abstraction** - Create `src/transport/`, migrate all IPC code. Unix behavior unchanged.
2. **Phase 2: Platform module** - Create `src/platform.rs` for exec, signals, symlinks, permissions.
3. **Phase 3: Windows implementation** - Implement named pipe transport + platform functions.
4. **Phase 4: CI & testing** - Add Windows CI, test IPC roundtrip, basic flows.
