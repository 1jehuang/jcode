use super::*;

#[test]
fn text_framing_preserves_chunks_and_reasoning_then_separates_messages() {
    let mut state = state_with_session();
    let first = state.legacy_event_to_api(&json!({"type":"text_delta","text":"The cause is "}));
    let ApiEvent::TextDelta {
        message_id: Some(id),
        ..
    } = &first[0].event
    else {
        panic!("missing id")
    };
    state.legacy_event_to_api(&json!({"type":"reasoning_delta","text":"thinking"}));
    state.legacy_event_to_api(&json!({"type":"reasoning_done"}));
    let second = state.legacy_event_to_api(&json!({"type":"text_delta","text":"the retry loop."}));
    assert!(
        matches!(&second[0].event, ApiEvent::TextDelta { message_id: Some(next), .. } if next == id)
    );
    let end = state.legacy_event_to_api(&json!({"type":"text_done"}));
    assert!(
        matches!(&end[0].event, ApiEvent::TextDone { message_id: Some(next), .. } if next == id)
    );
    assert!(
        state
            .legacy_event_to_api(&json!({"type":"text_done"}))
            .is_empty()
    );
    let next = state.legacy_event_to_api(&json!({"type":"text_delta","text":"Another message"}));
    assert!(
        matches!(&next[0].event, ApiEvent::TextDelta { message_id: Some(next), .. } if next != id)
    );
    assert!(matches!(
        &state.legacy_event_to_api(&json!({"type":"message_end"}))[0].event,
        ApiEvent::TextDone { .. }
    ));
    assert!(
        state
            .legacy_event_to_api(&json!({"type":"message_end"}))
            .is_empty()
    );
    assert!(matches!(
        state
            .legacy_event_to_api(&json!({"type":"done","id":0}))
            .as_slice(),
        [ServerFrame {
            event: ApiEvent::TurnDone { .. },
            ..
        }]
    ));
}

#[test]
fn citation_suffix_replacements_preserve_messages_across_native_search_and_retry() {
    let mut state = state_with_session();
    let first = state.legacy_event_to_api(&json!({"type":"text_delta","text":"Checking sources."}));
    let ApiEvent::TextDelta {
        message_id: Some(first_id),
        ..
    } = &first[0].event
    else {
        panic!("missing first message id")
    };
    state.legacy_event_to_api(&json!({"type":"text_done"}));
    state.legacy_event_to_api(&json!({"type":"tool_start","id":"ws_1","name":"web_search"}));
    state.legacy_event_to_api(&json!({"type":"tool_exec","id":"ws_1","name":"web_search"}));
    state.legacy_event_to_api(
        &json!({"type":"tool_done","id":"ws_1","name":"web_search","output":"Rust documentation"}),
    );

    let mut corrected_ids = Vec::new();
    let mut corrected_texts = Vec::new();
    for (raw, corrected) in [
        (
            "Install Rust. RAW_A",
            "Install Rust. [Rust](https://www.rust-lang.org/tools/install)",
        ),
        (
            "Read docs. RAW_B",
            "Read docs. [Docs](https://doc.rust-lang.org/)",
        ),
    ] {
        let delta = state.legacy_event_to_api(&json!({"type":"text_delta","text":raw}));
        let ApiEvent::TextDelta {
            message_id: Some(id),
            ..
        } = &delta[0].event
        else {
            panic!("missing citation message id")
        };
        assert_ne!(id, first_id);
        assert!(!corrected_ids.contains(id));
        let replacement = state.legacy_event_to_api(&json!({
            "type":"text_suffix_replace","old_text":raw,"new_text":corrected
        }));
        assert!(matches!(replacement.as_slice(), [ServerFrame {
            event: ApiEvent::TextReplace { session_id, message_id: Some(next), text, .. }, ..
        }] if session_id == "s1" && next == id && text == corrected));
        let done = state.legacy_event_to_api(&json!({"type":"text_done"}));
        assert!(matches!(done.as_slice(), [ServerFrame {
            event: ApiEvent::TextDone { message_id: Some(next), .. }, ..
        }] if next == id));
        corrected_ids.push(id.clone());
        corrected_texts.push(corrected.to_string());
    }
    assert_eq!(state.text_attempt.len(), 3);
    assert_eq!(state.text_attempt[0].1, "Checking sources.");
    for (index, text) in corrected_texts.iter().enumerate() {
        assert_eq!(&state.text_attempt[index + 1].1, text);
        assert!(state.text_attempt[index + 1].2);
    }
    assert!(
        state
            .legacy_event_to_api(&json!({"type":"message_end"}))
            .is_empty()
    );

    // Corrections retain the attempt ledger so a failed request retracts every
    // completed message under its original correlation id, not just the last.
    let rollback = state.legacy_event_to_api(&json!({"type":"retry_rollback","attempt":2,"max":3}));
    assert_eq!(rollback.len(), 3);
    for (frame, id) in rollback
        .iter()
        .zip(std::iter::once(first_id).chain(corrected_ids.iter()))
    {
        assert!(matches!(&frame.event, ApiEvent::TextReplace {
            message_id: Some(next), text, ..
        } if next == id && text.is_empty()));
    }
    assert!(state.text_attempt.is_empty());
    let retry = state.legacy_event_to_api(&json!({"type":"text_delta","text":"Retried answer"}));
    assert!(matches!(&retry[0].event, ApiEvent::TextDelta {
        message_id: Some(next), ..
    } if next != first_id && !corrected_ids.contains(next)));
}

