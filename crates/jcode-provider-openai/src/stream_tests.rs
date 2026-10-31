use super::*;

#[test]
fn responses_transport_activity_includes_ignored_sse_frames() {
    use futures::StreamExt;
    use futures::task::noop_waker;
    for payload in [
        ": keepalive\n\n",
        "data: {\"type\":\"response.in_progress\"}\n\n",
        "data: {\"type\":\"future.status.event\"}\n\n",
    ] {
        let input =
            futures::stream::iter([Ok(Bytes::from(payload))]).chain(futures::stream::pending());
        let mut stream = OpenAIResponsesStream::new(input);
        stream.last_transport_activity =
            std::time::Instant::now() - std::time::Duration::from_secs(300);
        let waker = noop_waker();
        let mut cx = TaskContext::from_waker(&waker);
        assert!(Pin::new(&mut stream).poll_next(&mut cx).is_pending());
        assert_eq!(stream.transport_bytes(), payload.len() as u64);
        assert!(stream.transport_idle_for() < std::time::Duration::from_secs(1));
    }
}

#[test]
fn responses_empty_chunks_do_not_reset_transport_idle() {
    use futures::StreamExt;
    use futures::task::noop_waker;
    let input = futures::stream::iter([Ok(Bytes::new())]).chain(futures::stream::pending());
    let mut stream = OpenAIResponsesStream::new(input);
    stream.last_transport_activity =
        std::time::Instant::now() - std::time::Duration::from_secs(300);
    let waker = noop_waker();
    let mut cx = TaskContext::from_waker(&waker);
    assert!(Pin::new(&mut stream).poll_next(&mut cx).is_pending());
    assert_eq!(stream.transport_bytes(), 0);
    assert!(stream.transport_idle_for() >= std::time::Duration::from_secs(300));
}

#[test]
fn cache_response_evidence_records_native_diagnosis_without_response_contents() {
    let response = serde_json::json!({
        "id": "resp_private_identifier", "model": "gpt-6.1-sol", "service_tier": "default",
        "usage": {"input_tokens": 30743, "input_tokens_details": {"cached_tokens": 20096}},
        "output": [{"content": "PRIVATE_PROMPT"}],
        "prompt_cache_diagnostics": {
            "type": "cache_miss", "reason": "service_tier_changed",
            "comparison_reusable_tokens": 30379, "cache_missed_tokens": 10283,
            "private_detail": "PRIVATE_PROVIDER_DATA"
        }
    });
    let evidence = cache_response_evidence(&response);
    assert!(evidence["response_id_hash"].as_u64().is_some());
    assert_eq!(evidence["model"], "gpt-6.1-sol");
    assert_eq!(evidence["service_tier"], "default");
    assert_eq!(evidence["input_tokens"], 30743);
    assert_eq!(evidence["cached_tokens"], 20096);
    assert_eq!(evidence["diagnostic_type"], "cache_miss");
    assert_eq!(evidence["diagnostic_reason"], "service_tier_changed");
    assert_eq!(evidence["comparison_reusable_tokens"], 30379);
    assert_eq!(evidence["cache_missed_tokens"], 10283);
    let serialized = evidence.to_string();
    for private in [
        "resp_private_identifier",
        "PRIVATE_PROMPT",
        "PRIVATE_PROVIDER_DATA",
    ] {
        assert!(!serialized.contains(private));
    }
}

#[test]
fn cache_response_evidence_does_not_invent_missing_or_malformed_metadata() {
    let evidence = cache_response_evidence(&serde_json::json!({
        "model": "invalid\nlog entry", "service_tier": "x".repeat(129),
        "usage": {"input_tokens": -1, "input_tokens_details": {"cached_tokens": "20096"}},
        "prompt_cache_diagnostics": {
            "type": "unknown PRIVATE_DATA", "reason": "unknown PRIVATE_DATA",
            "comparison_reusable_tokens": -10, "cache_missed_tokens": "10283"
        }
    }));
    assert!(evidence.as_object().unwrap().values().all(Value::is_null));
    let missing = cache_response_evidence(&serde_json::json!({}));
    assert!(missing.as_object().unwrap().values().all(Value::is_null));
}

