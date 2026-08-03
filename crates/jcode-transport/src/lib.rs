//! Local IPC transport, one API across platforms.
//!
//! A Unix socket on Unix, a named pipe on Windows, both exposed as `Listener`
//! and `Stream` with the same shape as `tokio::net::Unix*`. Callers write one
//! code path and get the platform's native mechanism.
//!
//! This lives in its own crate so the harness API bridge can use it without
//! depending on `jcode-base`. The bridge is deliberately free of the
//! application foundation, and the alternative was duplicating 450 lines of
//! named pipe handling into it.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
