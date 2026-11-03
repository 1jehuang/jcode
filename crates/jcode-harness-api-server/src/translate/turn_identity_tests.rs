use super::*;
use serde_json::json;
fn started(generation: &str, id: u64) -> Value {
    json!({"type":"turn_started","identity":{"runtime_generation":generation,"turn_id":id}})
}
fn scoped(generation: &str, id: u64, event: Value) -> Value {
    json!({"type":"turn_event","identity":{"runtime_generation":generation,"turn_id":id},"event":event})
}
fn bridge() -> BridgeState {
    BridgeState {
        session_id: Some("session".into()),
        ..Default::default()
    }
}

#[test]
fn request_rejection_does_not_settle_scoped_observer() {
    let mut state = bridge();
    state.legacy_event_to_api(&started("runtime", 1));
    state.pending_message_id = Some(99);
    let frames = state.legacy_event_to_api(
        &json!({"type":"error","id":99,"message":"Already processing a message"}),
    );
    assert!(
        frames
            .iter()
            .any(|frame| matches!(frame.event, ApiEvent::Error { .. }))
    );
    assert!(state.observed_turn_active);
    assert!(state.turn_fence.active());
    assert_eq!(state.pending_message_id, None);
    let frames = state.legacy_event_to_api(&scoped("runtime", 1, json!({"type":"done","id":1})));
    assert!(
        frames
            .iter()
            .any(|frame| matches!(frame.event, ApiEvent::TurnDone { .. }))
    );
}
#[test]
fn scoped_output_translates_and_old_completion_cannot_finish_new_turn() {
    let mut state = bridge();
    state.legacy_event_to_api(&started("runtime", 1));
    state.legacy_event_to_api(&started("runtime", 2));
    for event in [
        json!({"type":"done","id":1}),
        json!({"type":"error","id":1,"message":"old"}),
    ] {
        assert!(
            state
                .legacy_event_to_api(&scoped("runtime", 1, event))
                .is_empty()
        );
        assert!(state.observed_turn_active);
    }
    let frames = state.legacy_event_to_api(&scoped(
        "runtime",
        2,
        json!({"type":"text_delta","text":"current"}),
    ));
    assert!(
        frames
            .iter()
            .any(|f| matches!(&f.event,ApiEvent::TextDelta { text, .. } if text == "current"))
    );
    let frames = state.legacy_event_to_api(&scoped("runtime", 2, json!({"type":"done","id":999})));
    assert!(
        frames
            .iter()
            .any(|f| matches!(f.event, ApiEvent::TurnDone { .. }))
    );
    assert!(!state.observed_turn_active);
}
#[test]
fn runtime_bootstrap_rejects_retired_start_and_completion() {
    let mut state = bridge();
    state.legacy_event_to_api(&started("old", 100));
    state.legacy_event_to_api(&json!({"type":"turn_state","session_id":"session","runtime_generation":"new","latest_turn_id":1,"turn_id":1}));
    state.legacy_event_to_api(&started("old", 101));
    assert!(
        state
            .legacy_event_to_api(&scoped("old", 100, json!({"type":"done","id":1})))
            .is_empty()
    );
    assert!(state.observed_turn_active);
    let frames = state.legacy_event_to_api(&scoped("new", 1, json!({"type":"done","id":1})));
    assert!(
        frames
            .iter()
            .any(|f| matches!(f.event, ApiEvent::TurnDone { .. }))
    );
}
#[test]
fn same_turn_bootstrap_is_idempotent_and_legacy_bridge_remains_compatible() {
    let mut state = bridge();
    for _ in 0..2 {
        state.legacy_event_to_api(&json!({"type":"turn_state","session_id":"session","runtime_generation":"runtime","latest_turn_id":7,"turn_id":7}));
    }
    assert!(state.observed_turn_active);
    assert!(
        state
            .legacy_event_to_api(&scoped("runtime", 7, json!({"type":"done","id":42})))
            .iter()
            .any(|f| matches!(f.event, ApiEvent::TurnDone { .. }))
    );
    let mut legacy = bridge();
    legacy.legacy_event_to_api(&json!({"type":"text_delta","text":"legacy"}));
    assert!(
        legacy
            .legacy_event_to_api(&json!({"type":"done","id":42}))
            .iter()
            .any(|f| matches!(f.event, ApiEvent::TurnDone { .. }))
    );
}