#[test]
fn cache_response_evidence_matches_compatibility_usage_counters() {
    let evidence = cache_response_evidence(&serde_json::json!({"usage": {
        "input_tokens_details": {"cached_tokens": null},
        "prompt_tokens_details": {"cached_tokens": 1024, "cache_write_tokens": 128}
    }}));
    assert_eq!(evidence["cached_tokens"], 1024);
    assert_eq!(evidence["cache_write_tokens"], 128);
    let absent = cache_response_evidence(&serde_json::json!({"usage": {"input_tokens": 2048}}));
    assert!(absent["cached_tokens"].is_null());
    assert!(absent["cache_write_tokens"].is_null());
}

#[test]
fn service_tier_metadata_is_provider_reported_and_precedes_completion() {
    for kind in ["response.created", "response.completed"] {
        for tier in ["default", "priority", "ultrafast"] {
            let frame = serde_json::json!({"type":kind,"response":{"service_tier":tier}});
            let mut pending = VecDeque::new();
            let event = parse_openai_response_event(
                &frame.to_string(),
                &mut false,
                &mut false,
                &mut HashMap::new(),
                &mut HashSet::new(),
                &mut pending,
            );
            assert!(
                matches!(event, Some(StreamEvent::ServiceTier { tier: actual, requested: None }) if actual == tier)
            );
            if kind == "response.completed" {
                assert!(matches!(
                    pending.pop_front(),
                    Some(StreamEvent::MessageEnd { .. })
                ));
            }
            assert!(pending.is_empty());
        }
    }
}

#[test]
fn service_tier_metadata_does_not_infer_missing_or_invalid_tiers() {
    for response in [
        serde_json::json!({}),
        serde_json::json!({"service_tier":null}),
        serde_json::json!({"service_tier":3}),
        serde_json::json!({"service_tier":" "}),
    ] {
        let frame = serde_json::json!({"type":"response.created","response":response});
        assert!(
            parse_openai_response_event(
                &frame.to_string(),
                &mut false,
                &mut false,
                &mut HashMap::new(),
                &mut HashSet::new(),
                &mut VecDeque::new(),
            )
            .is_none()
        );
    }
}

#[test]
fn usage_preserves_inclusive_input_and_optional_cache_details() {
    for (usage, expected_read, expected_write) in [
        (
            serde_json::json!({"input_tokens_details": {
                "cached_tokens": 12000, "cache_write_tokens": 3000
            }}),
            Some(12000),
            Some(3000),
        ),
        (
            serde_json::json!({"input_tokens_details": {
                "cached_tokens": 0, "cache_write_tokens": 15000
            }}),
            Some(0),
            Some(15000),
        ),
        (
            serde_json::json!({"input_tokens_details": {"cached_tokens": 12000}}),
            Some(12000),
            None,
        ),
        (serde_json::json!({}), None, None),
        (
            serde_json::json!({"input_tokens_details": null}),
            None,
            None,
        ),
        (
            serde_json::json!({"input_tokens_details": {
                "cached_tokens": -1, "cache_write_tokens": "3000"
            }}),
            None,
            None,
        ),
        (
            serde_json::json!({"prompt_tokens_details": {
                "cached_tokens": 12000, "cache_write_tokens": 3000
            }}),
            Some(12000),
            Some(3000),
        ),
        (
            serde_json::json!({
                "input_tokens_details": {"cached_tokens": null},
                "prompt_tokens_details": {
                    "cached_tokens": 12000, "cache_write_tokens": 3000
                }
            }),
            Some(12000),
            Some(3000),
        ),
        (
            serde_json::json!({
                "input_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 0},
                "prompt_tokens_details": {
                    "cached_tokens": 12000, "cache_write_tokens": 3000
                }
            }),
            Some(0),
            Some(0),
        ),
    ] {
        let mut usage = usage;
        usage["input_tokens"] = serde_json::json!(15000);
        usage["output_tokens"] = serde_json::json!(200);
        let response = serde_json::json!({"usage": usage});
        let Some(StreamEvent::TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
        }) = extract_usage_from_response(&response)
        else {
            panic!("missing usage for {response}");
        };
        assert_eq!(input_tokens, Some(15000), "{response}");
        assert_eq!(output_tokens, Some(200), "{response}");
        assert_eq!(cache_read_input_tokens, expected_read, "{response}");
        assert_eq!(cache_creation_input_tokens, expected_write, "{response}");
    }
}

