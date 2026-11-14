use super::{ConnectionMetadataUpdate, ServerEvent};
use serde_json::json;

#[test]
fn writer_buffer_retains_64kib_and_releases_larger_capacity() {
    for capacity in [0, 1024, 64 * 1024, 64 * 1024 + 1, 128 * 1024] {
        let mut buffer = Vec::with_capacity(capacity);
        buffer.extend_from_slice(b"written");
        let actual_capacity = buffer.capacity();
        let pointer = buffer.as_ptr();
        super::release_large_event_buffer(&mut buffer);
        if actual_capacity > 64 * 1024 {
            assert_eq!(buffer.capacity(), 0);
            assert!(buffer.is_empty());
        } else {
            assert_eq!(buffer.capacity(), actual_capacity);
            assert_eq!(buffer.as_ptr(), pointer);
            assert_eq!(buffer, b"written");
        }
    }
}

#[tokio::test]
async fn writer_reused_buffer_preserves_sequential_wire_bytes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut events = fixtures();
    events.push(ServerEvent::TextDelta {
        text: "large".repeat(20_000),
    });
    events.push(wrapped(ServerEvent::TextDelta {
        text: "after large\n☃".into(),
    }));
    events.push(ServerEvent::Done { id: 99 });
    let expected: String = events.iter().map(crate::protocol::encode_event).collect();
    // A small transport forces partial writes, exercising write_all with the
    // same borrowed buffer until the entire event has been delivered.
    let (mut writer, mut reader) = tokio::io::duplex(17);
    let sender = tokio::spawn(async move {
        let mut buffer = Vec::new();
        for event in events {
            crate::protocol::encode_event_into(&event, &mut buffer);
            writer.write_all(&buffer).await.unwrap();
            super::release_large_event_buffer(&mut buffer);
            assert!(buffer.capacity() <= 64 * 1024);
        }
    });
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual).await.unwrap();
    sender.await.unwrap();
    assert_eq!(actual, expected.as_bytes());
}

fn fixtures() -> Vec<ServerEvent> {
    [
        json!({"type":"turn_started", "identity":{"runtime_generation":"generation", "turn_id":1}}),
        json!({"type":"tool_start", "id":"a", "name":"bash"}),
        json!({"type":"tool_start", "id":"b", "name":""}),
        json!({"type":"tool_done", "id":"unrelated", "name":"other", "output":"", "error":null}),
        json!({"type":"done", "id":1}),
        json!({"type":"error", "id":1, "message":"failure"}),
        json!({"type":"interrupted"}),
        json!({"type":"text_delta", "text":"hello"}),
        json!({"type":"text_done"}),
        json!({"type":"reasoning_delta", "text":"thinking"}),
        json!({"type":"reasoning_done"}),
        json!({"type":"tool_exec", "id":"a", "name":"bash"}),
        json!({"type":"tool_input", "id":"a", "delta":"{}"}),
        json!({"type":"message_end"}),
        json!({"type":"ack", "id":1}),
        json!({"type":"retry_rollback", "attempt":1, "max":3}),
    ]
    .into_iter()
    .map(|value| serde_json::from_value(value).unwrap())
    .collect()
}

fn wrapped(event: ServerEvent) -> ServerEvent {
    serde_json::from_value(json!({
        "type":"turn_event",
        "identity":{"runtime_generation":"generation", "turn_id":1},
        "event":event,
    }))
    .unwrap()
}

// Independent copy of the original forwarder transitions, including its single
// wrapper unwrap and unconditional ToolDone clear regardless of tool identity.
fn old_transition(event: &ServerEvent, state: &mut (bool, Option<String>)) {
    let observed = match event {
        ServerEvent::TurnEvent { event, .. } => event.as_ref(),
        event => event,
    };
    match observed {
        ServerEvent::TurnStarted { .. } => {
            state.0 = true;
            state.1 = None;
        }
        ServerEvent::ToolStart { name, .. } => {
            state.0 = true;
            state.1 = Some(name.clone());
        }
        ServerEvent::ToolDone { .. } => state.1 = None,
        ServerEvent::Done { .. } | ServerEvent::Error { .. } | ServerEvent::Interrupted => {
            state.0 = false;
            state.1 = None;
        }
        _ => {}
    }
}

fn new_transition(event: &ServerEvent, state: &mut (bool, Option<String>)) {
    if let Some(update) = ConnectionMetadataUpdate::from_event(event) {
        update.apply(&mut state.0, &mut state.1);
    }
}

#[test]
fn metadata_transition_equivalence_all_states_wrappers_and_event_pairs() {
    let mut events = Vec::new();
    for event in fixtures() {
        events.push(event.clone());
        events.push(wrapped(event.clone()));
        events.push(wrapped(wrapped(event)));
    }
    // Exhaust all processing/tool-presence combinations, including inconsistent
    // states. Names include empty and nonempty values to catch accidental guards.
    for processing in [false, true] {
        for tool in [None, Some(String::new()), Some("prior-tool".to_owned())] {
            for first in &events {
                for second in &events {
                    let mut old = (processing, tool.clone());
                    let mut new = old.clone();
                    for event in [first, second] {
                        old_transition(event, &mut old);
                        new_transition(event, &mut new);
                        assert_eq!(old, new, "event: {event:?}, first: {first:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn metadata_lock_eligibility_exact_transition_set_and_single_wrapper() {
    for (index, event) in fixtures().into_iter().enumerate() {
        let relevant = index < 7;
        assert_eq!(
            ConnectionMetadataUpdate::from_event(&event).is_some(),
            relevant
        );
        let event = wrapped(event);
        assert_eq!(
            ConnectionMetadataUpdate::from_event(&event).is_some(),
            relevant
        );
        assert!(ConnectionMetadataUpdate::from_event(&wrapped(event)).is_none());
    }
}

#[test]
fn metadata_lock_eligibility_for_1000_text_events_is_zero() {
    for wrap in [false, true] {
        let events: Vec<_> = (0..1000)
            .map(|index| {
                let event = ServerEvent::TextDelta {
                    text: index.to_string(),
                };
                if wrap { wrapped(event) } else { event }
            })
            .collect();
        let old_lock_count = events.len();
        let new_lock_count = events
            .iter()
            .filter(|event| ConnectionMetadataUpdate::from_event(event).is_some())
            .count();
        assert_eq!(old_lock_count, 1000);
        assert_eq!(new_lock_count, 0);
    }
}
