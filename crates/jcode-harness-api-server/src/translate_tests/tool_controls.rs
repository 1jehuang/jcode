use super::*;

fn tool_state() -> BridgeState {
    BridgeState {
        session_id: Some("s1".into()),
        session_tools_supported: true,
        ..BridgeState::default()
    }
}

#[test]
fn session_tool_controls_translate_and_ack_without_ending_turn() {
    for mut request in [
        json!({"req":"configure_tools","tools":{}}),
        json!({"req":"configure_tools","tools":{"enabled":null}}),
        json!({"req":"configure_tools","tools":{"enabled":[],"disabled":["bash"],"custom":[{"name":"lookup","description":"Lookup","parameters":{"type":"object"}}]}}),
        json!({"req":"tool_result","call_id":"c1","output":"ok"}),
        json!({"req":"tool_result","call_id":"c1","output":"","error":"failed"}),
    ] {
        let mut state = tool_state();
        state.observed_turn_active = true;
        request["id"] = json!(7);
        request["session_id"] = json!("s1");
        let actions = state.api_request_to_legacy(&request);
        let [Outbound::Legacy(legacy)] = actions.as_slice() else {
            panic!("{actions:?}")
        };
        let mut expected = request.clone();
        expected.as_object_mut().unwrap().remove("session_id");
        expected.as_object_mut().unwrap().remove("req");
        expected["type"] = request["req"].clone();
        expected["id"] = legacy["id"].clone();
        assert_eq!(legacy, &expected);
        assert_eq!(
            state.legacy_event_to_api(&json!({"type":"ack","id":legacy["id"]})),
            vec![ServerFrame::reply(7, ApiEvent::Ok)]
        );
        // Tool controls have no legacy Done, so retain no completion IDs.
        assert!(state.pending_control_done_ids.is_empty());
        assert!(state.pending_simple.is_empty());
        assert!(state.observed_turn_active);
    }
}

#[test]
fn session_tool_inventory_correlates_and_validates_daemon_response() {
    let mut state = tool_state();
    for tools in [
        json!([]),
        json!([{"name":"lookup","description":"Lookup","parameters":{}}]),
        json!([{"name":"bad","description":"Bad","parameters":[]}]),
    ] {
        let actions =
            state.api_request_to_legacy(&json!({"id":8,"req":"list_tools","session_id":"s1"}));
        let [Outbound::Legacy(legacy)] = actions.as_slice() else {
            panic!()
        };
        assert_eq!(legacy["type"], "list_tools");
        assert!(
            state
                .legacy_event_to_api(&json!({"type":"ack","id":legacy["id"]}))
                .is_empty()
        );
        assert!(
            state
                .legacy_event_to_api(&json!({"type":"tools","id":u64::MAX,"tools":tools}))
                .is_empty()
        );
        let result =
            state.legacy_event_to_api(&json!({"type":"tools","id":legacy["id"],"tools":tools}));
        assert_eq!(result[0].reply_to, Some(8));
        if tools.get(0).is_some_and(|t| t["name"] == "bad") {
            assert!(matches!(
                result[0].event,
                ApiEvent::Error {
                    code: ErrorCode::Internal,
                    ..
                }
            ));
        } else {
            assert_eq!(
                serde_json::to_value(&result[0]).unwrap(),
                json!({"v":1,"reply_to":8,"ev":"tools","session_id":"s1","tools":tools})
            );
        }
        assert!(
            state
                .legacy_event_to_api(&json!({"type":"tools","id":legacy["id"],"tools":tools}))
                .is_empty()
        );
    }
}

#[test]
fn custom_tool_call_is_distinct_from_streaming_lifecycle() {
    let mut state = tool_state();
    let frames = state.legacy_event_to_api(
        &json!({"type":"tool_call","session_id":"s1","call_id":"c1","name":"lookup","input":{"x":1}}),
    );
    assert_eq!(
        serde_json::to_value(&frames[0]).unwrap(),
        json!({"v":1,"ev":"tool_call","session_id":"s1","call_id":"c1","name":"lookup","input":{"x":1}})
    );
    assert!(
        state
            .legacy_event_to_api(
                &json!({"type":"tool_call","session_id":"s1","call_id":"c1","name":"lookup"})
            )
            .is_empty()
    );
    for session_id in [json!("other"), Value::Null] {
        assert!(state.legacy_event_to_api(&json!({"type":"tool_call","session_id":session_id,"call_id":"c1","name":"lookup","input":{}})).is_empty());
    }
    assert!(
        state
            .legacy_event_to_api(
                &json!({"type":"tool_call","call_id":"c1","name":"lookup","input":{}})
            )
            .is_empty()
    );
    state.session_id = None;
    assert!(
        state
            .legacy_event_to_api(
                &json!({"type":"tool_call","session_id":"s1","call_id":"c1","name":"lookup","input":null})
            )
            .is_empty()
    );
}

