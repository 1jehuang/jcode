use super::STREAM_KEEPALIVE_PONG_ID;
use crate::protocol::ServerEvent;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};

fn stream_keepalive_interval() -> Duration {
    if cfg!(test) {
        Duration::from_millis(50)
    } else {
        Duration::from_secs(30)
    }
}

pub(super) fn stream_keepalive_ticker() -> time::Interval {
    let interval = stream_keepalive_interval();
    let mut ticker = time::interval_at(time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

pub(super) fn send_stream_keepalive_mpsc(event_tx: &mpsc::UnboundedSender<ServerEvent>) {
    let _ = event_tx.send(ServerEvent::Pong {
        id: STREAM_KEEPALIVE_PONG_ID,
        native_ssh_protocol: None,
        capabilities: Vec::new(),
        user_connected_session_ids: None,
    });
}

/// Largest byte index `<= index` that is a UTF-8 char boundary in `text`.
/// Equivalent to the unstable `str::floor_char_boundary`, reimplemented so the
/// incremental marker scan can clamp its scan-window start onto a valid
/// boundary without re-scanning the whole accumulated response.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut boundary = index;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// The wrapped-tool-call markers emitted by some models inside plain text.
const WRAP_TOOL_MARKERS: [&str; 2] = ["to=functions.", "+#+#"];

type StreamUsageSink = fn(&str, &str, &str, [Option<u64>; 4]);

struct ObservedStreamUsage {
    provider: String,
    model: String,
    counters: [Option<u64>; 4],
}

/// Keep the latest cumulative usage snapshot for one upstream response.
///
/// The drop path also runs when a task is cancelled or an error exits the loop.
/// Capture routing identity when usage arrives, rather than using the requested
/// model or consulting mutable routing state after compaction/tool execution.
pub(super) struct StreamUsageRecorder<
    F: FnMut(&str, &str, &str, [Option<u64>; 4]) = StreamUsageSink,
> {
    session_id: String,
    observed: Option<ObservedStreamUsage>,
    emit: F,
}

impl StreamUsageRecorder {
    pub(super) fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            observed: None,
            emit: record_partial_stream_usage,
        }
    }
}

impl<F: FnMut(&str, &str, &str, [Option<u64>; 4])> StreamUsageRecorder<F> {
    pub(super) fn observe(&mut self, provider: &str, model: &str, counters: [Option<u64>; 4]) {
        if counters.iter().any(Option::is_some) {
            self.observed = Some(ObservedStreamUsage {
                provider: provider.to_string(),
                model: model.to_string(),
                counters,
            });
        }
    }

    pub(super) fn finish(&mut self) {
        if let Some(usage) = self.observed.take() {
            (self.emit)(
                &self.session_id,
                &usage.provider,
                &usage.model,
                usage.counters,
            );
        }
    }
}

impl<F: FnMut(&str, &str, &str, [Option<u64>; 4])> Drop for StreamUsageRecorder<F> {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Report whatever usage a provider stream reported before it failed.
///
/// The normal `usage_report` is emitted after the stream completes. A stream
/// that errors (or is retried after compaction) still consumed the tokens the
/// provider already reported, so count them. `[input, output, cache_read,
/// cache_creation]`; a no-op when nothing was reported.
pub(super) fn record_partial_stream_usage(
    session_id: &str,
    provider: &str,
    model: &str,
    [input, output, cache_read, cache_creation]: [Option<u64>; 4],
) {
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_creation.is_none() {
        return;
    }
    crate::telemetry::record_provider_usage(
        Some(session_id),
        provider,
        model,
        crate::telemetry::UsageSource::Agent,
        crate::telemetry::ProviderUsage {
            input_tokens: input.unwrap_or(0),
            output_tokens: output.unwrap_or(0),
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_creation,
        },
    );
}

/// Find the first wrapped-tool-call marker in `accumulated`, scanning only the
/// newly appended `delta` plus a short overlap from the previous tail (so a
/// marker straddling the append boundary is still found).
///
/// This avoids re-scanning the entire accumulated response on every streamed
/// delta, which was O(response) per token and O(response^2) over a full answer.
pub(super) fn find_wrap_marker_incremental(
    accumulated: &str,
    appended_len: usize,
) -> Option<usize> {
    let max_marker_len = WRAP_TOOL_MARKERS
        .iter()
        .map(|marker| marker.len())
        .max()
        .unwrap_or(0);
    let scan_start = accumulated
        .len()
        .saturating_sub(appended_len + max_marker_len.saturating_sub(1));
    let scan_start = floor_char_boundary(accumulated, scan_start);
    let window = &accumulated[scan_start..];
    WRAP_TOOL_MARKERS
        .iter()
        .filter_map(|marker| window.find(marker))
        .min()
        .map(|rel_idx| scan_start + rel_idx)
}

#[cfg(test)]
mod usage_capture_tests {
    use super::*;

    #[test]
    fn cumulative_usage_is_emitted_once_with_observed_fallback_identity() {
        let mut reports = Vec::new();
        {
            let mut recorder = StreamUsageRecorder {
                session_id: "worker-session".to_string(),
                observed: None,
                emit: |session: &str, provider: &str, model: &str, counts| {
                    reports.push((
                        session.to_string(),
                        provider.to_string(),
                        model.to_string(),
                        counts,
                    ));
                },
            };
            recorder.observe("anthropic", "requested", [Some(10), Some(1), None, None]);
            recorder.observe(
                "anthropic",
                "served-fallback",
                [Some(10), Some(5), Some(100), None],
            );
            recorder.finish();
            recorder.finish();
        }
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].0, "worker-session");
        assert_eq!(reports[0].2, "served-fallback");
        assert_eq!(reports[0].3, [Some(10), Some(5), Some(100), None]);
    }

    #[test]
    fn error_exit_preserves_cached_only_usage_without_inventing_unreported_tokens() {
        let mut reports = Vec::new();
        let result: Result<(), ()> = {
            let mut recorder = StreamUsageRecorder {
                session_id: "worker-session".to_string(),
                observed: None,
                emit: |_: &str, _: &str, _: &str, counts| reports.push(counts),
            };
            recorder.observe("anthropic", "model", [None, None, None, Some(20)]);
            Err(())
        };
        assert!(result.is_err());
        assert_eq!(reports, vec![[None, None, None, Some(20)]]);
        {
            let _unobserved = StreamUsageRecorder {
                session_id: "worker-session".to_string(),
                observed: None,
                emit: |_: &str, _: &str, _: &str, _| panic!("invented unreported usage"),
            };
        }
    }

    #[tokio::test]
    async fn aborting_a_stream_task_preserves_already_observed_usage() {
        let reports = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = reports.clone();
        let (ready, observed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut recorder = StreamUsageRecorder {
                session_id: "worker-session".to_string(),
                observed: None,
                emit: move |_: &str, _: &str, _: &str, counts| {
                    captured.lock().unwrap().push(counts)
                },
            };
            recorder.observe(
                "openai",
                "served-model",
                [Some(100), Some(7), Some(90), None],
            );
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
            recorder.finish();
        });
        observed.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            *reports.lock().unwrap(),
            vec![[Some(100), Some(7), Some(90), None]]
        );
    }
}
