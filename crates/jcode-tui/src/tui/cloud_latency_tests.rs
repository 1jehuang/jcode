use super::*;

#[test]
fn cloud_latency_pending_requests_are_bounded_and_responses_remove_them() {
    REQUESTS.with_borrow_mut(VecDeque::clear);
    for id in 0..100 {
        request_sent(&Request::GetHistory { id });
    }
    REQUESTS.with_borrow(|pending| {
        assert_eq!(pending.len(), MAX_PENDING);
        assert_eq!(pending.front().map(|entry| entry.0), Some(36));
    });
    let event = parse_event(r#"{"type":"compacted_history","id":99,"session_id":"fixture","messages":[],"images":[],"compacted_total":0,"compacted_visible":0,"compacted_remaining":0,"compacted_hidden_prompts":0}"#).unwrap();
    assert_eq!(event_key(&event), Some(("history_page", Some(99))));
    REQUESTS.with_borrow(|pending| assert!(!pending.iter().any(|entry| entry.0 == 99)));
    REQUESTS.with_borrow_mut(VecDeque::clear);
}

#[test]
fn cloud_latency_todos_preserve_attach_correlation_and_payload() {
    REQUESTS.with_borrow_mut(|pending| {
        pending.clear();
        pending.push_back((42, "subscribe", Instant::now()));
    });
    let event = parse_event(r#"{"type":"todos","session_id":"fixture","payload":"private transcript must not be logged"}"#).unwrap();
    assert_eq!(event_key(&event), Some(("todos", None)));
    REQUESTS.with_borrow(|pending| assert_eq!(pending.len(), 1));
    assert!(
        matches!(event, ServerEvent::Todos { payload, .. } if payload == "private transcript must not be logged")
    );
    REQUESTS.with_borrow_mut(VecDeque::clear);
    assert!(parse_event("not json").is_err());
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
    rendered();
}
