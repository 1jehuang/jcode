//! Harness API bridge: exposes the stable versioned harness API on its own
//! Unix socket and translates to the internal (legacy) jcode protocol.
//!
//! Architecture (milestone 2 of docs/HARNESS_API_AND_DESKTOP_REWRITE.md):
//! - Listens on `~/.jcode/jcode-api.sock` (or `JCODE_API_SOCKET`).
//! - For each API client, dials the legacy daemon socket (`JCODE_SOCKET` or
//!   `~/.jcode/jcode.sock`) and speaks `subscribe`/`message`/... on its
//!   behalf.
//! - Translation is JSON-to-JSON so this crate does not depend on the heavy
//!   internal protocol types and cannot be broken by additive internal
//!   changes.
//!
//! This keeps the daemon untouched while the API surface stabilizes. Once
//! proven, the same translation can move in-process behind a `hello` sniff on
//! the main socket.

pub mod background_progress;
pub mod translate;

use anyhow::{Context, Result};
use jcode_harness_api::{API_VERSION_MAJOR, ApiEvent, ErrorCode, ServerFrame};
use serde_json::Value;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
// Unix sockets on Unix, named pipes on Windows, one API. Without this the
// bridge simply did not compile for Windows, so the SDK could not run there at
// all.
use jcode_transport::{Listener, Stream};

// Socket paths live in `jcode-harness-api` so clients and the bridge can never
// resolve different directories (they once did, and the desktop app could not
// connect as a result).
pub use jcode_harness_api::{api_socket_path, legacy_socket_path};

/// Largest single request frame accepted from an API client, in bytes.
///
/// `read_line` grows its buffer until it finds a newline, so a client that
/// never sends one makes the bridge allocate without bound: one connection can
/// exhaust the host's memory, and the bridge serves every client on the
/// machine. 16 MiB is far above any legitimate frame (the largest real one is a
/// message carrying base64 images) and far below a problem.
const MAX_FRAME_BYTES: u64 = 16 * 1024 * 1024;

/// Read one newline-delimited frame, refusing to buffer more than
/// `MAX_FRAME_BYTES`. Returns `Ok(0)` at end of stream, like `read_line`.
async fn read_frame<R>(reader: &mut R, line: &mut String) -> std::io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    line.clear();
    let mut limited = tokio::io::AsyncReadExt::take(reader, MAX_FRAME_BYTES);
    let read = limited.read_line(line).await?;
    // A full buffer with no terminator means the frame exceeded the cap (or is
    // exactly at it and unterminated); either way it cannot be trusted.
    if read as u64 == MAX_FRAME_BYTES && !line.ends_with('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame exceeds {MAX_FRAME_BYTES} byte limit"),
        ));
    }
    Ok(read)
}

/// Cancellation-safe variant for the multiplexed client loop. Tokio guarantees
/// `read_until` retains bytes already appended to `frame` when another select
/// branch wins, unlike `read_line` with a temporary String.
async fn read_frame_bytes<R>(reader: &mut R, frame: &mut Vec<u8>) -> std::io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut limited = tokio::io::AsyncReadExt::take(reader, MAX_FRAME_BYTES);
    let read = limited.read_until(b'\n', frame).await?;
    if frame.len() as u64 == MAX_FRAME_BYTES && !frame.ends_with(b"\n") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame exceeds {MAX_FRAME_BYTES} byte limit"),
        ));
    }
    Ok(read)
}

/// Run the bridge accept loop forever.
#[cfg(unix)]
pub(crate) struct InstanceLock {
    _file: std::fs::File,
    path: PathBuf,
}

#[cfg(unix)]
impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Best effort: the flock is released by the fd close regardless, and a
        // leftover empty lock file is harmless (the next bridge re-locks it).
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Take the exclusive bridge lock beside the API socket, or report that a live
/// bridge already holds it. `flock` is released by the kernel when the holder
/// dies, so a crashed bridge never wedges the next one out.
#[cfg(unix)]
pub(crate) fn single_instance_lock(api_socket: &std::path::Path) -> Result<Option<InstanceLock>> {
    use std::os::fd::AsRawFd;

    let path = api_socket.with_extension("lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open bridge lock {}", path.display()))?;
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    Ok(taken.then_some(InstanceLock { _file: file, path }))
}

