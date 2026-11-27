use super::*;

#[test]
fn scoped_terminal_roundtrips_without_changing_legacy_done_shape() {
    let event = ServerEvent::TurnEvent {
        identity: crate::TurnIdentity {
            runtime_generation: "runtime-uuid".into(),
            turn_id: 7,
        },
        event: Box::new(ServerEvent::Done { id: 42 }),
    };
    let wire = encode_event(&event);
    let decoded: ServerEvent = serde_json::from_str(&wire).unwrap();
    let ServerEvent::TurnEvent { identity, event } = decoded else {
        panic!("missing scope");
    };
    assert_eq!(identity.runtime_generation, "runtime-uuid");
    assert_eq!(identity.turn_id, 7);
    assert!(matches!(*event, ServerEvent::Done { id: 42 }));
    let legacy: ServerEvent = serde_json::from_str(r#"{"type":"done","id":42}"#).unwrap();
    assert!(matches!(legacy, ServerEvent::Done { id: 42 }));
    assert_eq!(
        serde_json::to_value(&legacy).unwrap(),
        serde_json::json!({"type":"done","id":42})
    );
}

#[test]
fn old_subscribe_defaults_to_legacy_transport() {
    let request: Request = serde_json::from_str(r#"{"type":"subscribe","id":1}"#).unwrap();
    assert!(matches!(
        request,
        Request::Subscribe {
            supports_turn_identity: false,
            ..
        }
    ));
    let request: Request =
        serde_json::from_str(r#"{"type":"subscribe","id":1,"supports_turn_identity":true}"#)
            .unwrap();
    assert!(matches!(
        request,
        Request::Subscribe {
            supports_turn_identity: true,
            ..
        }
    ));
}

#[test]
fn bootstrap_roundtrip_carries_active_identity_and_idle_watermark() {
    for turn_id in [None, Some(7)] {
        let event = ServerEvent::TurnState {
            session_id: "session".into(),
            runtime_generation: "runtime".into(),
            turn_id,
            latest_turn_id: 7,
        };
        let decoded: ServerEvent = serde_json::from_str(&encode_event(&event)).unwrap();
        assert!(
            matches!(decoded, ServerEvent::TurnState { latest_turn_id: 7, turn_id: active, .. } if active == turn_id)
        );
    }
}
