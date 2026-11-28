use super::{ACP_HANDSHAKE_TIMEOUT, Result, STDERR_TAIL_BYTES, StreamEvent, Value, anyhow, bail};
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::ChildStderr;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};

pub(super) async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: Value,
    tx: Option<&mpsc::Sender<Result<StreamEvent>>>,
    limit: Duration,
) -> Result<()> {
    let mut encoded = serde_json::to_vec(&value)?;
    encoded.push(b'\n');
    let write = async {
        writer.write_all(&encoded).await?;
        writer.flush().await
    };
    let bounded = timeout(limit, write);
    let result = if let Some(tx) = tx {
        tokio::select! {
            biased;
            _ = tx.closed() => bail!("Cursor ACP stream was cancelled"),
            result = bounded => result,
        }
    } else {
        bounded.await
    };
    result.map_err(|_| {
        anyhow!("Cursor ACP write did not complete within {limit:?}; subprocess may be hung")
    })??;
    Ok(())
}

pub(super) async fn write_request(
    writer: &mut tokio::process::ChildStdin,
    value: Value,
    tx: Option<&mpsc::Sender<Result<StreamEvent>>>,
) -> Result<()> {
    write_frame(writer, value, tx, ACP_HANDSHAKE_TIMEOUT).await
}

pub(super) fn trim_tail(tail: &mut String, limit: usize) {
    if tail.len() <= limit {
        return;
    }
    let mut start = tail.len() - limit;
    while !tail.is_char_boundary(start) {
        start += 1;
    }
    tail.drain(..start);
}

pub(super) fn capture_stderr(stderr: ChildStderr, tail: Arc<RwLock<String>>) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let mut tail = tail.write().unwrap_or_else(|e| e.into_inner());
                    tail.push_str(&line);
                    trim_tail(&mut tail, STDERR_TAIL_BYTES);
                }
            }
        }
    });
}
