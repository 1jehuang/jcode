use super::REQUEST_HANDLER_STALL_THRESHOLDS_MS;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub(super) struct RequestHandlerWatchdog {
    done: Arc<AtomicBool>,
}

pub(super) struct RequestHandlerWatchdogContext {
    pub(super) request_id: u64,
    pub(super) request_kind: String,
    pub(super) client_session_id: String,
    pub(super) client_connection_id: String,
    pub(super) client_instance_id: Option<String>,
    pub(super) client_is_processing: bool,
    pub(super) message_id: Option<u64>,
    pub(super) processing_session_id: Option<String>,
    pub(super) line_bytes: usize,
    pub(super) lifecycle_logged: bool,
}

impl RequestHandlerWatchdog {
    pub(super) fn spawn(ctx: RequestHandlerWatchdogContext) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let done_for_task = Arc::clone(&done);
        tokio::spawn(async move {
            let started = Instant::now();
            let mut previous_threshold = Duration::ZERO;
            for threshold_ms in REQUEST_HANDLER_STALL_THRESHOLDS_MS {
                let threshold = Duration::from_millis(threshold_ms);
                tokio::time::sleep(threshold.saturating_sub(previous_threshold)).await;
                previous_threshold = threshold;
                if done_for_task.load(Ordering::Acquire) {
                    return;
                }
                crate::logging::event_warn(
                    "SERVER_REQUEST_HANDLER_STALLED",
                    vec![
                        ("request_id", ctx.request_id.to_string()),
                        ("request_kind", ctx.request_kind.clone()),
                        ("session_id", ctx.client_session_id.clone()),
                        ("client_connection_id", ctx.client_connection_id.clone()),
                        (
                            "client_instance_id",
                            ctx.client_instance_id
                                .clone()
                                .unwrap_or_else(|| "none".to_string()),
                        ),
                        ("client_processing", ctx.client_is_processing.to_string()),
                        (
                            "message_id",
                            ctx.message_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "none".to_string()),
                        ),
                        (
                            "processing_session_id",
                            ctx.processing_session_id
                                .clone()
                                .unwrap_or_else(|| "none".to_string()),
                        ),
                        ("line_bytes", ctx.line_bytes.to_string()),
                        ("lifecycle_logged", ctx.lifecycle_logged.to_string()),
                        ("threshold_ms", threshold_ms.to_string()),
                        ("elapsed_ms", started.elapsed().as_millis().to_string()),
                    ],
                );
            }
        });
        Self { done }
    }
}

impl Drop for RequestHandlerWatchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
    }
}
