//! Tests for `#[path]`-attributed module `text_framing_tests` of `stream.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::*;

#[test]
fn output_item_completion_frames_messages_not_reasoning_or_text_chunks() {
    let mut saw_text = false;
    let mut saw_thinking = false;
    let mut tools = HashMap::new();
    let mut completed = HashSet::new();
    let mut pending = VecDeque::new();
    let mut events = Vec::new();
    for value in [
        serde_json::json!({"type":"response.output_text.delta","delta":"The cause is "}),
        serde_json::json!({"type":"response.reasoning_summary_text.delta","delta":"thinking"}),
        serde_json::json!({"type":"response.output_text.delta","delta":"the retry loop."}),
        serde_json::json!({"type":"response.output_item.done","item":{"type":"message","content":[{"type":"output_text","text":"The cause is the retry loop."}]}}),
        // This message has no text delta. Per-message deduplication must
        // allow fallback output even though the preceding message streamed.
        serde_json::json!({"type":"response.output_item.done","item":{"type":"message","content":[{"type":"output_text","text":"Second message"}]}}),
        serde_json::json!({"type":"response.completed","response":{}}),
    ] {
        events.extend(parse_openai_response_event(
            &value.to_string(),
            &mut saw_text,
            &mut saw_thinking,
            &mut tools,
            &mut completed,
            &mut pending,
        ));
        events.extend(pending.drain(..));
    }
    assert!(matches!(events.as_slice(), [
        StreamEvent::TextDelta(a), StreamEvent::ThinkingDelta(_), StreamEvent::TextDelta(b),
        StreamEvent::TextDone, StreamEvent::TextDelta(c), StreamEvent::TextDone,
        StreamEvent::MessageEnd { .. }
    ] if a == "The cause is " && b == "the retry loop." && c == "Second message"));
}

#[test]
fn web_search_call_item_is_emitted_verbatim() {
    let item = serde_json::json!({
        "type": "web_search_call", "id": "ws_1", "status": "completed",
        "action": {"type": "search", "query": "jcode"}
    });
    let mut pending = VecDeque::new();
    let event = parse_openai_response_event(
        &serde_json::json!({"type": "response.output_item.done", "item": item}).to_string(),
        &mut false,
        &mut false,
        &mut HashMap::new(),
        &mut HashSet::new(),
        &mut pending,
    );
    match event {
        Some(StreamEvent::ProviderNative {
            provider,
            item: got,
        }) => {
            assert_eq!(provider, "openai");
            assert_eq!(got, item);
        }
        other => panic!("expected provider-native item, got {other:?}"),
    }
    assert!(pending.is_empty());
}
