use super::*;

pub(super) struct RequestHandlerWatchdog {
    pub(super) done: Arc<AtomicBool>,
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

pub(super) fn log_request_lifecycle_handled(
    fields: ServerRequestLifecycleFields<'_>,
    request_lifecycle_start: Instant,
    request_decoded_at: Instant,
) {
    let mut fields = server_request_lifecycle_fields(fields);
    fields.push((
        "handler_total_ms".to_string(),
        request_lifecycle_start.elapsed().as_millis().to_string(),
    ));
    fields.push((
        "since_decode_ms".to_string(),
        request_decoded_at.elapsed().as_millis().to_string(),
    ));
    crate::logging::event_info("SERVER_REQUEST_LIFECYCLE", fields);
}

pub(super) fn should_start_idle_soft_interrupt(
    client_is_processing: bool,
    active_turn_registered: bool,
    session_connection_busy: bool,
) -> bool {
    !client_is_processing && !active_turn_registered && !session_connection_busy
}

pub(super) fn reject_if_agent_busy_for_request(
    request_id: u64,
    request_kind: &'static str,
    client_session_id: &str,
    client_is_processing: bool,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) -> bool {
    if agent.try_lock().is_ok() {
        return false;
    }

    send_agent_busy_error(
        request_id,
        request_kind,
        client_session_id,
        client_is_processing,
        client_event_tx,
    );
    true
}

pub(super) fn send_agent_busy_error(
    request_id: u64,
    request_kind: &'static str,
    client_session_id: &str,
    client_is_processing: bool,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    crate::logging::event_warn(
        "SERVER_REQUEST_BUSY_AGENT_REJECTED",
        vec![
            ("request_id", request_id.to_string()),
            ("request_kind", request_kind.to_string()),
            ("session_id", client_session_id.to_string()),
            ("client_processing", client_is_processing.to_string()),
            ("reason", "agent_busy".to_string()),
        ],
    );
    if let Err(error) = client_event_tx.send(ServerEvent::Error {
        id: request_id,
        message: format!(
            "Cannot handle {request_kind} while the session is busy. Try again after the current turn finishes."
        ),
        retry_after_secs: Some(1),
    }) {
        crate::logging::debug(&format!("Busy rejection could not reach disconnected client: {error}"));
    }
}
