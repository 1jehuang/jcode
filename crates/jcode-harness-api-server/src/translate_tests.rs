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
    assert_eq!(
        out.len(),
        3,
        "subscribe + state chase + model catalog probe"
    );
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

/// The daemon acking the in-flight message is the only signal that the agent
/// took delivery, so it must surface as its own event rather than being
/// swallowed as a bookkeeping ack. A client that shows "sent" until the first
/// token of the reply is showing a lie for as long as the model thinks.
#[test]
fn acking_the_pending_message_reports_acceptance() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(
        &json!({"req": "send_message", "id": 2, "session_id": "s1", "content": "hi"}),
    );
    let Outbound::Legacy(message) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = message["id"].as_u64().unwrap();

    let accepted = state.legacy_event_to_api(&json!({"type": "ack", "id": legacy_id}));
    assert!(matches!(
        &accepted[0].event,
        ApiEvent::MessageAccepted { session_id } if session_id == "s1"
    ));
    // The turn must still end normally: the acceptance event must not consume
    // the pending id the `done` boundary depends on.
    let done = state.legacy_event_to_api(&json!({"type": "done", "id": legacy_id}));
    assert!(matches!(&done[0].event, ApiEvent::TurnDone { .. }));
}

/// An ack for anything else (a ping, a clear) is still a plain request reply:
/// promoting those to acceptance would wiggle a message that nobody sent.
#[test]
fn acking_an_unrelated_request_stays_a_reply() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({"req": "clear", "id": 9, "session_id": "s1"}));
    let Outbound::Legacy(clear) = &out[0] else {
        panic!("expected legacy outbound");
    };
    let legacy_id = clear["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({"type": "ack", "id": legacy_id}));
    assert_eq!(frames[0].reply_to, Some(9));
    assert!(matches!(&frames[0].event, ApiEvent::Ok));
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
    let frames =
        state.legacy_event_to_api(&json!({"type": "error", "id": legacy_id, "message": "nope"}));
    assert_eq!(frames[0].reply_to, Some(7));
}

/// Attaching must volunteer the model identity: a client that has to know to
/// ask would show "unknown model" forever, which is what this fixes.
#[test]
fn attaching_probes_and_reports_the_model() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 7}));
    let Outbound::Legacy(catalog) = &out[2] else {
        panic!("expected a legacy catalog probe");
    };
    assert_eq!(catalog["type"], "get_model_catalog");
    let catalog_id = catalog["id"].as_u64().unwrap();

    // The daemon answers the probe with a `history`-shaped reply carrying no
    // messages. That must become an unsolicited model_info event, not a reply
    // to some client request that never asked for history.
    let frames = state.legacy_event_to_api(&json!({
        "type": "history", "id": catalog_id, "messages": [],
        "provider_name": "anthropic", "provider_model": "claude-sonnet-4-5",
    }));
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].reply_to, None,
        "the probe was not client-initiated"
    );
    match &frames[0].event {
        ApiEvent::ModelInfo {
            provider, model, ..
        } => {
            assert_eq!(provider.as_deref(), Some("anthropic"));
            assert_eq!(model.as_deref(), Some("claude-sonnet-4-5"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// A real `get_history` reply must still be a history reply after the probe has
/// been consumed, or the probe would swallow the client's own request.
#[test]
fn a_client_history_request_is_untouched_by_the_probe() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 1}));
    let Outbound::Legacy(catalog) = &out[2] else {
        panic!("expected a catalog probe");
    };
    let catalog_id = catalog["id"].as_u64().unwrap();
    state.legacy_event_to_api(&json!({"type": "history", "id": catalog_id, "messages": []}));

    let out = state.api_request_to_legacy(&json!({"req": "get_history", "id": 9}));
    let Outbound::Legacy(request) = &out[0] else {
        panic!("expected a legacy history request");
    };
    let history_id = request["id"].as_u64().unwrap();
    let frames = state.legacy_event_to_api(&json!({
        "type": "history", "id": history_id,
        "messages": [{"role": "user", "content": "hi"}],
    }));
    assert_eq!(frames[0].reply_to, Some(9));
    assert!(matches!(frames[0].event, ApiEvent::History { .. }));
}

/// Switching model mid-session must reach the client, or the caption goes stale
/// and confidently lies about which model answered.
#[test]
fn a_model_change_is_forwarded() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "model_changed", "id": 3,
        "model": "gpt-5.6", "provider_name": "openai",
    }));
    match &frames[0].event {
        ApiEvent::ModelInfo {
            provider, model, ..
        } => {
            assert_eq!(provider.as_deref(), Some("openai"));
            assert_eq!(model.as_deref(), Some("gpt-5.6"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// A failed model change must not be reported as the active model.
#[test]
fn a_failed_model_change_is_not_reported() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "model_changed", "id": 3, "model": "nope", "error": "no such model",
    }));
    assert!(frames.is_empty());
}

/// An auth change re-resolves the route, so the push must update the caption.
#[test]
fn an_available_models_push_updates_the_model() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(&json!({
        "type": "available_models_updated",
        "provider_name": "anthropic", "provider_model": "claude-opus-4-5",
        "available_models": ["claude-opus-4-5"],
    }));
    match &frames[0].event {
        ApiEvent::ModelInfo {
            session_id, model, ..
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(model.as_deref(), Some("claude-opus-4-5"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn create_session_in_a_jcode_checkout_requests_selfdev() {
    // Regression: desktop2 opens its own crate, and without the `selfdev`
    // flag the daemon hands back an agent with no self-dev tools or prompt.
    let mut state = BridgeState::default();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .join("crates/jcode-desktop2");
    let out = state.api_request_to_legacy(&json!({
        "req": "create_session",
        "id": 1,
        "working_dir": repo.display().to_string(),
    }));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(value["selfdev"], json!(true));
}

#[test]
fn create_session_outside_a_checkout_leaves_selfdev_unset() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({
        "req": "create_session",
        "id": 1,
        "working_dir": "/",
    }));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert!(value.get("selfdev").is_none(), "got {value}");
}

/// A turn that fails ends with `error` instead of `done`. The bridge must let
/// go of the pending message, or a later unrelated `done` reusing that legacy
/// id would be reported to the client as this turn finally finishing, and a
/// client that trusts `turn_done` would unblock on a turn that never ran.
#[test]
fn a_failed_turn_clears_the_pending_message() {
    let mut state = state_with_session();
    let out = state.api_request_to_legacy(&json!({
        "req": "send_message", "id": 11, "content": "hi",
    }));
    let Outbound::Legacy(message) = &out[0] else {
        panic!("expected a legacy message");
    };
    let legacy_id = message["id"].as_u64().expect("a legacy id");

    let frames = state.legacy_event_to_api(&json!({
        "type": "error", "id": legacy_id, "message": "dns error",
    }));
    assert!(
        frames
            .iter()
            .any(|frame| matches!(frame.event, ApiEvent::Error { .. })),
        "the failure was not forwarded"
    );

    // The same id arriving as `done` afterwards is no longer this turn.
    let frames = state.legacy_event_to_api(&json!({"type": "done", "id": legacy_id}));
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame.event, ApiEvent::TurnDone { .. })),
        "a failed turn reported a second, phantom completion"
    );
}
