//! Per-attempt stream output tracking for provider retry loops.
//!
//! Retry loops re-invoke a provider's `stream_response` with the same event
//! sender after a transient transport fault (e.g. TLS `BadRecordMac`, idle
//! timeout, connection reset). If the failed attempt already streamed output
//! to the consumer, blindly replaying the request duplicates that output.
//!
//! [`track_attempt_output`] wraps the outer sender for the duration of one
//! attempt and records whether any *replay-visible* event passed through. On a
//! retryable failure the loop can then emit [`StreamEvent::RetryRollback`] so
//! consumers discard the partial output before the replay streams in.
//!
//! This is safe for jcode's HTTP providers because tools are only executed by
//! the agent loop *after* the stream completes; a partially streamed attempt
//! has no side effects. The Claude CLI path is the exception (the CLI executes
//! tools live mid-stream) and keeps its no-retry-after-output guard instead.

use anyhow::Result;
use jcode_message_types::StreamEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Whether an event renders as content on the consumer side, such that
/// replaying the request after it was emitted would visibly duplicate output.
///
/// Status-style events (connection phases, token usage snapshots, transport
/// labels) are excluded: consumers overwrite rather than accumulate them, so a
/// replay is harmless.
fn stream_event_is_replay_visible(event: &StreamEvent) -> bool {
    match event {
        StreamEvent::TextDelta(_)
        | StreamEvent::TextDone
        | StreamEvent::ToolUseStart { .. }
        | StreamEvent::ToolInputDelta(_)
        | StreamEvent::ToolInputDeltaFor { .. }
        | StreamEvent::ToolUseEndFor { .. }
        | StreamEvent::ToolUseSignatureFor { .. }
        | StreamEvent::ToolUseEnd
        | StreamEvent::ToolUseSignature(_)
        | StreamEvent::ToolResult { .. }
        | StreamEvent::GeneratedImage { .. }
        | StreamEvent::ThinkingDelta(_)
        | StreamEvent::ThinkingSignatureDelta(_)
        | StreamEvent::OpenAIReasoning { .. }
        | StreamEvent::MessageEnd { .. }
        | StreamEvent::Compaction { .. }
        | StreamEvent::NativeToolCall { .. } => true,
        StreamEvent::ThinkingStart
        | StreamEvent::ThinkingEnd
        | StreamEvent::ThinkingDone { .. }
        | StreamEvent::RetryRollback { .. }
        | StreamEvent::TokenUsage { .. }
        | StreamEvent::ConnectionType { .. }
        | StreamEvent::ConnectionPhase { .. }
        | StreamEvent::StatusDetail { .. }
        | StreamEvent::Error { .. }
        | StreamEvent::SessionId(_)
        | StreamEvent::UpstreamProvider { .. } => false,
    }
}

/// Handle returned by [`track_attempt_output`]. Await [`AttemptGuard::finish`]
/// after the attempt completes (and after dropping the attempt sender) to
/// drain in-flight events and learn whether the attempt streamed output.
pub struct AttemptGuard {
    saw_output: Arc<AtomicBool>,
    forwarder: JoinHandle<()>,
}

impl AttemptGuard {
    /// Wait for all events the attempt buffered to be forwarded to the outer
    /// sender (preserving ordering relative to any subsequent rollback event),
    /// then report whether any replay-visible output was emitted.
    pub async fn finish(self) -> bool {
        let _ = self.forwarder.await;
        self.saw_output.load(Ordering::SeqCst)
    }
}

/// Wrap `outer` for a single retry attempt. Returns a sender to pass into the
/// attempt's `stream_response` call and a guard that reports whether the
/// attempt emitted replay-visible output.
///
/// All events are forwarded in order to `outer`. The forwarder ends when the
/// attempt sender (and all its clones) are dropped, so callers must drop the
/// returned sender (usually implicit: `stream_response` consumes it) before
/// awaiting the guard.
pub fn track_attempt_output(
    outer: mpsc::Sender<Result<StreamEvent>>,
) -> (mpsc::Sender<Result<StreamEvent>>, AttemptGuard) {
    let (attempt_tx, mut attempt_rx) = mpsc::channel::<Result<StreamEvent>>(32);
    let saw_output = Arc::new(AtomicBool::new(false));
    let saw_output_writer = Arc::clone(&saw_output);
    let forwarder = tokio::spawn(async move {
        while let Some(item) = attempt_rx.recv().await {
            if let Ok(event) = &item
                && stream_event_is_replay_visible(event)
            {
                saw_output_writer.store(true, Ordering::SeqCst);
            }
            if outer.send(item).await.is_err() {
                // Consumer hung up; closing the attempt channel lets the
                // in-flight attempt observe `tx.closed()` / send errors.
                break;
            }
        }
    });
    (
        attempt_tx,
        AttemptGuard {
            saw_output,
            forwarder,
        },
    )
}

