//! Content-free client-side cloud latency milestones.
use crate::protocol::{Request, ServerEvent};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Instant;

#[cfg(test)]
#[path = "cloud_latency_tests.rs"]
mod tests;

const MAX_PENDING: usize = 64;
thread_local! {
    static REQUESTS: RefCell<VecDeque<(u64, &'static str, Instant)>> = const { RefCell::new(VecDeque::new()) };
    static PAINT: RefCell<Option<(&'static str, Instant)>> = const { RefCell::new(None) };
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

pub(super) fn request_sent(request: &Request) {
    let Some((id, kind)) = request_key(request) else {
        return;
    };
    REQUESTS.with_borrow_mut(|pending| {
        pending.retain(|(previous, operation, at)| {
            if at.elapsed().as_secs() >= 120 {
                crate::logging::info(&format!("[CLOUD_TIMING] stage=response_wait operation={operation} request_id={previous} outcome=expired duration_ms={:.3}", at.elapsed().as_secs_f64() * 1000.0));
                false
            } else { *previous != id }
        });
        if pending.len() == MAX_PENDING { pending.pop_front(); }
        pending.push_back((id, kind, Instant::now()));
    });
    crate::logging::info(&format!(
        "[CLOUD_TIMING] stage=request_sent operation={kind} request_id={id} outcome=ok duration_ms=0"
    ));
}

pub(super) fn event_key(event: &ServerEvent) -> Option<(&'static str, Option<u64>)> {
    match event {
        ServerEvent::History { id, .. } => Some(("history", Some(*id))),
        ServerEvent::CompactedHistory { id, .. } => Some(("history_page", Some(*id))),
        ServerEvent::Todos { .. } => Some(("todos", None)),
        _ => None,
    }
}

pub(super) fn parse_event(text: &str) -> serde_json::Result<ServerEvent> {
    let started = Instant::now();
    let result = serde_json::from_str(text);
    let key = match &result {
        Ok(event) => event_key(event),
        Err(_) => None,
    };
    if key.is_some() || result.is_err() || started.elapsed().as_millis() >= 100 {
        let (kind, id) = key.unwrap_or(("other", None));
        let wait = REQUESTS.with_borrow_mut(|pending| {
            let index = pending.iter().position(|(request, operation, _)| {
                id == Some(*request)
                    || (kind == "todos" && matches!(*operation, "todos" | "subscribe" | "resume"))
            })?;
            // Early Todos must not consume the History correlation for Subscribe.
            let elapsed = pending[index].2.elapsed();
            if kind != "todos" || pending[index].1 == "todos" {
                pending.remove(index);
            }
            Some(elapsed.as_secs_f64() * 1000.0)
        });
        crate::logging::info(&format!(
            "[CLOUD_TIMING] stage=event_parse operation={kind} request_id={id:?} outcome={} duration_ms={:.3} bytes={} request_wait_ms={}",
            if result.is_ok() { "ok" } else { "error" },
            started.elapsed().as_secs_f64() * 1000.0,
            text.len(),
            wait.map(|ms| format!("{ms:.3}"))
                .unwrap_or_else(|| "none".into())
        ));
    }
    result
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