#[test]
fn terminal_responses_emit_cache_write_only_usage_before_message_end() {
    for kind in ["response.completed", "response.incomplete"] {
        let frame = serde_json::json!({
            "type": kind,
            "response": {"usage": {"input_tokens_details": {"cache_write_tokens": 1024}}}
        });
        let mut pending = VecDeque::new();
        let event = parse_openai_response_event(
            &frame.to_string(),
            &mut false,
            &mut false,
            &mut HashMap::new(),
            &mut HashSet::new(),
            &mut pending,
        );
        assert!(matches!(
            event,
            Some(StreamEvent::TokenUsage {
                input_tokens: None,
                output_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: Some(1024),
            })
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(StreamEvent::MessageEnd { .. })
        ));
        assert!(pending.is_empty());
    }
}

#[test]
fn missing_usage_does_not_report_a_cache_miss() {
    for response in [
        serde_json::json!({}),
        serde_json::json!({"usage": null}),
        serde_json::json!({"usage": {}}),
        serde_json::json!({"usage": {"input_tokens_details": {}}}),
    ] {
        assert!(extract_usage_from_response(&response).is_none());
    }
}

#[test]
fn structured_stream_read_error_is_extracted_and_classified_as_transient() {
    let error = serde_json::json!({
        "type": "upstream_error",
        "code": "stream_read_error"
    });

    let (message, retry_after) = extract_error_with_retry(&None, &Some(error));

    assert_eq!(
        message,
        "upstream_error (stream_read_error): OpenAI response stream error (unknown)"
    );
    assert_eq!(retry_after, None);
    assert!(jcode_provider_core::is_transient_transport_error(&message));
}

#[test]
fn parse_text_wrapped_tool_call_rejects_non_object_json() {
    let text = "prefix to=functions.read [1,2,3]";
    let parsed = parse_text_wrapped_tool_call(text);
    assert!(parsed.is_none());
}

#[test]
fn parse_openai_response_event_ignores_malformed_json_chunks() {
    let mut saw_text_delta = false;
    let mut saw_thinking_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let event = parse_openai_response_event(
        "{not-json}",
        &mut saw_text_delta,
        &mut saw_thinking_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    );

    assert!(event.is_none());
    assert!(!saw_text_delta);
    assert!(streaming_tool_calls.is_empty());
    assert!(completed_tool_items.is_empty());
    assert!(pending.is_empty());
}

#[test]
fn response_completed_emits_message_end_even_when_payload_mentions_fallback() {
    // Regression: when the model edits source that mentions the websocket
    // fallback phrase, that text rides along inside structured events. A
    // `response.completed` frame containing the phrase must still produce a
    // MessageEnd, otherwise the stream "ends before the completion marker".
    let mut saw_text_delta = false;
    let mut saw_thinking_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let payload = serde_json::json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "falling back from websockets to https transport"
                }]
            }]
        }
    })
    .to_string();

    let event = parse_openai_response_event(
        &payload,
        &mut saw_text_delta,
        &mut saw_thinking_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    );

    assert!(
        matches!(event, Some(StreamEvent::MessageEnd { .. })),
        "expected MessageEnd, got {event:?}"
    );
}

#[test]
fn function_call_arguments_with_fallback_phrase_still_emit_tool_call() {
    let mut saw_text_delta = false;
    let mut saw_thinking_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let payload = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "item_id": "fc_1",
        "call_id": "call_1",
        "name": "bash",
        "arguments": "{\"command\":\"echo falling back from websockets to https transport\"}"
    })
    .to_string();

    let event = parse_openai_response_event(
        &payload,
        &mut saw_text_delta,
        &mut saw_thinking_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    );

    assert!(
        matches!(event, Some(StreamEvent::ToolUseStart { .. })),
        "expected ToolUseStart, got {event:?}"
    );
}

#[test]
fn plain_text_fallback_notice_is_still_dropped() {
    let mut saw_text_delta = false;
    let mut saw_thinking_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut pending = VecDeque::new();

    let event = parse_openai_response_event(
        "falling back from websockets to https transport",
        &mut saw_text_delta,
        &mut saw_thinking_delta,
        &mut streaming_tool_calls,
        &mut completed_tool_items,
        &mut pending,
    );

    assert!(event.is_none());
}