/// Exponential backoff with +/-20% jitter for provider retry loops.
///
/// Pure exponential backoff retries every in-flight session in lockstep during
/// a correlated upstream outage (thundering herd). Jitter spreads the retries
/// out. `attempt` is the 0-based index of the attempt about to run (so the
/// first retry, `attempt == 1`, waits ~`base_ms`).
pub fn retry_backoff_delay(attempt: u32, base_ms: u64) -> std::time::Duration {
    use rand::Rng;
    let exp = base_ms.saturating_mul(1u64 << attempt.saturating_sub(1).min(16));
    let jittered = (exp as f64 * rand::rng().random_range(0.8..1.2)) as u64;
    std::time::Duration::from_millis(jittered.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_tool_events_are_replay_visible() {
        assert!(stream_event_is_replay_visible(&StreamEvent::TextDelta(
            "hi".into()
        )));
        assert!(stream_event_is_replay_visible(&StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "bash".into(),
        }));
        assert!(stream_event_is_replay_visible(&StreamEvent::ThinkingDelta(
            "hmm".into()
        )));
        assert!(stream_event_is_replay_visible(&StreamEvent::MessageEnd {
            stop_reason: None,
        }));
    }

    #[test]
    fn keyed_tool_events_are_replay_visible() {
        for event in [
            StreamEvent::ToolInputDeltaFor {
                id: "a".into(),
                delta: "{".into(),
            },
            StreamEvent::ToolUseEndFor { id: "a".into() },
            StreamEvent::ToolUseSignatureFor {
                id: "a".into(),
                signature: "sig".into(),
            },
        ] {
            assert!(stream_event_is_replay_visible(&event), "{event:?}");
        }
    }

    #[test]
    fn status_events_are_not_replay_visible() {
        assert!(!stream_event_is_replay_visible(
            &StreamEvent::ConnectionPhase {
                phase: jcode_message_types::ConnectionPhase::Connecting,
            }
        ));
        assert!(!stream_event_is_replay_visible(&StreamEvent::TokenUsage {
            input_tokens: Some(1),
            output_tokens: Some(2),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        }));
        assert!(!stream_event_is_replay_visible(
            &StreamEvent::StatusDetail {
                detail: "https".into(),
            }
        ));
        assert!(!stream_event_is_replay_visible(
            &StreamEvent::RetryRollback { attempt: 1, max: 3 }
        ));
    }

    #[tokio::test]
    async fn tracker_forwards_in_order_and_reports_output() {
        let (outer_tx, mut outer_rx) = mpsc::channel::<Result<StreamEvent>>(8);
        let (attempt_tx, guard) = track_attempt_output(outer_tx.clone());

        attempt_tx
            .send(Ok(StreamEvent::ConnectionPhase {
                phase: jcode_message_types::ConnectionPhase::Connecting,
            }))
            .await
            .unwrap();
        attempt_tx
            .send(Ok(StreamEvent::TextDelta("partial".into())))
            .await
            .unwrap();
        drop(attempt_tx);

        // Guard must report output and only resolve after both events are
        // forwarded, so a rollback sent afterwards is ordered behind them.
        assert!(guard.finish().await);
        let _ = outer_tx
            .send(Ok(StreamEvent::RetryRollback { attempt: 1, max: 3 }))
            .await;

        let first = outer_rx.recv().await.unwrap().unwrap();
        assert!(matches!(first, StreamEvent::ConnectionPhase { .. }));
        let second = outer_rx.recv().await.unwrap().unwrap();
        assert!(matches!(second, StreamEvent::TextDelta(t) if t == "partial"));
        let third = outer_rx.recv().await.unwrap().unwrap();
        assert!(matches!(
            third,
            StreamEvent::RetryRollback { attempt: 1, max: 3 }
        ));
    }

    #[tokio::test]
    async fn tracker_reports_no_output_for_status_only_attempt() {
        let (outer_tx, _outer_rx) = mpsc::channel::<Result<StreamEvent>>(8);
        let (attempt_tx, guard) = track_attempt_output(outer_tx);
        attempt_tx
            .send(Ok(StreamEvent::ConnectionPhase {
                phase: jcode_message_types::ConnectionPhase::Connecting,
            }))
            .await
            .unwrap();
        drop(attempt_tx);
        assert!(!guard.finish().await);
    }

    #[test]
    fn backoff_delay_is_jittered_exponential() {
        for attempt in 1..=3u32 {
            let base = 1000u64;
            let exp = base * (1 << (attempt - 1));
            let lo = (exp as f64 * 0.8) as u64;
            let hi = (exp as f64 * 1.2) as u64 + 1;
            for _ in 0..50 {
                let d = retry_backoff_delay(attempt, base).as_millis() as u64;
                assert!(
                    (lo..=hi).contains(&d),
                    "attempt {attempt}: delay {d} outside [{lo}, {hi}]"
                );
            }
        }
    }
}