#[test]
fn session_tool_controls_require_attachment_and_negotiated_support() {
    for mut request in [
        json!({"req":"configure_tools","tools":{}}),
        json!({"req":"list_tools"}),
        json!({"req":"tool_result","call_id":"c1","output":"ok"}),
    ] {
        request["id"] = json!(9);
        request["session_id"] = json!("s1");
        for (mut state, code) in [
            (BridgeState::default(), ErrorCode::UnknownSession),
            (
                BridgeState {
                    session_id: Some("other".into()),
                    ..tool_state()
                },
                ErrorCode::UnknownSession,
            ),
            (
                BridgeState {
                    session_tools_supported: false,
                    ..tool_state()
                },
                ErrorCode::UnknownRequest,
            ),
        ] {
            let actions = state.api_request_to_legacy(&request);
            assert!(
                matches!(actions.as_slice(), [Outbound::Reply(ServerFrame { reply_to: Some(9), event: ApiEvent::Error { code: actual, .. }, .. })] if actual == &code)
            );
            assert!(state.pending_simple.is_empty());
        }
        let mut state = tool_state();
        state.observed_turn_active = true;
        let actions = state.api_request_to_legacy(&request);
        let [Outbound::Legacy(legacy)] = actions.as_slice() else {
            panic!()
        };
        let result = state.legacy_event_to_api(
            &json!({"type":"error","id":legacy["id"],"message":"invalid tool"}),
        );
        assert_eq!(
            result,
            vec![ServerFrame::reply(
                9,
                ApiEvent::Error {
                    code: ErrorCode::Internal,
                    message: "invalid tool".into()
                }
            )]
        );
        assert!(state.observed_turn_active);
    }
}

#[test]
fn malformed_tool_controls_never_reach_daemon() {
    let mut state = tool_state();
    for mut request in [
        json!({"req":"configure_tools","tools":{"enabled":false}}),
        json!({"req":"configure_tools","tools":{"custom":[{"name":"x","description":"x","parameters":[]}]}}),
        json!({"req":"configure_tools"}),
        json!({"req":"tool_result","call_id":"x","output":{}}),
        json!({"req":"tool_result","call_id":"x","output":"","error":false}),
    ] {
        request["id"] = json!(10);
        request["session_id"] = json!("s1");
        let actions = state.api_request_to_legacy(&request);
        assert!(matches!(
            actions.as_slice(),
            [Outbound::Reply(ServerFrame {
                reply_to: Some(10),
                event: ApiEvent::Error {
                    code: ErrorCode::InvalidRequest,
                    ..
                },
                ..
            })]
        ));
    }
}

#[test]
fn tool_controls_validate_pending_attachment_and_required_session() {
    let mut state = BridgeState {
        session_tools_supported: true,
        ..BridgeState::default()
    };
    state.api_request_to_legacy(&json!({"id":1,"req":"attach_session","session_id":"s1"}));
    assert!(matches!(
        state
            .api_request_to_legacy(&json!({"id":2,"req":"list_tools","session_id":"s1"}))
            .as_slice(),
        [Outbound::Legacy(_)]
    ));
    for (session_id, code) in [
        (json!("s2"), ErrorCode::UnknownSession),
        (json!(""), ErrorCode::InvalidRequest),
        (Value::Null, ErrorCode::InvalidRequest),
    ] {
        let actions = state
            .api_request_to_legacy(&json!({"id":3,"req":"list_tools","session_id":session_id}));
        assert!(
            matches!(actions.as_slice(), [Outbound::Reply(ServerFrame { event: ApiEvent::Error { code: actual, .. }, .. })] if actual == &code)
        );
    }
}

