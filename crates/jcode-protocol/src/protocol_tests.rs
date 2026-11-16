use super::*;

/// A transport prototype only. Real clients negotiate turn identity separately,
/// so a shared encoding cannot replace per-client encoding without grouping by
/// transport capabilities first.
#[test]
#[ignore = "manual multi-client encoding cost probe"]
fn multi_client_shared_encoding_cost_probe() {
    use std::{hint::black_box, sync::Arc, time::Instant};

    for bytes in [24, 65_536] {
        for recipients in [1, 4] {
            let event = ServerEvent::TurnEvent {
                identity: TurnIdentity {
                    runtime_generation: "generation".into(),
                    turn_id: 1,
                },
                event: Box::new(ServerEvent::TextDelta {
                    text: "x".repeat(bytes),
                }),
            };
            let expected = encode_event(&event).into_bytes();
            let mut legacy = Vec::new();
            let mut shared = Vec::new();
            for round in 0..8 {
                for prototype in if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let mut buffers = vec![Vec::new(); recipients];
                    let started = Instant::now();
                    for _ in 0..1_000 {
                        if prototype {
                            // Models encoding once and queueing immutable bytes.
                            // Includes the shared allocation even for one client.
                            let mut buffer = Vec::new();
                            encode_event_into(black_box(&event), &mut buffer);
                            let payload: Arc<[u8]> = buffer.into();
                            for _ in 0..recipients {
                                black_box(Arc::clone(&payload));
                            }
                            black_box(&*payload);
                        } else {
                            for (index, buffer) in buffers.iter_mut().enumerate() {
                                if index + 1 == recipients {
                                    // Production moves the original to the final recipient.
                                    encode_event_into(black_box(&event), buffer);
                                } else {
                                    let owned = black_box(&event).clone();
                                    encode_event_into(&owned, buffer);
                                }
                                black_box(&*buffer);
                            }
                        }
                    }
                    let ns = started.elapsed().as_nanos() / 1_000;
                    if prototype {
                        let mut buffer = Vec::new();
                        encode_event_into(&event, &mut buffer);
                        let payload: Arc<[u8]> = buffer.into();
                        assert_eq!(&*payload, expected.as_slice());
                        shared.push(ns);
                    } else {
                        for buffer in &buffers {
                            assert_eq!(buffer, &expected);
                        }
                        legacy.push(ns);
                    }
                }
            }
            legacy.sort_unstable();
            shared.sort_unstable();
            eprintln!(
                "sharing prototype bytes={bytes} recipients={recipients}: owned+encode={} ns/event, shared-encode={} ns/event (upper median, test profile)",
                legacy[4], shared[4]
            );
        }
    }
}

#[test]
fn reusable_event_encoding_matches_legacy_bytes_and_reuses_storage() {
    let mut buffer = Vec::new();
    for text in ["", "plain text", "界 👩‍💻", "\"quoted\"\n\t\\\u{0000}"] {
        let event = ServerEvent::TextDelta { text: text.into() };
        encode_event_into(&event, &mut buffer);
        assert_eq!(buffer, encode_event(&event).as_bytes());
    }
    let event = ServerEvent::TextDelta {
        text: "fixed streaming chunk 界".into(),
    };
    encode_event_into(&event, &mut buffer);
    let pointer = buffer.as_ptr();
    let capacity = buffer.capacity();
    for _ in 0..1000 {
        encode_event_into(&event, &mut buffer);
        assert_eq!(buffer.as_ptr(), pointer);
        assert_eq!(buffer.capacity(), capacity);
        assert_eq!(buffer, encode_event(&event).as_bytes());
    }
    for payload in [
        serde_json::json!({"type":"interrupted"}),
        serde_json::json!({"type":"todos", "session_id":"session", "payload":"[]"}),
    ] {
        let event: ServerEvent = serde_json::from_value(payload).unwrap();
        encode_event_into(&event, &mut buffer);
        assert_eq!(buffer, encode_event(&event).as_bytes());
    }
}

#[test]
fn reusable_event_encoding_does_not_retain_large_snapshots_on_next_call() {
    let mut buffer = Vec::new();
    let large = ServerEvent::TextDelta {
        text: "界".repeat(100_000),
    };
    encode_event_into(&large, &mut buffer);
    assert_eq!(buffer, encode_event(&large).as_bytes());
    assert!(buffer.capacity() > 64 * 1024);
    let small = ServerEvent::TextDelta {
        text: "small".into(),
    };
    encode_event_into(&small, &mut buffer);
    assert_eq!(buffer, encode_event(&small).as_bytes());
    assert!(buffer.capacity() <= 64 * 1024);
}

