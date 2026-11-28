use super::*;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::io::AsyncWrite;

struct PendingFlush;

impl AsyncWrite for PendingFlush {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Pending
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn blocked_frame_write_is_cancelled_on_stream_close() {
    let (mut writer, _reader) = tokio::io::duplex(1);
    let (tx, rx) = mpsc::channel(1);
    let task = tokio::spawn(async move {
        transport::write_frame(
            &mut writer,
            json!({"large": "x".repeat(1024)}),
            Some(&tx),
            Duration::from_secs(30),
        )
        .await
    });
    tokio::task::yield_now().await;
    drop(rx);
    let error = timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
}

#[tokio::test]
async fn blocked_write_and_flush_have_bounded_timeouts() {
    let (mut writer, _reader) = tokio::io::duplex(1);
    let error = transport::write_frame(
        &mut writer,
        json!({"large":"x".repeat(1024)}),
        None,
        Duration::from_millis(10),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("write did not complete"));
    let error = transport::write_frame(
        &mut PendingFlush,
        json!({}),
        None,
        Duration::from_millis(10),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("write did not complete"));
}

#[tokio::test]
async fn blocked_flush_is_cancelled_on_stream_close() {
    let (tx, rx) = mpsc::channel(1);
    let task = tokio::spawn(async move {
        transport::write_frame(
            &mut PendingFlush,
            json!({}),
            Some(&tx),
            Duration::from_secs(30),
        )
        .await
    });
    tokio::task::yield_now().await;
    drop(rx);
    assert!(
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
}

#[test]
fn stderr_tail_trims_at_utf8_boundaries_with_a_strict_byte_limit() {
    let mut tail = "é".repeat(STDERR_TAIL_BYTES / 2 + 1);
    tail.push('x');
    transport::trim_tail(&mut tail, STDERR_TAIL_BYTES);
    assert!(tail.len() <= STDERR_TAIL_BYTES);
    assert!(tail.ends_with('x'));
    assert!(tail.starts_with('é'));
    transport::trim_tail(&mut tail, 0);
    assert!(tail.is_empty());
}
