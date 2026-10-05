//! Tests for `#[path]`-attributed module `tests` of `stream.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::*;

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
