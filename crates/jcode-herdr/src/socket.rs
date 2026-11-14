//! HERDR Unix socket communication.
//!
//! Sends newline-delimited JSON requests to HERDR's local socket and
//! reads one-line responses. Used for `pane.report_agent`,
//! `pane.report_agent_session`, and `pane.release_agent`.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;

/// Send a JSON-RPC-style request to HERDR's socket and return the
/// parsed response. Each request is one newline-terminated JSON line;
/// the response is the first line from HERDR.
#[cfg(unix)]
pub async fn send_request(socket_path: &Path, request: &Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket_path).await.with_context(|| {
        format!(
            "failed to connect to HERDR socket at {}",
            socket_path.display()
        )
    })?;

    let mut line = serde_json::to_string(request).context("failed to serialize HERDR request")?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .await
        .context("failed to write to HERDR socket")?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .await
        .context("failed to read HERDR response")?;

    if response_line.is_empty() {
        anyhow::bail!("HERDR returned empty response");
    }

    serde_json::from_str(&response_line)
        .with_context(|| format!("failed to parse HERDR response: {response_line}"))
}

/// HERDR's control socket is a Unix domain socket, which this platform
/// does not provide. Callers treat the transport as best-effort, so a
/// platform error is reported instead of a hard build failure.
#[cfg(not(unix))]
pub async fn send_request(socket_path: &Path, _request: &Value) -> Result<Value> {
    anyhow::bail!(
        "HERDR socket transport is unavailable on this platform ({} is unix-only)",
        socket_path.display()
    )
}

/// Fire-and-forget: send a request and ignore the response.
/// Used for non-critical state reports where latency matters more
/// than confirmation.
pub async fn send_fire_and_forget(socket_path: &Path, request: &Value) {
    if let Err(e) = send_request(socket_path, request).await {
        tracing::debug!("HERDR fire-and-forget failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialization() {
        let req = serde_json::json!({
            "id": "test:1",
            "method": "pane.report_agent",
            "params": {
                "pane_id": "w1:p1",
                "source": "custom:jcode",
                "agent": "jcode",
                "state": "working"
            }
        });
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("pane.report_agent"));
        assert!(s.contains("w1:p1"));
    }
}
