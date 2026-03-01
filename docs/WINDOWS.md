# Windows Support Architecture

This document describes how jcode achieves cross-platform support for Linux, macOS, and Windows.

## Status

- **Transport layer**: Implemented (`src/transport/`)
- **Platform module**: Implemented (`src/platform.rs`)
- **Windows transport**: Implemented but untested (`src/transport/windows.rs`)
- **Windows platform**: Implemented (`src/platform.rs` has `#[cfg(windows)]` branches)
- **Windows CI**: Not yet set up

## Design Principle

**Zero cost on Unix.** The abstraction layer uses `#[cfg]` compile-time gates and type aliases so that Linux and macOS code paths compile to the exact same binary as before. Windows gets its own implementations behind `#[cfg(windows)]`. No traits, no dynamic dispatch, no runtime branching.

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

1. **Windows CI** - Add GitHub Actions Windows runner, test compilation and basic IPC
2. **Shell tool** - Detect platform and use `cmd.exe` or `pwsh.exe` on Windows
3. **Self-update** - Handle Windows exe replacement (can't overwrite running binary)
4. **Testing** - Run full test suite on Windows
