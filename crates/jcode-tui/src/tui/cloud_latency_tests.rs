use super::*;

fn subscribe(id: u64, target: Option<&str>) -> Request {
    serde_json::from_value(serde_json::json!({
        "type": "subscribe", "id": id, "target_session_id": target,
    }))
    .unwrap()
}

fn todo_event() -> ServerEvent {
    ServerEvent::Todos {
        session_id: "fixture".into(),
        payload: "private transcript must not be logged".into(),
    }
}

#[test]
fn cloud_latency_pending_requests_are_bounded_and_responses_remove_them() {
    let timings = RequestTimings::default();
    for id in 0..100 {
        timings.request_sent(&Request::GetHistory { id });
    }
    {
        let pending = timings.pending.lock().unwrap();
        assert_eq!(pending.len(), MAX_PENDING);
        assert_eq!(pending.front().map(|entry| entry.id), Some(36));
    }
    let event = timings.parse_event(r#"{"type":"compacted_history","id":99,"session_id":"fixture","messages":[],"images":[],"compacted_total":0,"compacted_visible":0,"compacted_remaining":0,"compacted_hidden_prompts":0}"#).unwrap();
    assert_eq!(event_key(&event), Some(("history_page", Some(99))));
    assert!(
        !timings
            .pending
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry.id == 99)
    );
}

#[test]
fn cloud_latency_todos_preserve_attach_correlation_and_payload() {
    let timings = RequestTimings::default();
    timings.request_sent(&subscribe(42, Some("fixture")));
    let event = timings.parse_event(r#"{"type":"todos","session_id":"fixture","payload":"private transcript must not be logged"}"#).unwrap();
    assert_eq!(event_key(&event), Some(("todos", None)));
    assert_eq!(timings.pending.lock().unwrap().len(), 1);
    assert!(
        matches!(event, ServerEvent::Todos { payload, .. } if payload == "private transcript must not be logged")
    );
    // Repeated unsolicited snapshots are not additional bootstrap responses.
    assert!(timings.response_wait(&todo_event()).is_none());
    assert!(timings.parse_event("not json").is_err());
}

#[test]
fn cloud_latency_polls_take_priority_over_attach_and_unsolicited_todos() {
    let timings = RequestTimings::default();
    timings.request_sent(&subscribe(42, Some("fixture")));
    timings.request_sent(&Request::GetTodos { id: 43 });
    assert_eq!(
        timings.response_wait(&todo_event()).map(|entry| entry.1),
        Some(43)
    );
    assert_eq!(
        timings.response_wait(&todo_event()).map(|entry| entry.1),
        Some(42)
    );
    assert!(timings.response_wait(&todo_event()).is_none());
    let pending = timings.pending.lock().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, 42);
}

#[test]
fn cloud_latency_fresh_subscribe_done_does_not_expire_waiting_for_history() {
    let timings = RequestTimings::default();
    timings.request_sent(&subscribe(10, None));
    timings.request_sent(&Request::GetHistory { id: 11 });
    let event = timings.parse_event(r#"{"type":"done","id":10}"#).unwrap();
    assert!(matches!(event, ServerEvent::Done { id: 10 }));
    let pending = timings.pending.lock().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, 11);
}

#[test]
fn cloud_latency_target_subscribe_done_retains_history_and_error_clears_it() {
    let timings = RequestTimings::default();
    timings.request_sent(&subscribe(10, Some("fixture")));
    assert!(
        timings
            .response_wait(&ServerEvent::Done { id: 10 })
            .is_none()
    );
    assert_eq!(timings.pending.lock().unwrap().len(), 1);
    let event = timings
        .parse_event(r#"{"type":"error","id":10,"message":"PRIVATE"}"#)
        .unwrap();
    assert!(matches!(event, ServerEvent::Error { id: 10, .. }));
    assert!(timings.pending.lock().unwrap().is_empty());
}

#[test]
fn cloud_latency_correlation_survives_thread_migration_and_isolates_connections() {
    let first = std::sync::Arc::new(RequestTimings::default());
    let second = RequestTimings::default();
    let sender = first.clone();
    std::thread::spawn(move || sender.request_sent(&Request::GetTodos { id: 1 }))
        .join()
        .unwrap();
    assert!(second.response_wait(&todo_event()).is_none());
    second.request_sent(&Request::GetTodos { id: 1 });
    let receiver = first.clone();
    let response = std::thread::spawn(move || receiver.response_wait(&todo_event()))
        .join()
        .unwrap();
    assert_eq!(response.map(|entry| entry.1), Some(1));
    assert!(first.pending.lock().unwrap().is_empty());
    assert_eq!(second.pending.lock().unwrap().len(), 1);
    assert_eq!(
        second.response_wait(&todo_event()).map(|entry| entry.1),
        Some(1)
    );
}

#[test]
fn cloud_latency_expiry_remains_bounded_and_reused_ids_replace_old_requests() {
    let timings = RequestTimings::default();
    timings.request_sent(&Request::GetTodos { id: 1 });
    timings.pending.lock().unwrap()[0].started =
        Instant::now() - std::time::Duration::from_secs(121);
    timings.request_sent(&Request::GetHistory { id: 2 });
    timings.request_sent(&Request::GetTodos { id: 2 });
    let pending = timings.pending.lock().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, 2);
    assert_eq!(pending[0].operation, "todos");
}

#[test]
fn cloud_latency_paint_milestone_is_consumed_once() {
    PAINT.with_borrow_mut(|pending| *pending = None);
    applied("history", Instant::now(), false);
    PAINT.with_borrow(|pending| assert!(pending.is_none()));
    applied("todos", Instant::now(), true);
    applied("history", Instant::now(), true);
    PAINT.with_borrow(|pending| assert_eq!(pending.as_ref().map(|entry| entry.0), Some("todos")));
    rendered();
    PAINT.with_borrow(|pending| assert!(pending.is_none()));
}