#[test]
fn citation_suffix_replacement_preserves_unicode_prefix_and_accepts_empty_retraction() {
    let mut state = state_with_session();
    state.legacy_event_to_api(&json!({"type":"text_delta","text":"Préface 🦀. "}));
    let delta = state.legacy_event_to_api(&json!({"type":"text_delta","text":"日本語 RAW"}));
    let ApiEvent::TextDelta { message_id, .. } = &delta[0].event else {
        panic!("missing text delta")
    };
    let corrected = state.legacy_event_to_api(&json!({
        "type":"text_suffix_replace","old_text":"日本語 RAW","new_text":"日本語 [source](https://example.com/)"
    }));
    assert!(matches!(corrected.as_slice(), [ServerFrame {
        event: ApiEvent::TextReplace { message_id: next, text, .. }, ..
    }] if next == message_id && text == "Préface 🦀. 日本語 [source](https://example.com/)"));
    assert!(
        state
            .legacy_event_to_api(&json!({
                "type":"text_suffix_replace","old_text":"日本語 [source](https://example.com/)",
                "new_text":"日本語 [source](https://example.com/)"
            }))
            .is_empty()
    );
    let retracted = state.legacy_event_to_api(&json!({
        "type":"text_suffix_replace","old_text":"Préface 🦀. 日本語 [source](https://example.com/)","new_text":""
    }));
    assert!(matches!(retracted.as_slice(), [ServerFrame {
        event: ApiEvent::TextReplace { message_id: next, text, .. }, ..
    }] if next == message_id && text.is_empty()));
    assert!(
        state
            .legacy_event_to_api(&json!({"type":"text_done"}))
            .is_empty()
    );
}

#[test]
fn citation_suffix_replacement_ignores_malformed_mismatched_and_closed_messages() {
    let mut state = state_with_session();
    let correction = json!({"type":"text_suffix_replace","old_text":"raw","new_text":"corrected"});
    assert!(state.legacy_event_to_api(&correction).is_empty());
    assert!(state.text_attempt.is_empty());
    state.legacy_event_to_api(&json!({"type":"text_delta","text":"raw"}));
    state.legacy_event_to_api(&json!({"type":"text_done"}));
    assert!(state.legacy_event_to_api(&correction).is_empty());
    state.legacy_event_to_api(&json!({"type":"text_delta","text":"current answer"}));
    // A matching earlier message must never be selected over the active one.
    for event in [
        correction,
        json!({"type":"text_suffix_replace","old_text":"","new_text":"injected"}),
        json!({"type":"text_suffix_replace","old_text":"current","new_text":"wrong"}),
        json!({"type":"text_suffix_replace","old_text":null,"new_text":"wrong"}),
        json!({"type":"text_suffix_replace","old_text":"current answer"}),
        json!({"type":"text_suffix_replace","old_text":"current answer","new_text":7}),
    ] {
        assert!(state.legacy_event_to_api(&event).is_empty());
        assert_eq!(state.text_attempt[0].1, "raw");
        assert_eq!(state.text_attempt[1].1, "current answer");
    }
    state.legacy_event_to_api(&json!({"type":"message_end"}));
    assert!(
        state
            .legacy_event_to_api(&json!({
                "type":"text_suffix_replace","old_text":"current answer","new_text":"wrong"
            }))
            .is_empty()
    );
    assert_eq!(state.text_attempt[1].1, "current answer");
    state.legacy_event_to_api(&json!({"type":"kv_cache_request"}));
    assert!(
        state
            .legacy_event_to_api(&json!({
                "type":"text_suffix_replace","old_text":"","new_text":"phantom"
            }))
            .is_empty()
    );
    assert!(state.text_attempt.is_empty());
}