#[test]
fn todos_request_and_snapshot_roundtrip() {
    let request: Request = serde_json::from_str(r#"{"type":"get_todos","id":41}"#).unwrap();
    assert!(matches!(request, Request::GetTodos { id: 41 }));
    assert_eq!(request.id(), 41);
    // A todo snapshot needs an attached session, unlike one-shot control requests.
    assert!(!request.is_lightweight_control_request());
    let payload = serde_json::json!({"todos": [], "plan": {}, "goals": []}).to_string();
    let event = ServerEvent::Todos {
        session_id: "attached-session".into(),
        payload: payload.clone(),
        pushes: false,
    };
    let encoded = serde_json::to_value(&event).unwrap();
    assert_eq!(encoded["type"], "todos");
    // Older clients and daemons never see the field unless it is set.
    assert!(encoded.get("pushes").is_none());
    let pushing = serde_json::to_value(ServerEvent::Todos {
        session_id: "attached-session".into(),
        payload: payload.clone(),
        pushes: true,
    })
    .unwrap();
    assert_eq!(pushing["pushes"], true);
    assert!(matches!(
        serde_json::from_value::<ServerEvent>(pushing).unwrap(),
        ServerEvent::Todos { pushes: true, .. }
    ));
    let decoded: ServerEvent = serde_json::from_value(encoded).unwrap();
    assert!(
        matches!(decoded, ServerEvent::Todos { session_id, payload: actual, pushes: false }
        if session_id == "attached-session" && actual == payload)
    );
}
use anyhow::{Result, anyhow};

fn parse_request_json(json: &str) -> Result<Request> {
    serde_json::from_str(json).map_err(Into::into)
}

fn parse_event_json(json: &str) -> Result<ServerEvent> {
    serde_json::from_str(json).map_err(Into::into)
}

include!("protocol_tests/core_events.rs");
include!("protocol_tests/comm_requests.rs");
include!("protocol_tests/comm_responses.rs");
include!("protocol_tests/comm_format_awaited.rs");
include!("protocol_tests/misc_events.rs");
include!("protocol_tests/randomized.rs");
include!("protocol_tests/model_usage.rs");
#[path = "protocol_tests/turn_identity.rs"]
mod turn_identity;
#[test]
fn swarm_runtime_wait_metadata_is_optional_and_nonempty() {
    let empty: super::SwarmMemberRuntime = serde_json::from_str("{}").unwrap();
    assert!(empty.is_empty());
    assert_eq!(serde_json::to_value(&empty).unwrap(), serde_json::json!({}));
    let waiting = super::SwarmMemberRuntime {
        wait_reason: Some("Waiting for background task".into()),
        wait_age_secs: Some(0),
        ..Default::default()
    };
    assert!(!waiting.is_empty());
    let encoded = serde_json::to_value(&waiting).unwrap();
    assert_eq!(encoded["wait_age_secs"], 0);
    assert_eq!(
        serde_json::from_value::<super::SwarmMemberRuntime>(encoded).unwrap(),
        waiting
    );
    assert!(
        !super::SwarmMemberRuntime {
            wait_age_secs: Some(1),
            ..Default::default()
        }
        .is_empty()
    );
}

#[test]
fn swarm_live_status_defaults_and_roundtrips() {
    let legacy: super::SwarmMemberRuntime =
        serde_json::from_str(r#"{"context_tokens":4200,"output_tokens":99}"#).unwrap();
    assert!(legacy.live_status.is_none());
    assert!(legacy.todo_blocked_reason.is_none());
    let partial: super::SwarmLiveStatus =
        serde_json::from_str(r#"{"phase":"streaming","future_field":true}"#).unwrap();
    assert_eq!(partial.input_tokens, None);
    assert_eq!(partial.connection_type, None);
    let runtime = super::SwarmMemberRuntime {
        todo_blocked_reason: Some("Required data is permanently unavailable".into()),
        live_status: Some(super::SwarmLiveStatus {
            phase: "streaming".into(),
            input_tokens: Some(0),
            output_tokens: Some(17),
            output_tps_milli: Some(12500),
            connection_type: Some("websocket".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(!runtime.is_empty());
    assert_eq!(
        serde_json::from_value::<super::SwarmMemberRuntime>(
            serde_json::to_value(&runtime).unwrap()
        )
        .unwrap(),
        runtime
    );
}

#[test]
fn sdk_login_notify_is_accepted_without_subscribe() {
    // The SDK login flow sends exactly this frame on a fresh connection.
    // Requiring Subscribe first dropped it, so Desktop never saw new models.
    let request: Request =
        serde_json::from_str(r#"{"type":"notify_auth_changed","id":1,"provider":"claude"}"#)
            .unwrap();
    assert!(request.is_lightweight_control_request());
}

#[test]
fn client_focus_report_roundtrip() {
    let request: Request =
        serde_json::from_str(r#"{"type":"client_focus","id":7,"focused":false}"#).unwrap();
    assert!(matches!(
        request,
        Request::ClientFocus {
            id: 7,
            focused: false,
            hidden: false
        }
    ));
    assert_eq!(request.id(), 7);
    // It belongs to an attached connection, so it is not a one-shot control.
    assert!(!request.is_lightweight_control_request());
    let encoded = serde_json::to_value(Request::ClientFocus {
        id: 8,
        focused: true,
        hidden: false,
    })
    .unwrap();
    assert_eq!(encoded["type"], "client_focus");
    assert_eq!(encoded["focused"], true);
    // Left off the wire unless set, so the report is unchanged for a server
    // that predates the field.
    assert!(encoded.get("hidden").is_none());
    let hidden = Request::ClientFocus {
        id: 9,
        focused: false,
        hidden: true,
    };
    let encoded = serde_json::to_string(&hidden).unwrap();
    assert!(matches!(
        serde_json::from_str::<Request>(&encoded).unwrap(),
        Request::ClientFocus {
            id: 9,
            focused: false,
            hidden: true
        }
    ));
}