#[test]
fn detach_waits_for_release_barrier_and_drops_late_callbacks() {
    let mut state = tool_state();
    state.observed_turn_active = true;
    let actions =
        state.api_request_to_legacy(&json!({"id":11,"req":"detach_session","session_id":"s1"}));
    let [Outbound::Legacy(legacy)] = actions.as_slice() else {
        panic!("detach must not acknowledge locally")
    };
    assert!(
        state
            .legacy_event_to_api(&json!({"type":"ack","id":legacy["id"]}))
            .is_empty()
    );
    assert_eq!(state.session_id.as_deref(), Some("s1"));
    assert_eq!(
        state.legacy_event_to_api(&json!({"type":"done","id":legacy["id"]})),
        vec![ServerFrame::reply(11, ApiEvent::Ok)]
    );
    assert!(state.session_id.is_none());
    assert!(state.pending_simple.is_empty());
    assert!(state.pending_control_done_ids.is_empty());
    assert!(state.legacy_event_to_api(&json!({"type":"tool_call","session_id":"s1","call_id":"c1","name":"lookup","input":{}})).is_empty());
    let actions =
        state.api_request_to_legacy(&json!({"id":12,"req":"list_tools","session_id":"s1"}));
    assert!(matches!(
        actions.as_slice(),
        [Outbound::Reply(ServerFrame {
            event: ApiEvent::Error {
                code: ErrorCode::UnknownSession,
                ..
            },
            ..
        })]
    ));
}

#[test]
fn abnormal_stop_precedes_error_and_done_and_preserves_specific_reason() {
    use jcode_harness_api::TurnStopReason;
    for (reason, expected) in [
        ("failure", TurnStopReason::Failure),
        ("crash", TurnStopReason::Crash),
        ("limit_reached", TurnStopReason::LimitReached),
    ] {
        let mut state = state_with_session();
        let stopped = state.legacy_event_to_api(
            &json!({"type":"turn_stopped", "reason":reason, "message":"Specific explanation"}),
        );
        assert!(
            matches!(&stopped[0].event, ApiEvent::TurnStopped { session_id, reason, message, .. } if session_id == "s1" && *reason == expected && message == "Specific explanation")
        );
        assert!(
            state
                .legacy_event_to_api(
                    &json!({"type":"turn_stopped","reason":"failure","message":"generic wrapper"})
                )
                .is_empty()
        );
        let end = state.legacy_event_to_api(&json!({"type":"error","id":0,"message":"failed"}));
        assert!(matches!(&end[0].event, ApiEvent::Error { .. }));
        assert!(matches!(
            &end.last().unwrap().event,
            ApiEvent::TurnDone { .. }
        ));
        assert!(
            state
                .legacy_event_to_api(&json!({"type":"done","id":0}))
                .is_empty()
        );
        state.legacy_event_to_api(&json!({"type":"text_delta","text":"next turn"}));
        let natural = state.legacy_event_to_api(&json!({"type":"done","id":0}));
        assert!(
            natural
                .iter()
                .all(|f| !matches!(f.event, ApiEvent::TurnStopped { .. }))
        );
    }
}

#[test]
fn guardrail_stop_preserves_provider_reason_and_finishes_observer_turn() {
    let mut state = state_with_session();
    let frames = state.legacy_event_to_api(
        &json!({"type":"provider_guardrail","stop_reason":"refusal","message":"Provider refused"}),
    );
    assert!(
        matches!(&frames[0].event, ApiEvent::TurnStopped { reason: jcode_harness_api::TurnStopReason::ProviderGuardrail, provider_stop_reason: Some(reason), .. } if reason == "refusal")
    );
    assert!(matches!(
        &state.legacy_event_to_api(&json!({"type":"done","id":0}))[0].event,
        ApiEvent::TurnDone { .. }
    ));
}

#[test]
fn legacy_interrupt_finishes_observer_turn_but_idle_cancel_is_not_a_stop() {
    let mut state = state_with_session();
    let idle = state.legacy_event_to_api(&json!({"type":"interrupted"}));
    assert!(
        idle.iter()
            .all(|f| !matches!(f.event, ApiEvent::TurnStopped { .. }))
    );
    state.legacy_event_to_api(&json!({"type":"text_delta","text":"partial"}));
    let stopped = state.legacy_event_to_api(&json!({"type":"interrupted"}));
    assert!(matches!(
        &stopped[0].event,
        ApiEvent::TurnStopped {
            reason: jcode_harness_api::TurnStopReason::Interrupted,
            ..
        }
    ));
    let done = state.legacy_event_to_api(&json!({"type":"done","id":0}));
    assert!(matches!(
        &done.last().unwrap().event,
        ApiEvent::TurnDone { .. }
    ));
    let late = state.legacy_event_to_api(&json!({"type":"interrupted"}));
    assert!(
        late.iter()
            .all(|f| !matches!(f.event, ApiEvent::TurnStopped { .. }))
    );
}