pub async fn run_bridge(api_socket: PathBuf, legacy_socket: PathBuf) -> Result<()> {
    // Only one bridge may own the socket.
    //
    // This is the fix for clients seeing "disconnected: harness API stream
    // closed" at random: every desktop client spawned a bridge on demand, and
    // each new bridge unlinked the live socket and bound its own. The older
    // bridges kept running with their connected clients, but the *pathname*
    // now pointed at the newest one, and each reconnect churned the same way.
    // Whoever lost the race had its clients dropped. Refusing to start when a
    // live bridge holds the lock makes on-demand spawning idempotent, which is
    // what every caller already assumes.
    #[cfg(unix)]
    let _lock = match single_instance_lock(&api_socket)? {
        Some(lock) => lock,
        None => {
            eprintln!(
                "harness API bridge: another bridge already owns {}; exiting",
                api_socket.display()
            );
            return Ok(());
        }
    };
    // A stale socket file blocks bind on Unix. On Windows there is no file to
    // remove: the pipe namespace is not the filesystem. Safe to unlink here
    // only because we hold the exclusive lock above, so no live bridge owns it.
    #[cfg(unix)]
    let _ = std::fs::remove_file(&api_socket);
    if let Some(parent) = api_socket.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    // `mut` only on Windows: the named-pipe listener republishes a pipe
    // instance on every accept, so accepting takes `&mut self`. Unix's
    // UnixListener::accept takes `&self`, and an unconditional `mut` there is
    // an unused_mut warning, so the binding is declared per platform rather
    // than warning on every build.
    #[cfg(windows)]
    let mut listener = Listener::bind(&api_socket)
        .with_context(|| format!("bind API socket {}", api_socket.display()))?;
    #[cfg(unix)]
    let listener = Listener::bind(&api_socket)
        .with_context(|| format!("bind API socket {}", api_socket.display()))?;
    // Restrict the socket to its owner, matching the daemon socket it fronts.
    //
    // Without this the bridge widens access to everything behind it: the
    // daemon socket is 0600, but a default-umask bind here produced 0755, so
    // any local user could drive sessions, read transcripts, and spend the
    // owner's provider tokens. A bridge must never be more permissive than
    // the thing it bridges to.
    //
    // Unix only: a Windows named pipe carries an ACL rather than a file mode,
    // and the transport applies it when publishing the pipe.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&api_socket, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restrict API socket {}", api_socket.display()))?;
    }
    eprintln!(
        "harness API bridge: listening on {} -> {}",
        api_socket.display(),
        legacy_socket.display()
    );
    loop {
        let (stream, _) = listener.accept().await?;
        let legacy = legacy_socket.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_api_client(stream, legacy).await {
                eprintln!("harness API bridge: client ended: {error:#}");
            }
        });
    }
}

async fn handle_api_client(stream: Stream, legacy_socket: PathBuf) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    // 1. Handshake: first frame must be hello with a compatible version.
    read_frame(&mut reader, &mut line).await?;
    // A malformed first frame used to abort the task, closing the connection
    // with no reply at all: the client saw only an EOF and could not tell a
    // protocol mistake from a crashed bridge. Say what was wrong, then close.
    let hello: Value = match serde_json::from_str(line.trim()) {
        Ok(value) => value,
        Err(error) => {
            let frame = ServerFrame::event(ApiEvent::Error {
                code: ErrorCode::InvalidRequest,
                message: format!("first frame must be a JSON `hello`: {error}"),
            });
            write_json_line(&mut write_half, &frame).await?;
            return Ok(());
        }
    };
    let reply_to = hello["id"].as_u64().unwrap_or(0);
    let compatible = hello["req"] == "hello"
        && hello["min_version"].as_u64().unwrap_or(0) <= u64::from(API_VERSION_MAJOR)
        && hello["max_version"].as_u64().unwrap_or(0) >= u64::from(API_VERSION_MAJOR);
    if !compatible {
        let frame = ServerFrame::reply(
            reply_to,
            ApiEvent::Error {
                code: ErrorCode::UnsupportedVersion,
                message: format!(
                    "bridge speaks API v{API_VERSION_MAJOR}; this client asked for v{}..=v{}",
                    hello["min_version"].as_u64().unwrap_or(0),
                    hello["max_version"].as_u64().unwrap_or(0),
                ),
            },
        );
        write_json_line(&mut write_half, &frame).await?;
        return Ok(());
    }
    let hello_ok = ServerFrame::reply(
        reply_to,
        ApiEvent::HelloOk {
            version: API_VERSION_MAJOR,
            server: format!("jcode-harness-api-bridge/{}", env!("CARGO_PKG_VERSION")),
            capabilities: [
                "sessions",
                "streaming",
                "persisted_session_discovery",
                "runtime_info",
                "api_key_provisioning",
                "session_archive",
                "session_retention",
                "session_files",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        },
    );
    write_json_line(&mut write_half, &hello_ok).await?;

    // 2. Dial the legacy daemon for this client.
    let legacy = Stream::connect(&legacy_socket)
        .await
        .with_context(|| format!("connect legacy socket {}", legacy_socket.display()))?;
    let (legacy_read, mut legacy_write) = legacy.into_split();
    let mut legacy_reader = BufReader::new(legacy_read);

    let mut state = translate::BridgeState::default();

    // 3. Pump both directions in one select loop so translation state stays
    //    single-threaded.
    let mut api_frame = Vec::new();
    let mut legacy_line = String::new();
    loop {
        tokio::select! {
            // A busy daemon can keep the legacy socket continuously readable.
            // Prefer client requests so locally answered operations such as
            // list_sessions cannot sit behind an unbounded event broadcast.
            biased;
            n = read_frame_bytes(&mut reader, &mut api_frame) => {
                let n = match n {
                    Ok(n) => n,
                    // An oversized frame is unrecoverable: the stream is now
                    // mid-frame with no way to resynchronise. Report and close.
                    Err(error) => {
                        let frame = ServerFrame::event(ApiEvent::Error {
                            code: ErrorCode::InvalidRequest,
                            message: error.to_string(),
                        });
                        write_json_line(&mut write_half, &frame).await?;
                        return Ok(());
                    }
                };
                if n == 0 { return Ok(()); }
                if api_frame.iter().all(u8::is_ascii_whitespace) {
                    api_frame.clear();
                    continue;
                }
                let parsed = serde_json::from_slice(&api_frame);
                api_frame.clear();
                let request: Value = match parsed {
                    Ok(value) => value,
                    Err(error) => {
                        // No `reply_to`: the id lived in the frame that failed
                        // to parse, so there is nothing to correlate against.
                        let frame = ServerFrame::event(ApiEvent::Error {
                            code: ErrorCode::InvalidRequest,
                            message: error.to_string(),
                        });
                        write_json_line(&mut write_half, &frame).await?;
                        continue;
                    }
                };
                // Translation may inspect persisted session/archive files. Tell
                // Tokio before entering that synchronous region so it can keep
                // the accept loop and fresh-client handshakes scheduled.
                let outbound = tokio::task::block_in_place(|| {
                    state.api_request_to_legacy(&request)
                });
                for out in outbound {
                    match out {
                        translate::Outbound::Legacy(value) => {
                            write_json_line(&mut legacy_write, &value).await?;
                        }
                        translate::Outbound::Reply(frame) => {
                            write_json_line(&mut write_half, &frame).await?;
                        }
                    }
                }
            }
            n = legacy_reader.read_line({ legacy_line.clear(); &mut legacy_line }) => {
                if n? == 0 {
                    let frame = ServerFrame::event(ApiEvent::Error {
                        code: ErrorCode::Internal,
                        message: "daemon connection closed".into(),
                    });
                    write_json_line(&mut write_half, &frame).await?;
                    return Ok(());
                }
                if legacy_line.trim().is_empty() { continue; }
                let event: Value = match serde_json::from_str(legacy_line.trim()) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let frames = tokio::task::block_in_place(|| {
                    state.legacy_event_to_api(&event)
                });
                for frame in frames {
                    write_json_line(&mut write_half, &frame).await?;
                }
            }
        }
    }
}

