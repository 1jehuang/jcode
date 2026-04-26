#[test]
fn test_build_response_request_includes_stream_for_http() {
    let request = OpenAIProvider::build_response_request(
        "gpt-5.4",
        "system".to_string(),
        &[],
        &[],
        false,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );
    assert_eq!(request["stream"], serde_json::json!(true));
    assert_eq!(request["store"], serde_json::json!(false));
}

#[test]
fn test_websocket_payload_strips_stream_and_background() {
    let mut request = OpenAIProvider::build_response_request(
        "gpt-5.4",
        "system".to_string(),
        &[serde_json::json!({"role": "user", "content": "hello"})],
        &[],
        false,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );

    assert_eq!(request["stream"], serde_json::json!(true));

    request["background"] = serde_json::json!(true);

    let obj = request.as_object_mut().expect("request is object");
    obj.insert(
        "type".to_string(),
        serde_json::Value::String("response.create".to_string()),
    );
    obj.remove("stream");
    obj.remove("background");

    assert!(
        request.get("stream").is_none(),
        "stream must be stripped for WebSocket payloads"
    );
    assert!(
        request.get("background").is_none(),
        "background must be stripped for WebSocket payloads"
    );
    assert_eq!(request["type"], serde_json::json!("response.create"));
}

#[test]
fn test_websocket_payload_preserves_required_fields() {
    let mut request = OpenAIProvider::build_response_request(
        "gpt-5.4",
        "system prompt".to_string(),
        &[serde_json::json!({"role": "user", "content": "hello"})],
        &[serde_json::json!({"type": "function", "name": "bash"})],
        false,
        Some(16384),
        Some("high"),
        None,
        None,
        None,
        None,
    );

    let obj = request.as_object_mut().expect("request is object");
    obj.insert(
        "type".to_string(),
        serde_json::Value::String("response.create".to_string()),
    );
    obj.remove("stream");
    obj.remove("background");

    assert_eq!(request["type"], "response.create");
    assert_eq!(request["model"], "gpt-5.4");
    assert_eq!(request["instructions"], "system prompt");
    assert!(request["input"].is_array());
    assert!(request["tools"].is_array());
    assert_eq!(request["max_output_tokens"], serde_json::json!(16384));
    assert_eq!(request["reasoning"], serde_json::json!({"effort": "high"}));
    assert_eq!(request["tool_choice"], "auto");
}

#[test]
fn test_websocket_continuation_request_excludes_transport_fields() {
    let base_request = OpenAIProvider::build_response_request(
        "gpt-5.4",
        "system".to_string(),
        &[],
        &[serde_json::json!({"type": "function", "name": "bash"})],
        false,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        Some(160_000),
    );

    let mut continuation = serde_json::json!({
        "type": "response.create",
        "previous_response_id": "resp_abc123",
        "input": [{"role": "user", "content": "follow up"}],
    });

    if let Some(model) = base_request.get("model") {
        continuation["model"] = model.clone();
    }
    if let Some(tools) = base_request.get("tools") {
        continuation["tools"] = tools.clone();
    }
    if let Some(instructions) = base_request.get("instructions") {
        continuation["instructions"] = instructions.clone();
    }
    if let Some(context_management) = base_request.get("context_management") {
        continuation["context_management"] = context_management.clone();
    }
    continuation["store"] = serde_json::json!(false);
    continuation["parallel_tool_calls"] = serde_json::json!(false);

    assert!(
        continuation.get("stream").is_none(),
        "continuation request must not include stream"
    );
    assert!(
        continuation.get("background").is_none(),
        "continuation request must not include background"
    );
    assert_eq!(continuation["type"], "response.create");
    assert_eq!(continuation["previous_response_id"], "resp_abc123");
    assert_eq!(continuation["model"], "gpt-5.4");
    assert_eq!(
        continuation["context_management"],
        serde_json::json!([
            {
                "type": "compaction",
                "compact_threshold": 160_000,
            }
        ])
    );
}
