use super::*;

fn state_with_session() -> BridgeState {
    BridgeState {
        session_id: Some("s1".into()),
        ..Default::default()
    }
}

#[test]
fn create_session_maps_to_subscribe() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 1}));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(value["type"], "subscribe");
    assert!(value["working_dir"].is_string());
}

#[test]
fn state_event_answers_pending_attach() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 5}));
    assert_eq!(out.len(), 2, "subscribe + state chase");
    let Outbound::Legacy(state_req) = &out[1] else {
        panic!("expected legacy state request");
    };
    assert_eq!(state_req["type"], "state");
    let state_id = state_req["id"].as_u64().unwrap();

    // A subscribe `done` must not leak a turn_done.
    let done = state.legacy_event_to_api(&json!({"type": "done", "id": 1}));
    assert!(done.is_empty());

    let frames = state.legacy_event_to_api(&json!({
        "type": "state", "id": state_id, "session_id": "abc",
        "message_count": 0, "is_processing": false,
    }));
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].reply_to, Some(5));
    match &frames[0].event {
        ApiEvent::Attached { session } => assert_eq!(session.session_id, "abc"),
        other => panic!("unexpected: {other:?}"),
    }
    assert_eq!(state.session_id.as_deref(), Some("abc"));
}

#[test]
fn send_message_then_done_becomes_turn_done() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(
        &json!({"req": "send_message", "id": 2, "session_id": "s1", "content": "hi"}),
    );
    let Outbound::Legacy(message) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(message["type"], "message");
    let legacy_id = message["id"].as_u64().unwrap();

    let deltas = state.legacy_event_to_api(&json!({"type": "text_delta", "text": "yo"}));
    assert!(matches!(
        &deltas[0].event,
        ApiEvent::TextDelta { session_id, text } if session_id == "s1" && text == "yo"
    ));

    let done = state.legacy_event_to_api(&json!({"type": "done", "id": legacy_id}));
    assert!(matches!(
        &done[0].event,
        ApiEvent::TurnDone { session_id } if session_id == "s1"
    ));
}

#[test]
fn ping_pong_roundtrip() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "ping", "id": 9}));
    let Outbound::Legacy(ping) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = ping["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({"type": "pong", "id": legacy_id}));
    assert_eq!(frames[0].reply_to, Some(9));
    assert!(matches!(frames[0].event, ApiEvent::Pong));
}

#[test]
fn history_reply_is_mapped() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "get_history", "id": 4}));
    let Outbound::Legacy(get) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = get["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({
        "type": "history",
        "id": legacy_id,
        "session_id": "s1",
        "messages": [{"role": "user", "content": "hi"}],
    }));
    match &frames[0].event {
        ApiEvent::History { messages, .. } => {
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].role, "user");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn unknown_legacy_events_are_dropped() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({"type": "swarm_event", "data": {}}));
    assert!(frames.is_empty());
}

#[test]
fn unknown_api_request_gets_error_reply() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "frobnicate", "id": 3}));
    let Outbound::Reply(frame) = &out[0] else {
        panic!("expected direct reply");
    };
    assert_eq!(frame.reply_to, Some(3));
    assert!(matches!(
        frame.event,
        ApiEvent::Error {
            code: ErrorCode::UnknownRequest,
            ..
        }
    ));
}

#[test]
fn error_routes_to_pending_request() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "clear", "id": 7}));
    let Outbound::Legacy(clear) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = clear["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(
        &json!({"type": "error", "id": legacy_id, "message": "nope"}),
    );
    assert_eq!(frames[0].reply_to, Some(7));
}
