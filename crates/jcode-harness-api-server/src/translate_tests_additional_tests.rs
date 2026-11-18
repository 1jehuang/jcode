use super::*;

#[test]
fn session_mutations_correlate_interleaved_terminals_after_receipt() {
    for scoped in [false, true] {
        let mut state = state_with_session();
        if scoped {
            state.legacy_event_to_api(&json!({
                "type":"turn_state", "session_id":state.session_id,
                "runtime_generation":"runtime", "latest_turn_id":0,
            }));
        }
        // Include every invalid environment from the issue's SDK reproduction.
        // The bridge forwards string maps for daemon validation, not success.
        let requests = [
            json!({"id":71,"req":"set_session_env","env":{"BAD=KEY":"v"}}),
            json!({"id":72,"req":"set_session_env","env":{"BAD":"nul\0value"}}),
            json!({"id":73,"req":"set_session_env","env":{"JCODE_SESSION_ID":"x"}}),
            json!({"id":74,"req":"set_session_env","env":{"":"v"}}),
            json!({"id":75,"req":"set_session_env","env":{"GOOD":"v"}}),
            json!({"id":76,"req":"rename_session","title":"idle title"}),
            json!({"id":77,"req":"set_session_saved","saved":true}),
        ];
        let mut ids = Vec::new();
        for request in &requests {
            let outbound = state.api_request_to_legacy(request);
            let [Outbound::Legacy(legacy)] = outbound.as_slice() else {
                panic!("{request}: {outbound:?}");
            };
            let id = legacy["id"].as_u64().unwrap();
            assert!(
                state
                    .legacy_event_to_api(&json!({"type":"ack","id":id}))
                    .is_empty(),
                "receipt must not resolve {request}"
            );
            ids.push(id);
        }
        assert_eq!(state.pending_simple.len(), requests.len());
        // Deliver terminals in reverse order. A terminal must resolve only its
        // own API request, even when all receipts precede all completions.
        for index in (0..requests.len()).rev() {
            let succeeds = index >= 4;
            let terminal = if succeeds {
                json!({"type":"done","id":ids[index]})
            } else {
                json!({"type":"error","id":ids[index],"message":"Invalid session environment"})
            };
            assert_eq!(
                state.legacy_event_to_api(&terminal),
                vec![ServerFrame::reply(
                    requests[index]["id"].as_u64().unwrap(),
                    if succeeds {
                        ApiEvent::Ok
                    } else {
                        ApiEvent::Error {
                            code: ErrorCode::Internal,
                            message: "Invalid session environment".into(),
                        }
                    }
                )],
                "scoped={scoped}, request={}",
                requests[index]
            );
            assert_eq!(state.pending_simple.len(), index);
            assert!(!state.observed_turn_active);
            assert!(state.pending_message_id.is_none());
        }
        assert!(state.pending_control_done_ids.is_empty());
        assert!(state.pending_activity_snapshots.is_empty());
    }
}

#[test]
fn receipt_only_controls_keep_ack_compatibility() {
    for mut request in [
        json!({"req":"cancel"}),
        json!({"req":"soft_interrupt","content":"next"}),
        json!({"req":"notify_auth_changed","provider":"openai"}),
        json!({"req":"invalidate_usage","provider":"openai"}),
        json!({"req":"invalidate_usage","provider":"claude"}),
        json!({"req":"cancel_soft_interrupts"}),
        json!({"req":"background_tool"}),
    ] {
        let mut state = state_with_session();
        request["id"] = json!(7);
        let actions = state.api_request_to_legacy(&request);
        let [Outbound::Legacy(legacy)] = actions.as_slice() else {
            panic!("{request}: {actions:?}");
        };
        assert_eq!(
            state.legacy_event_to_api(&json!({"type":"ack","id":legacy["id"]})),
            vec![ServerFrame::reply(7, ApiEvent::Ok)],
            "{request}"
        );
        assert!(state.pending_simple.is_empty());
    }
}
