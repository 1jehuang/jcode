use super::*;
use crate::transport::WriteHalf;
use anyhow::{Result, anyhow};

fn setup_runtime_dir() -> Result<(tempfile::TempDir, Option<std::ffi::OsString>)> {
    let runtime = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    Ok((runtime, prev_runtime))
}

fn restore_runtime_dir(prev_runtime: Option<std::ffi::OsString>) {
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

fn test_writer() -> Result<(Arc<Mutex<WriteHalf>>, crate::transport::Stream)> {
    let (stream_a, stream_b) = crate::transport::stream_pair().map_err(|e| anyhow!(e))?;
    let (_reader, writer_half) = stream_a.into_split();
    Ok((Arc::new(Mutex::new(writer_half)), stream_b))
}

/// Bind a peer stream to a background task that keeps reading it.
///
/// The History payload a live-attach resume writes can exceed the operating
/// system's socket buffer (measured at 14,642 bytes on master). `write_all`
/// then parks forever once that buffer fills, which deadlocks every resume test
/// that discards its peer. Draining the peer keeps the writer progressing, and
/// the channel carries the frames it read so a caller can still assert on the
/// exact payload the resume sent.
fn drain_peer(peer: crate::transport::Stream) -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut reader = tokio::io::BufReader::new(peer);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) if tx.send(line).is_err() => break,
                Ok(_) => {}
            }
        }
    });
    rx
}

/// Assert that the peer received a History frame for `session_id`.
///
/// Draining a peer without checking what arrived would let a resume silently
/// stop writing history and still pass, so every drained test pins the frame it
/// was previously too busy blocking to inspect. Bounded so a resume that fails
/// to write at all fails the test instead of parking it.
async fn assert_peer_history_for(
    peer: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    session_id: &str,
) -> Result<()> {
    let line = tokio::time::timeout(std::time::Duration::from_secs(5), peer.recv())
        .await
        .map_err(|_| anyhow!("peer never received the resume History frame"))?
        .ok_or_else(|| anyhow!("peer stream closed before the resume History frame"))?;
    let event: ServerEvent = serde_json::from_str(line.trim())?;
    match event {
        ServerEvent::History {
            session_id: got, ..
        } if got == session_id => Ok(()),
        other => Err(anyhow!("expected History for {session_id}, got {other:?}")),
    }
}

include!("resume/multiple_live_attach.rs");
include!("resume/busy_existing_attach.rs");
include!("resume/reconnect_takeover_with_history.rs");
include!("resume/attach_without_local_history.rs");
include!("resume/different_client_attach.rs");
include!("resume/live_events_before_history.rs");
include!("resume/same_client_takeover.rs");