async fn write_json_line<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: ?Sized + serde::Serialize,
{
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
#[path = "framing_tests.rs"]
mod framing_tests;

#[cfg(all(test, unix))]
mod single_instance_tests {
    /// Two bridges must never both own the API socket.
    ///
    /// They used to: `run_bridge` unlinked whatever socket file was there and
    /// bound its own, so every on-demand spawn silently evicted the live
    /// bridge and its clients reported "harness API stream closed".
    #[test]
    fn a_second_bridge_cannot_take_the_socket() {
        let dir = std::env::temp_dir().join(format!("jcode-bridge-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("jcode-api.sock");

        let first = super::single_instance_lock(&socket).unwrap();
        assert!(first.is_some(), "the first bridge must take the lock");
        assert!(
            super::single_instance_lock(&socket).unwrap().is_none(),
            "a second bridge must be refused while the first is alive"
        );

        // Once the owner is gone the lock is available again, so a crashed
        // bridge never wedges its replacement out.
        drop(first);
        assert!(super::single_instance_lock(&socket).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(test, unix))]
mod socket_permission_tests {
    /// The API socket must never be more permissive than the daemon socket it
    /// fronts.
    ///
    /// This regressed once: `UnixListener::bind` applies the process umask, so
    /// the socket landed at 0755 while the daemon socket it bridges to is
    /// 0600. Every guarantee behind the daemon socket was then reachable by
    /// any local user, including reading transcripts and spending the owner's
    /// provider tokens.
    #[tokio::test]
    async fn the_api_socket_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("jcode-api-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let api_socket = dir.join("api.sock");
        let legacy_socket = dir.join("daemon.sock");

        let bridge_socket = api_socket.clone();
        let handle = tokio::spawn(async move {
            let _ = super::run_bridge(bridge_socket, legacy_socket).await;
        });

        // Wait for the bind, which happens before the accept loop.
        let mut mode = None;
        for _ in 0..100 {
            if let Ok(meta) = std::fs::metadata(&api_socket) {
                mode = Some(meta.permissions().mode() & 0o777);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            mode,
            Some(0o600),
            "API socket must be owner-only (0600); a wider mode exposes every \
             session behind the bridge to other local users"
        );
    }
}
