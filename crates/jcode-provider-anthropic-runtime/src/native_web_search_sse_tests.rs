//! SSE parsing and request shaping for Anthropic's server-side `web_search`.

use super::*;
use serde_json::json;

fn sse(event_type: &str, data: serde_json::Value) -> SseEvent {
    SseEvent {
        event_type: event_type.to_string(),
        data: data.to_string(),
    }
}

fn provider_native_items(events: &[StreamEvent]) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ProviderNative { provider, item } => {
                assert_eq!(provider, "anthropic");
                Some(item.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn server_tool_use_streams_input_and_emits_one_native_item() {
    let mut state = SseStreamState::default();
    let mut events = Vec::new();
    for event in [
        sse(
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {
                "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {}
            }}),
        ),
        sse(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1,
                "delta": {"type": "input_json_delta", "partial_json": "{\"query\": \"ru"}}),
        ),
        sse(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1,
                "delta": {"type": "input_json_delta", "partial_json": "st\"}"}}),
        ),
        sse(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 1}),
        ),
    ] {
        events.extend(process_sse_event(&event, &mut state, false));
    }

    // Never surfaced as a jcode tool call: the provider already ran it.
    assert!(!events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolUseStart { .. } | StreamEvent::ToolInputDelta(_) | StreamEvent::ToolUseEnd
    )));
    assert_eq!(
        provider_native_items(&events),
        vec![json!({
            "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
            "input": {"query": "rust"}
        })]
    );
    assert!(state.current_server_block.is_none());
}

#[test]
fn web_search_tool_result_is_kept_verbatim() {
    let block = json!({
        "type": "web_search_tool_result",
        "tool_use_id": "srvtoolu_1",
        "content": [{
            "type": "web_search_result",
            "url": "https://www.rust-lang.org/",
            "title": "Rust",
            "encrypted_content": "EqgfCioIARgBIiQ3YTAwMjY1Mi1mZjM5LTQ1NGUtODgxNC1kNjNjNTk1ZWI3Y",
            "page_age": "2 days ago"
        }]
    });
    let mut state = SseStreamState::default();
    let mut events = process_sse_event(
        &sse(
            "content_block_start",
            json!({"type": "content_block_start", "index": 2, "content_block": block}),
        ),
        &mut state,
        false,
    );
    events.extend(process_sse_event(
        &sse(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 2}),
        ),
        &mut state,
        false,
    ));
    assert_eq!(provider_native_items(&events), vec![block]);
    // The stop must not be mistaken for a thinking/tool block end.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, StreamEvent::ThinkingEnd | StreamEvent::ToolUseEnd))
    );
}

#[test]
fn client_tool_use_after_server_tool_still_works() {
    let mut state = SseStreamState::default();
    for event in [
        sse(
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {
                "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "q"}
            }}),
        ),
        sse(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
    ] {
        process_sse_event(&event, &mut state, false);
    }
    let events = process_sse_event(
        &sse(
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {
                "type": "tool_use", "id": "toolu_1", "name": "bash", "input": {}
            }}),
        ),
        &mut state,
        false,
    );
    assert!(matches!(
        events.as_slice(),
        [StreamEvent::ToolUseStart { name, .. }] if name == "bash"
    ));
}

fn stored_search_turn() -> Vec<Message> {
    vec![
        Message::user("what is new in rust?"),
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ProviderNative {
                    provider: "anthropic".to_string(),
                    item: json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "rust news"}}),
                },
                ContentBlock::ProviderNative {
                    provider: "anthropic".to_string(),
                    item: json!({"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [
                        {"type": "web_search_result", "url": "https://blog.rust-lang.org/", "title": "Rust Blog", "encrypted_content": "ENC"}
                    ]}),
                },
                ContentBlock::Text {
                    text: "Rust 1.90 shipped.".to_string(),
                    cache_control: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message::user("thanks"),
    ]
}

#[test]
fn stored_server_blocks_replay_verbatim_when_tool_attached() {
    let formatted = jcode_provider_anthropic::format_messages_with_native(
        &stored_search_turn(),
        false,
        &[],
        true,
    );
    let assistant = serde_json::to_value(&formatted[1]).unwrap();
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"][0]["type"], "server_tool_use");
    assert_eq!(assistant["content"][1]["type"], "web_search_tool_result");
    assert_eq!(
        assistant["content"][1]["content"][0]["encrypted_content"],
        "ENC"
    );
    assert_eq!(assistant["content"][2]["type"], "text");
}

#[test]
fn stored_server_blocks_become_text_without_the_tool() {
    let formatted = jcode_provider_anthropic::format_messages_with_native(
        &stored_search_turn(),
        false,
        &[],
        false,
    );
    let assistant = serde_json::to_value(&formatted[1]).unwrap();
    let content = assistant["content"].as_array().unwrap();
    assert!(content.iter().all(|block| block["type"] == "text"));
    let summary = content[0]["text"].as_str().unwrap();
    assert!(summary.contains("rust news"), "{summary}");
    assert!(summary.contains("https://blog.rust-lang.org/"), "{summary}");
    assert!(!serde_json::to_string(&assistant).unwrap().contains("ENC"));
}

#[test]
fn paused_server_tool_turn_is_not_given_a_continuation_user_turn() {
    let mut messages = stored_search_turn();
    messages.pop();
    // Drop the trailing answer text: the turn paused right after the search.
    messages.last_mut().unwrap().content.pop();
    let formatted =
        jcode_provider_anthropic::format_messages_with_native(&messages, false, &[], true);
    let last = serde_json::to_value(formatted.last().unwrap()).unwrap();
    assert_eq!(last["role"], "assistant");
    assert_eq!(last["content"][1]["type"], "web_search_tool_result");

    // Without native replay the text fallback is ordinary assistant text, so
    // the usual prefill repair still applies.
    let formatted =
        jcode_provider_anthropic::format_messages_with_native(&messages, false, &[], false);
    assert_eq!(formatted.last().unwrap().role, "user");
}

#[test]
fn interrupted_turn_after_search_text_still_gets_continuation_user_turn() {
    // A turn that searched and then started answering (e.g. the user
    // interrupted it) ends on text, not a server block. It is not a paused
    // server-tool turn, so the prefill repair must still append a user turn.
    let mut messages = stored_search_turn();
    messages.pop();
    let formatted =
        jcode_provider_anthropic::format_messages_with_native(&messages, false, &[], true);
    assert_eq!(formatted.last().unwrap().role, "user");
    let assistant = serde_json::to_value(&formatted[formatted.len() - 2]).unwrap();
    assert_eq!(assistant["content"][0]["type"], "server_tool_use");
}

#[test]
fn request_tools_place_server_tool_between_eager_and_deferred() {
    let tool = |name: &str, deferred: bool| jcode_provider_anthropic::ApiTool {
        name: name.to_string(),
        description: String::new(),
        input_schema: json!({"type": "object", "properties": {}}),
        cache_control: None,
        defer_loading: deferred,
    };
    let tools = jcode_provider_anthropic::request_tools(
        vec![tool("bash", false), tool("mcp_x", true)],
        vec![json!({"type": "web_search_20250305", "name": "web_search"})],
    )
    .unwrap();
    let value = serde_json::to_value(&tools).unwrap();
    assert_eq!(value[0]["name"], "bash");
    assert_eq!(value[1]["type"], "web_search_20250305");
    assert_eq!(value[2]["name"], "mcp_x");
    assert!(jcode_provider_anthropic::request_tools(vec![], vec![]).is_none());
}
