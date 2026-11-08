//! Content-free client-side cloud latency milestones.
use crate::protocol::{Request, ServerEvent};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

#[cfg(test)]
#[path = "cloud_latency_tests.rs"]
mod tests;

const MAX_PENDING: usize = 64;
thread_local! {
    static PAINT: RefCell<Option<(&'static str, Instant)>> = const { RefCell::new(None) };
}

// Async connection futures can migrate between executor threads. Keep request
// correlation on the connection, not the thread or a process-global request map.
#[derive(Default)]
pub(super) struct RequestTimings {
    pending: Mutex<VecDeque<PendingRequest>>,
}
struct PendingRequest {
    id: u64,
    operation: &'static str,
    started: Instant,
    history_expected: bool,
    todos_seen: bool,
}

pub(super) struct Span {
    stage: &'static str,
    started: Instant,
    outcome: &'static str,
    operation: &'static str,
    request_id: Option<u64>,
}
impl Span {
    pub(super) fn new(stage: &'static str) -> Self {
        Self {
            stage,
            started: Instant::now(),
            outcome: "error_or_cancelled",
            operation: "none",
            request_id: None,
        }
    }
    pub(super) fn operation(&mut self, operation: &'static str) {
        self.operation = operation;
    }
    pub(super) fn finish(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        crate::logging::info(&format!(
            "[CLOUD_TIMING] stage={} operation={} request_id={:?} outcome={} duration_ms={:.3}",
            self.stage,
            self.operation,
            self.request_id,
            self.outcome,
            self.started.elapsed().as_secs_f64() * 1000.0
        ));
    }
}

fn request_key(request: &Request) -> Option<(u64, &'static str)> {
    match request {
        Request::Subscribe { id, .. } => Some((*id, "subscribe")),
        Request::ResumeSession { id, .. } => Some((*id, "resume")),
        Request::GetHistory { id } => Some((*id, "history")),
        Request::GetCompactedHistory { id, .. } => Some((*id, "history_page")),
        Request::GetTodos { id } => Some((*id, "todos")),
        _ => None,
    }
}

pub(super) fn begin_request(request: &Request) -> Option<Span> {
    request_key(request).map(|(id, kind)| {
        let mut span = Span::new("request_write");
        span.operation = kind;
        span.request_id = Some(id);
        span
    })
}

impl RequestTimings {
    pub(super) fn request_sent(&self, request: &Request) {
        let Some((id, kind)) = request_key(request) else {
            return;
        };
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pending.retain(|previous| {
            if previous.started.elapsed().as_secs() >= 120 {
                crate::logging::info(&format!("[CLOUD_TIMING] stage=response_wait operation={} request_id={} outcome=expired duration_ms={:.3}", previous.operation, previous.id, previous.started.elapsed().as_secs_f64() * 1000.0));
                false
            } else { previous.id != id }
        });
        if pending.len() == MAX_PENDING {
            pending.pop_front();
        }
        pending.push_back(PendingRequest {
            id,
            operation: kind,
            started: Instant::now(),
            // Fresh Subscribe completes with Done, followed by a separately
            // requested History. A target-aware attach returns History itself.
            history_expected: !matches!(
                request,
                Request::Subscribe {
                    target_session_id: None,
                    ..
                }
            ),
            todos_seen: false,
        });
        crate::logging::info(&format!(
            "[CLOUD_TIMING] stage=request_sent operation={kind} request_id={id} outcome=ok duration_ms=0"
        ));
    }

    fn response_wait(&self, event: &ServerEvent) -> Option<(&'static str, u64, f64)> {
        if !matches!(
            event,
            ServerEvent::Todos { .. }
                | ServerEvent::History { .. }
                | ServerEvent::CompactedHistory { .. }
                | ServerEvent::Error { .. }
                | ServerEvent::Done { .. }
        ) {
            return None;
        }
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let index = match event {
            ServerEvent::Todos { .. } => {
                // Explicit polls take priority over an attach's early snapshot.
                // A bootstrap snapshot must not consume History correlation or
                // keep matching every unsolicited snapshot for that connection.
                pending
                    .iter()
                    .position(|request| request.operation == "todos")
                    .or_else(|| {
                        pending.iter().position(|request| {
                            !request.todos_seen
                                && matches!(request.operation, "subscribe" | "resume")
                        })
                    })?
            }
            ServerEvent::History { id, .. }
            | ServerEvent::CompactedHistory { id, .. }
            | ServerEvent::Error { id, .. } => {
                pending.iter().position(|request| request.id == *id)?
            }
            ServerEvent::Done { id } => pending.iter().position(|request| {
                request.id == *id && request.operation == "subscribe" && !request.history_expected
            })?,
            _ => return None,
        };
        let request = &mut pending[index];
        let result = (
            request.operation,
            request.id,
            request.started.elapsed().as_secs_f64() * 1000.0,
        );
        if matches!(event, ServerEvent::Todos { .. }) && request.operation != "todos" {
            request.todos_seen = true;
        } else {
            pending.remove(index);
        }
        Some(result)
    }

    pub(super) fn parse_event(&self, text: &str) -> serde_json::Result<ServerEvent> {
        let started = Instant::now();
        let result = serde_json::from_str(text);
        let (key, wait) = match &result {
            Ok(event) => (event_key(event), self.response_wait(event)),
            Err(_) => (None, None),
        };
        if key.is_some()
            || wait.is_some()
            || result.is_err()
            || started.elapsed().as_millis() >= 100
        {
            let (kind, id) = key
                .or_else(|| wait.map(|(operation, id, _)| (operation, Some(id))))
                .unwrap_or(("other", None));
            let failed = result.is_err() || matches!(&result, Ok(ServerEvent::Error { .. }));
            crate::logging::info(&format!(
                "[CLOUD_TIMING] stage=event_parse operation={kind} request_id={id:?} outcome={} duration_ms={:.3} bytes={} request_wait_ms={}",
                if failed { "error" } else { "ok" },
                started.elapsed().as_secs_f64() * 1000.0,
                text.len(),
                wait.map(|(_, _, ms)| format!("{ms:.3}"))
                    .unwrap_or_else(|| "none".into())
            ));
        }
        result
    }
}

pub(super) fn event_key(event: &ServerEvent) -> Option<(&'static str, Option<u64>)> {
    match event {
        ServerEvent::History { id, .. } => Some(("history", Some(*id))),
        ServerEvent::CompactedHistory { id, .. } => Some(("history_page", Some(*id))),
        ServerEvent::Todos { .. } => Some(("todos", None)),
        _ => None,
    }
}

pub(super) fn applied(kind: &'static str, started: Instant, redraw: bool) {
    crate::logging::info(&format!(
        "[CLOUD_TIMING] stage=event_apply operation={kind} outcome=ok duration_ms={:.3} redraw={redraw}",
        started.elapsed().as_secs_f64() * 1000.0
    ));
    if redraw {
        PAINT.with_borrow_mut(|pending| {
            if pending.is_none() {
                *pending = Some((kind, Instant::now()));
            }
        });
    }
}

pub(super) fn rendered() {
    PAINT.with_borrow_mut(|pending| {
        if let Some((kind, at)) = pending.take() {
            crate::logging::info(&format!(
                "[CLOUD_TIMING] stage=frame_ready operation={kind} outcome=ok duration_ms={:.3}",
                at.elapsed().as_secs_f64() * 1000.0
            ));
        }
    });
}

pub(super) fn spawn_usage_hydration(
    hydrate: impl FnOnce() -> bool + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    let queued = Instant::now();
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let imported = hydrate();
        crate::logging::info(&format!(
            "[CLOUD_TIMING] stage=quota_hydration outcome=ok duration_ms={:.3} queue_ms={:.3} imported={imported}",
            started.elapsed().as_secs_f64() * 1000.0,
            started.duration_since(queued).as_secs_f64() * 1000.0,
        ));
    })
}
