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
use tokio::net::{UnixListener, UnixStream};

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

/// Run the bridge accept loop forever.
pub async fn run_bridge(api_socket: PathBuf, legacy_socket: PathBuf) -> Result<()> {
    let _ = std::fs::remove_file(&api_socket);
    if let Some(parent) = api_socket.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let listener = UnixListener::bind(&api_socket)
        .with_context(|| format!("bind API socket {}", api_socket.display()))?;
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

async fn handle_api_client(stream: UnixStream, legacy_socket: PathBuf) -> Result<()> {
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
            capabilities: vec!["sessions".into(), "streaming".into()],
        },
    );
    write_json_line(&mut write_half, &hello_ok).await?;

    // 2. Dial the legacy daemon for this client.
    let legacy = UnixStream::connect(&legacy_socket)
        .await
        .with_context(|| format!("connect legacy socket {}", legacy_socket.display()))?;
    let (legacy_read, mut legacy_write) = legacy.into_split();
    let mut legacy_reader = BufReader::new(legacy_read);

    let mut state = translate::BridgeState::default();

    // 3. Pump both directions in one select loop so translation state stays
    //    single-threaded.
    let mut api_line = String::new();
    let mut legacy_line = String::new();
    loop {
        tokio::select! {
            n = read_frame(&mut reader, &mut api_line) => {
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
                if api_line.trim().is_empty() { continue; }
                let request: Value = match serde_json::from_str(api_line.trim()) {
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
                for out in state.api_request_to_legacy(&request) {
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
                for frame in state.legacy_event_to_api(&event) {
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