#[test]
fn request_error_does_not_fabricate_a_turn_stop() {
    let mut state = state_with_session();
    let frames =
        state.legacy_event_to_api(&json!({"type":"error","id":777,"message":"request failed"}));
    assert!(matches!(
        frames.as_slice(),
        [ServerFrame {
            event: ApiEvent::Error { .. },
            ..
        }]
    ));
}

#[test]
fn tool_streaming_forwards_zero_argument_starts_and_keyed_interleaved_input() {
    let mut state = state_with_session();
    for id in ["a", "b"] {
        let frames = state.legacy_event_to_api(&json!({"type":"tool_start","id":id,"name":"bash"}));
        assert!(matches!(frames.as_slice(), [ServerFrame {
            event: ApiEvent::ToolStart { call_id, name, .. }, ..
        }] if call_id == id && name == "bash"));
    }
    for (id, delta) in [
        ("b", "{\"command\":"),
        ("a", "{\"command\":\"echo a\"}"),
        ("b", "\"echo b\"}"),
    ] {
        let frames = state.legacy_event_to_api(&json!({"type":"tool_input","id":id,"delta":delta}));
        assert!(matches!(frames.as_slice(), [ServerFrame {
            event: ApiEvent::ToolInputDelta { call_id, delta: actual, .. }, ..
        }] if call_id == id && actual == delta));
    }
    let legacy = state.legacy_event_to_api(&json!({"type":"tool_input","delta":"{}"}));
    assert!(matches!(legacy.as_slice(), [ServerFrame {
        event: ApiEvent::ToolInputDelta { call_id, delta, .. }, ..
    }] if call_id.is_empty() && delta == "{}"));
}

#[test]
fn pre_stream_activity_invalidates_older_idle_history() {
    for phase in [
        None,
        Some("authenticating"),
        Some("connecting"),
        Some("sending request"),
        Some("waiting for response"),
        Some("streaming"),
        Some("retrying (2/4)"),
    ] {
        let mut state = state_with_session();
        let history =
            state.api_request_to_legacy(&json!({"req":"get_history", "id":22, "session_id":"s1"}));
        let Outbound::Legacy(probe) = &history[0] else {
            panic!()
        };
        match phase {
            Some(phase) => {
                state.legacy_event_to_api(&json!({"type":"connection_phase", "phase":phase}));
            }
            None => {
                let send = state.api_request_to_legacy(
                    &json!({"req":"send_message", "id":23, "session_id":"s1", "content":"hi"}),
                );
                let Outbound::Legacy(message) = &send[0] else {
                    panic!()
                };
                state.legacy_event_to_api(&json!({"type":"ack", "id":message["id"]}));
            }
        }
        let frames = state.legacy_event_to_api(&json!({
            "type":"history", "id":probe["id"], "session_id":"s1",
            "messages":[], "activity":{"is_processing":false}
        }));
        assert!(
            frames
                .iter()
                .all(|frame| !matches!(frame.event, ApiEvent::SessionStatus { .. })),
            "{phase:?}"
        );
        assert!(state.observed_turn_active, "{phase:?}");
        // Observer turns can end before any text, including provider retries.
        let done = state.legacy_event_to_api(&json!({"type":"done", "id":0}));
        assert!(
            matches!(
                done.last().map(|frame| &frame.event),
                Some(ApiEvent::TurnDone { .. })
            ),
            "{phase:?}"
        );
        assert!(!state.observed_turn_active);
    }
}

#[test]
fn transport_bookkeeping_does_not_start_a_turn() {
    let mut state = state_with_session();
    for phase in ["connected", "", "unknown"] {
        state.legacy_event_to_api(&json!({"type":"connection_phase", "phase":phase}));
        assert!(!state.observed_turn_active);
        assert_eq!(state.activity_version, 0);
    }
    state.legacy_event_to_api(&json!({"type":"ack", "id":999999}));
    assert!(!state.observed_turn_active);
    assert_eq!(state.activity_version, 0);
}
