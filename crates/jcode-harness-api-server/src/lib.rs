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

pub mod translate;

use anyhow::{Context, Result};
use jcode_harness_api::{API_VERSION_MAJOR, ApiEvent, ErrorCode, ServerFrame};
use serde_json::Value;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

pub fn api_socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("JCODE_API_SOCKET") {
        return PathBuf::from(custom);
    }
    jcode_home().join("jcode-api.sock")
}

pub fn legacy_socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("JCODE_SOCKET") {
        return PathBuf::from(custom);
    }
    jcode_home().join("jcode.sock")
}

fn jcode_home() -> PathBuf {
    if let Ok(dir) = std::env::var("JCODE_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(dir);
        if runtime.join("jcode.sock").exists() {
            return runtime;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".jcode")
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
    reader.read_line(&mut line).await?;
    let hello: Value = serde_json::from_str(line.trim()).context("parse hello")?;
    let reply_to = hello["id"].as_u64().unwrap_or(0);
    let compatible = hello["req"] == "hello"
        && hello["min_version"].as_u64().unwrap_or(0) <= u64::from(API_VERSION_MAJOR)
        && hello["max_version"].as_u64().unwrap_or(0) >= u64::from(API_VERSION_MAJOR);
    if !compatible {
        let frame = ServerFrame::reply(
            reply_to,
            ApiEvent::Error {
                code: ErrorCode::UnsupportedVersion,
                message: format!("bridge speaks API v{API_VERSION_MAJOR}; send hello first"),
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
            n = reader.read_line({ api_line.clear(); &mut api_line }) => {
                if n? == 0 { return Ok(()); }
                if api_line.trim().is_empty() { continue; }
                let request: Value = match serde_json::from_str(api_line.trim()) {
                    Ok(value) => value,
                    Err(error) => {
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
