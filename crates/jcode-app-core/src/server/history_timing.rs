use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_OPERATION: AtomicU64 = AtomicU64::new(1);

/// Only fixed stage labels and numeric metadata enter timing logs. Never errors or payloads.
pub(super) struct Timing {
    operation_id: u64,
    request_id: Option<u64>,
    session_key: u64,
    kind: &'static str,
    stage: &'static str,
    outcome: &'static str,
    started: Instant,
    stage_started: Instant,
}

impl Timing {
    pub(super) fn new(kind: &'static str, request_id: Option<u64>, session_id: &str) -> Self {
        use std::hash::{Hash, Hasher};
        let mut session = std::collections::hash_map::DefaultHasher::new();
        session_id.hash(&mut session);
        let started = Instant::now();
        Self {
            operation_id: NEXT_OPERATION.fetch_add(1, Ordering::Relaxed),
            request_id,
            session_key: session.finish(),
            kind,
            stage: "prepare",
            outcome: "dropped",
            started,
            stage_started: started,
        }
    }

    fn fields(&self, stage: &str, outcome: &str, duration_ms: u128) -> String {
        format!(
            "[CLOUD_TIMING] operation=server_history session_key={:016x} operation_id={} request_id={} kind={} stage={} outcome={} duration_ms={} total_duration_ms={}",
            self.session_key,
            self.operation_id,
            self.request_id
                .map_or_else(|| "none".into(), |id| id.to_string()),
            self.kind,
            stage,
            outcome,
            duration_ms,
            self.started.elapsed().as_millis(),
        )
    }

    pub(super) fn stage(&mut self, stage: &'static str) {
        crate::logging::info(&self.fields(
            self.stage,
            "ok",
            self.stage_started.elapsed().as_millis(),
        ));
        self.stage = stage;
        self.stage_started = Instant::now();
    }

    fn fallback<T: Default, E>(&self, result: std::result::Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(_) => {
                crate::logging::info(&self.fields(
                    "load",
                    "fallback",
                    self.started.elapsed().as_millis(),
                ));
                T::default()
            }
        }
    }

    pub(super) fn result<T, E>(&mut self, result: &std::result::Result<T, E>) {
        self.outcome = if result.is_ok() { "ok" } else { "failed" };
    }
}

impl Drop for Timing {
    fn drop(&mut self) {
        crate::logging::info(&self.fields(
            self.stage,
            self.outcome,
            self.stage_started.elapsed().as_millis(),
        ));
    }
}

/// Send authoritative todo chrome before the potentially large history frame.
pub(in crate::server) async fn send_resume_todos(
    session_id: &str,
    writer: &Arc<Mutex<WriteHalf>>,
) -> Result<()> {
    let mut timing = Timing::new("resume_todos", None, session_id);
    let todos = crate::todo::load_todos(session_id);
    let plan = crate::todo::load_plan(session_id);
    let goals = crate::todo::load_goals(session_id);
    timing.stage("encode");
    let payload = serde_json::json!({
        "todos": timing.fallback(todos),
        "plan": timing.fallback(plan),
        "goals": timing.fallback(goals),
    });
    let json = encode_event(&ServerEvent::Todos {
        session_id: session_id.to_owned(),
        payload: payload.to_string(),
    });
    timing.stage("writer_lock");
    let mut writer = writer.lock().await;
    timing.stage("write");
    let result = writer.write_all(json.as_bytes()).await;
    timing.result(&result);
    result.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resume_todos_reports_disconnected_writer() {
        let (stream, peer) = crate::transport::stream_pair().unwrap();
        let (_, write) = stream.into_split();
        drop(peer);
        let writer = Arc::new(Mutex::new(write));
        assert!(
            send_resume_todos("timing_disconnected_writer", &writer)
                .await
                .is_err()
        );
    }

    #[test]
    fn stages_keep_correlation_and_do_not_mark_cancellation_successful() {
        let mut timing = Timing::new("history", Some(9), "private-session");
        let operation_id = timing.operation_id;
        timing.stage("writer_lock");
        assert_eq!(timing.operation_id, operation_id);
        assert_eq!(timing.stage, "writer_lock");
        assert_eq!(timing.outcome, "dropped");
        let todos = Timing::new("resume_todos", None, "private-session");
        assert_eq!(timing.session_key, todos.session_key);
        assert!(
            !timing
                .fields("write", "failed", 1)
                .contains("private-session")
        );
        assert_ne!(
            timing.session_key,
            Timing::new("history", Some(9), "other").session_key
        );
    }

    #[test]
    fn timing_fields_are_correlated_numeric_and_content_free() {
        let mut timing = Timing::new("history", Some(42), "secret session");
        let fields = timing.fields("writer_lock", "ok", 7);
        assert!(
            fields
                .contains("request_id=42 kind=history stage=writer_lock outcome=ok duration_ms=7")
        );
        assert!(fields.contains("operation_id="));
        assert!(!fields.contains("session_id="));
        assert_eq!(timing.outcome, "dropped");
        timing.result(&Err::<(), _>("secret credential and content"));
        assert_eq!(timing.outcome, "failed");
        assert!(!timing.fields("write", timing.outcome, 0).contains("secret"));
        timing.result(&Ok::<_, ()>(()));
        assert_eq!(timing.outcome, "ok");
        assert_ne!(
            timing.operation_id,
            Timing::new("history", Some(42), "secret session").operation_id
        );
    }
}
