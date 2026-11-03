use super::*;
use crate::protocol::{ServerEvent, TurnIdentity};

fn envelope(event: ServerEvent) -> ServerEvent {
    ServerEvent::TurnEvent {
        identity: TurnIdentity {
            runtime_generation: "runtime-test".into(),
            turn_id: 7,
        },
        event: Box::new(event),
    }
}

#[test]
fn ndjson_preserves_wrapped_text_tool_and_error_payloads() {
    for event in [
        ServerEvent::TextDelta {
            text: "hello".into(),
        },
        ServerEvent::ToolStart {
            id: "tool-1".into(),
            name: "read".into(),
        },
        ServerEvent::Interrupted,
        ServerEvent::Error {
            id: 9,
            message: "failure".into(),
            retry_after_secs: None,
        },
    ] {
        let mut plain = Vec::new();
        let mut wrapped = Vec::new();
        let mut plain_state = NdjsonRunState::default();
        let mut wrapped_state = NdjsonRunState::default();
        emit_ndjson_event(&mut plain, &mut plain_state, event.clone()).unwrap();
        emit_ndjson_event(&mut wrapped, &mut wrapped_state, envelope(envelope(event))).unwrap();
        assert!(!plain.is_empty());
        assert_eq!(wrapped, plain);
        assert_eq!(wrapped_state.text, plain_state.text);
    }
}

#[test]
fn unwrap_preserves_terminal_request_id_and_legacy_events() {
    assert!(matches!(
        unwrap_turn_event(envelope(envelope(ServerEvent::Done { id: 42 }))),
        ServerEvent::Done { id: 42 }
    ));
    assert!(matches!(
        unwrap_turn_event(ServerEvent::Done { id: 43 }),
        ServerEvent::Done { id: 43 }
    ));
}