#[test]
fn provider_native_web_search_reaches_sdk_as_tool_events_without_tool_call() {
    // The agent renders a provider-executed search (Anthropic `srvtoolu_*`,
    // OpenAI `ws_*`) as a normal tool row. SDK clients must see the full
    // start/input/exec/done sequence and never a `tool_call` to answer.
    let mut state = state_with_session();
    let mut frames = Vec::new();
    for event in [
        json!({"type":"tool_start","id":"srvtoolu_1","name":"web_search"}),
        json!({"type":"tool_input","id":"srvtoolu_1","delta":"{\"query\":\"rust news\"}"}),
        json!({"type":"tool_exec","id":"srvtoolu_1","name":"web_search"}),
        json!({"type":"tool_done","id":"srvtoolu_1","name":"web_search",
            "output":"Rust Blog - https://blog.rust-lang.org/"}),
    ] {
        frames.extend(state.legacy_event_to_api(&event));
    }
    let events: Vec<_> = frames.iter().map(|frame| &frame.event).collect();
    assert!(matches!(
        events.as_slice(),
        [
            ApiEvent::ToolStart { call_id: a, name: n1, .. },
            ApiEvent::ToolInputDelta { call_id: b, delta, .. },
            ApiEvent::ToolExec { call_id: c, .. },
            ApiEvent::ToolDone { call_id: d, name: n2, output, error: None, .. },
        ] if [a, b, c, d].iter().all(|id| *id == "srvtoolu_1")
            && n1 == "web_search" && n2 == "web_search"
            && delta.contains("rust news")
            && output.contains("https://blog.rust-lang.org/")
    ));
}

#[test]
fn text_framing_tools_and_turn_fallback_close_once_without_phantom_messages() {
    let mut state = state_with_session();
    state.legacy_event_to_api(&json!({"type":"text_delta","text":"Checking"}));
    let tool = state.legacy_event_to_api(&json!({"type":"tool_start","id":"t","name":"read"}));
    assert!(matches!(
        tool.as_slice(),
        [ServerFrame {
            event: ApiEvent::ToolStart { .. },
            ..
        }]
    ));
    // A text content block after streamed tool arguments is still the same
    // assistant message. Only execution forces a fallback boundary.
    state.legacy_event_to_api(&json!({"type":"text_delta","text":" logs"}));
    let exec = state.legacy_event_to_api(&json!({"type":"tool_exec","id":"t","name":"read"}));
    assert!(matches!(
        exec.as_slice(),
        [
            ServerFrame {
                event: ApiEvent::TextDone { .. },
                ..
            },
            ServerFrame {
                event: ApiEvent::ToolExec { .. },
                ..
            }
        ]
    ));
    assert_eq!(
        state
            .legacy_event_to_api(&json!({"type":"tool_exec","id":"t2","name":"read"}))
            .len(),
        1
    );
    state.legacy_event_to_api(&json!({"type":"message_end"}));
    state.legacy_event_to_api(&json!({"type":"text_delta","text":"Answer"}));
    let end = state.legacy_event_to_api(&json!({"type":"done","id":0}));
    assert!(matches!(
        end.as_slice(),
        [
            ServerFrame {
                event: ApiEvent::TextDone { .. },
                ..
            },
            ServerFrame {
                event: ApiEvent::TurnDone { .. },
                ..
            }
        ]
    ));
    state.legacy_event_to_api(&json!({"type":"reasoning_delta","text":"thinking only"}));
    state.legacy_event_to_api(&json!({"type":"text_delta","text":""}));
    assert!(
        state
            .legacy_event_to_api(&json!({"type":"message_end"}))
            .is_empty()
    );
    assert_eq!(
        state
            .legacy_event_to_api(&json!({"type":"done","id":0}))
            .len(),
        1
    );
}

