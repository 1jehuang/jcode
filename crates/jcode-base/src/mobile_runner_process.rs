//! Structured child supervision. Both pipes and process exit share one deadline.
use anyhow::{Context, Result};
use std::{path::PathBuf, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

const STDERR_TAIL_BYTES: usize = 600 * 4 + 4;

struct BusyMarker(Option<PathBuf>);
impl BusyMarker {
    fn touch(&self) {
        if let Some(path) = &self.0
            && let Err(error) = std::fs::write(path, b"")
        {
            crate::logging::warn(&format!("mobile runner busy marker update failed: {error}"));
        }
    }
}
impl Drop for BusyMarker {
    fn drop(&mut self) {
        if let Some(path) = &self.0
            && let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::logging::warn(&format!(
                "mobile runner busy marker removal failed: {error}"
            ));
        }
    }
}

async fn stderr_tail(mut stderr: tokio::process::ChildStderr) -> Result<String> {
    let mut tail = Vec::with_capacity(STDERR_TAIL_BYTES);
    let mut chunk = [0; 8192];
    loop {
        let n = stderr.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        tail.extend_from_slice(&chunk[..n]);
        if tail.len() > STDERR_TAIL_BYTES {
            tail.drain(..tail.len() - STDERR_TAIL_BYTES);
        }
    }
    // Keep enough raw bytes for 600 four-byte scalars, then trim by characters.
    // Lossy decoding also preserves the former behavior for non-UTF-8 stderr.
    let text = String::from_utf8_lossy(&tail);
    Ok(text
        .chars()
        .rev()
        .take(600)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect())
}

pub(super) async fn run(
    mut command: tokio::process::Command,
    session_id: Option<&str>,
    marker: Option<PathBuf>,
    timeout: Duration,
    touch_interval: Duration,
) -> Result<String> {
    let marker = BusyMarker(marker);
    command.kill_on_drop(true);
    let mut child = command.spawn().context("starting jcode run")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let stderr = child.stderr.take().context("no stderr")?;
    marker.touch();
    let mut touch = tokio::time::interval(touch_interval);
    let result = {
        let output = async {
            let read_stdout = async {
                let mut session = session_id.map(str::to_owned);
                let mut lines = BufReader::new(stdout).lines();
                while let Some(line) = lines.next_line().await? {
                    if session.is_none() {
                        session = super::session_id_from_ndjson_line(&line);
                    }
                }
                Ok::<_, anyhow::Error>(session)
            };
            let (session, tail, status) =
                tokio::try_join!(read_stdout, stderr_tail(stderr), async {
                    child.wait().await.context("waiting for jcode run")
                },)?;
            anyhow::ensure!(
                status.success(),
                "jcode run failed ({}): {}",
                status,
                tail.trim()
            );
            session.context("jcode run did not report a session id")
        };
        let deadline = tokio::time::timeout(timeout, output);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                result = &mut deadline => break result.unwrap_or_else(|_| Err(anyhow::anyhow!("turn timed out"))),
                _ = touch.tick() => marker.touch(),
            }
        }
    };
    // No spawned reader tasks: cancellation drops both readers before reaping.
    if result.is_err()
        && let Err(error) = child.kill().await
    {
        crate::logging::warn(&format!("mobile runner child cleanup failed: {error}"));
    }
    result
}