#[test]
fn text_retry_retracts_completed_and_live_messages_and_late_replacements_keep_ids() {
    let mut state = state_with_session();
    state.legacy_event_to_api(&json!({"type":"text_delta","text":"first"}));
    state.legacy_event_to_api(&json!({"type":"text_done"}));
    state.legacy_event_to_api(&json!({"type":"text_delta","text":"second"}));
    let rollback = state.legacy_event_to_api(&json!({"type":"retry_rollback","attempt":2,"max":3}));
    assert_eq!(rollback.len(), 2);
    assert!(rollback.iter().all(|frame| matches!(&frame.event, ApiEvent::TextReplace { text, message_id: Some(_), .. } if text.is_empty())));
    assert!(
        state
            .legacy_event_to_api(&json!({"type":"text_done"}))
            .is_empty()
    );
    let retry = state.legacy_event_to_api(&json!({"type":"text_delta","text":"valid <tool>"}));
    let ApiEvent::TextDelta { message_id, .. } = &retry[0].event else {
        panic!()
    };
    state.legacy_event_to_api(&json!({"type":"message_end"}));
    let corrected = state.legacy_event_to_api(&json!({"type":"text_replace","text":"valid"}));
    assert!(
        matches!(&corrected[0].event, ApiEvent::TextReplace { message_id: id, text, .. } if id == message_id && text == "valid")
    );
}

#[test]
fn new_request_retry_does_not_retract_previous_response() {
    for activity in ["reasoning_delta", "tool_start"] {
        let mut state = state_with_session();
        state.legacy_event_to_api(&json!({"type":"text_delta","text":"Committed"}));
        state.legacy_event_to_api(&json!({"type":"message_end"}));
        assert!(
            state
                .legacy_event_to_api(&json!({"type":"kv_cache_request"}))
                .is_empty()
        );
        state.legacy_event_to_api(
            &json!({"type":activity,"text":"thinking","id":"t","name":"read"}),
        );
        assert!(
            state
                .legacy_event_to_api(&json!({"type":"retry_rollback","attempt":2,"max":3}))
                .is_empty()
        );
    }
}

#[test]
fn request_boundary_closes_provider_without_message_end_and_ids_survive_turns() {
    let mut state = state_with_session();
    let first = state.legacy_event_to_api(&json!({"type":"text_delta","text":"first"}));
    assert!(matches!(
        &state.legacy_event_to_api(&json!({"type":"kv_cache_request"}))[0].event,
        ApiEvent::TextDone { .. }
    ));
    state.legacy_event_to_api(&json!({"type":"done","id":0}));
    let next = state.legacy_event_to_api(&json!({"type":"text_delta","text":"next turn"}));
    let ApiEvent::TextDelta { message_id: a, .. } = &first[0].event else {
        panic!()
    };
    let ApiEvent::TextDelta { message_id: b, .. } = &next[0].event else {
        panic!()
    };
    assert_ne!(a, b);
}

#[test]
fn late_recovered_suffix_completes_previously_empty_or_unseen_text() {
    for partial in [false, true] {
        let mut state = state_with_session();
        if partial {
            state.legacy_event_to_api(&json!({"type":"text_delta","text":"to=fun"}));
        }
        state.legacy_event_to_api(&json!({"type":"text_replace","text":""}));
        assert!(
            state
                .legacy_event_to_api(&json!({"type":"message_end"}))
                .is_empty()
        );
        let recovered =
            state.legacy_event_to_api(&json!({"type":"text_replace","text":"Retained suffix"}));
        assert!(matches!(recovered.as_slice(), [
            ServerFrame { event: ApiEvent::TextReplace { text, message_id: Some(a), .. }, .. },
            ServerFrame { event: ApiEvent::TextDone { message_id: Some(b), .. }, .. }
        ] if text == "Retained suffix" && a == b));
        assert_eq!(
            state
                .legacy_event_to_api(&json!({"type":"tool_start","id":"t","name":"read"}))
                .len(),
            1
        );
        assert_eq!(
            state
                .legacy_event_to_api(&json!({"type":"tool_exec","id":"t","name":"read"}))
                .len(),
            1
        );
        assert!(
            state
                .legacy_event_to_api(&json!({"type":"text_done"}))
                .is_empty()
        );
    }
}
