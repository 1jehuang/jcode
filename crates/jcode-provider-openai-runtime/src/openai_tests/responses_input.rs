fn assistant_tool_use(id: &str, name: &str, input: serde_json::Value) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
            thought_signature: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn user_text(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn response_item_type(item: &serde_json::Value) -> Option<&str> {
    item.get("type").and_then(|v| v.as_str())
}

fn response_item_call_id(item: &serde_json::Value) -> Option<&str> {
    item.get("call_id").and_then(|v| v.as_str())
}

fn function_call_pos(items: &[serde_json::Value], call_id: &str) -> Option<usize> {
    items.iter().position(|item| {
        response_item_type(item) == Some("function_call")
            && response_item_call_id(item) == Some(call_id)
    })
}

fn function_call_output_pos(items: &[serde_json::Value], call_id: &str) -> Option<usize> {
    items.iter().position(|item| {
        response_item_type(item) == Some("function_call_output")
            && response_item_call_id(item) == Some(call_id)
    })
}

fn function_call_outputs(items: &[serde_json::Value], call_id: &str) -> Vec<String> {
    items
        .iter()
        .filter(|item| {
            response_item_type(item) == Some("function_call_output")
                && response_item_call_id(item) == Some(call_id)
        })
        .filter_map(|item| item.get("output").and_then(|v| v.as_str()))
        .map(str::to_string)
        .collect()
}

#[expect(
    clippy::too_many_arguments,
    reason = "test helper mirrors the request builder to keep call sites explicit"
)]
fn build_test_response_request(
    model_id: &str,
    is_chatgpt_mode: bool,
    max_output_tokens: Option<u32>,
    reasoning_effort: Option<&str>,
    service_tier: Option<&str>,
    prompt_cache_key: Option<&str>,
    prompt_cache_retention: Option<&str>,
    native_compaction_threshold: Option<usize>,
) -> serde_json::Value {
    OpenAIProvider::build_response_request(
        model_id,
        "system".to_string(),
        &[],
        &[],
        is_chatgpt_mode,
        max_output_tokens,
        reasoning_effort,
        service_tier,
        prompt_cache_key,
        prompt_cache_retention,
        native_compaction_threshold,
        None,
    )
}

#[test]
fn test_build_responses_input_injects_missing_tool_output() {
    let expected_missing = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
    let messages = vec![
        user_text("hi"),
        assistant_tool_use("call_1", "bash", serde_json::json!({"command": "ls"})),
    ];

    let items = build_responses_input(&messages);
    assert!(function_call_pos(&items, "call_1").is_some());
    assert_eq!(
        function_call_outputs(&items, "call_1"),
        vec![expected_missing]
    );
}

#[test]
fn test_build_responses_input_preserves_tool_output() {
    let messages = vec![
        assistant_tool_use("call_1", "bash", serde_json::json!({"command": "ls"})),
        ChatMessage::tool_result("call_1", "ok", false),
    ];

    let items = build_responses_input(&messages);
    assert_eq!(function_call_outputs(&items, "call_1"), vec!["ok"]);
}

#[test]
fn test_build_responses_input_reorders_early_tool_output() {
    let messages = vec![
        ChatMessage::tool_result("call_1", "ok", false),
        assistant_tool_use("call_1", "bash", serde_json::json!({"command": "ls"})),
    ];

    let items = build_responses_input(&messages);
    let call_pos = function_call_pos(&items, "call_1");
    let output_pos = function_call_output_pos(&items, "call_1");

    assert!(call_pos.is_some());
    assert!(output_pos.is_some());
    assert!(output_pos.unwrap() > call_pos.unwrap());
    assert_eq!(function_call_outputs(&items, "call_1"), vec!["ok"]);
}

#[test]
fn test_build_responses_input_keeps_image_context_after_tool_output() {
    let messages = vec![
        assistant_tool_use(
            "call_1",
            "read",
            serde_json::json!({"file_path": "screenshot.png"}),
        ),
        ChatMessage {
            role: Role::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "Image: screenshot.png\nImage sent to model for vision analysis."
                        .to_string(),
                    is_error: None,
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "ZmFrZQ==".to_string(),
                },
                ContentBlock::Text {
                    text:
                        "[Attached image associated with the preceding tool result: screenshot.png]"
                            .to_string(),
                    cache_control: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
    ];

    let items = build_responses_input(&messages);
    let output_pos = function_call_output_pos(&items, "call_1");
    let mut image_msg_pos = None;

    for (idx, item) in items.iter().enumerate() {
        match response_item_type(item) {
            Some("message") if item.get("role").and_then(|v| v.as_str()) == Some("user") => {
                let Some(content) = item.get("content").and_then(|v| v.as_array()) else {
                    continue;
                };
                let has_image = content
                    .iter()
                    .any(|part| part.get("type").and_then(|v| v.as_str()) == Some("input_image"));
                let has_label = content.iter().any(|part| {
                    part.get("type").and_then(|v| v.as_str()) == Some("input_text")
                        && part
                            .get("text")
                            .and_then(|v| v.as_str())
                            .map(|text| text.contains("screenshot.png"))
                            .unwrap_or(false)
                });
                if has_image && has_label {
                    image_msg_pos = Some(idx);
                }
            }
            _ => {}
        }
    }

    assert_eq!(
        function_call_outputs(&items, "call_1"),
        vec!["Image: screenshot.png\nImage sent to model for vision analysis."]
    );
    assert!(output_pos.is_some(), "expected function call output item");
    assert!(
        image_msg_pos.is_some(),
        "expected follow-up user image message"
    );
    assert!(
        image_msg_pos.unwrap() > output_pos.unwrap(),
        "image context should stay after the tool output"
    );
}

#[test]
fn test_build_responses_input_replaces_oversized_native_compaction_with_text() {
    let oversized = "x"
        .repeat(jcode_base::provider::openai_request::OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS + 1);
    let messages = vec![ChatMessage {
        role: Role::User,
        content: vec![ContentBlock::OpenAICompaction {
            encrypted_content: oversized,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let items = build_responses_input(&messages);

    assert!(
        items
            .iter()
            .all(|item| response_item_type(item) != Some("compaction")),
        "oversized native compaction must not be sent to OpenAI"
    );
    let fallback = items
        .iter()
        .find(|item| response_item_type(item) == Some("message"))
        .expect("fallback text message should be present");
    let text = fallback["content"][0]["text"]
        .as_str()
        .expect("fallback message should contain text");
    assert!(text.contains("OpenAI native compaction state was discarded"));
    assert!(text.contains("safe replay limit"));
}

#[test]
fn test_build_responses_input_injects_only_missing_outputs() {
    let expected_missing = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
    let messages = vec![
        assistant_tool_use("call_a", "bash", serde_json::json!({"command": "pwd"})),
        assistant_tool_use("call_b", "bash", serde_json::json!({"command": "whoami"})),
        ChatMessage::tool_result("call_b", "done", false),
    ];

    let items = build_responses_input(&messages);

    assert_eq!(
        function_call_outputs(&items, "call_a"),
        vec![expected_missing]
    );
    assert_eq!(function_call_outputs(&items, "call_b"), vec!["done"]);
}

#[test]
fn test_openai_retryable_error_patterns() {
    assert!(is_retryable_error(
        "stream disconnected before completion: transport error"
    ));
    assert!(is_retryable_error(
        "falling back from websockets to https transport. stream disconnected before completion"
    ));
    assert!(is_retryable_error(
        "OpenAI HTTPS stream ended before message completion marker"
    ));
    // TLS transport errors must be retryable (previously omitted from the
    // OpenAI-specific list, causing immediate user-facing failures).
    assert!(is_retryable_error(
        "stream error: io error: received fatal alert: badrecordmac"
    ));
    assert!(is_retryable_error("io error: broken pipe (os error 32)"));
    assert!(is_retryable_error("connection aborted"));
}

#[test]
fn test_parse_max_output_tokens_defaults_to_safe_value() {
    assert_eq!(
        OpenAIProvider::parse_max_output_tokens(None),
        Some(DEFAULT_MAX_OUTPUT_TOKENS)
    );
    assert_eq!(
        OpenAIProvider::parse_max_output_tokens(Some("")),
        Some(DEFAULT_MAX_OUTPUT_TOKENS)
    );
}

#[test]
fn test_parse_max_output_tokens_allows_disable_and_override() {
    assert_eq!(OpenAIProvider::parse_max_output_tokens(Some("0")), None);
    assert_eq!(
        OpenAIProvider::parse_max_output_tokens(Some("32768")),
        Some(32768)
    );
    assert_eq!(
        OpenAIProvider::parse_max_output_tokens(Some("not-a-number")),
        Some(DEFAULT_MAX_OUTPUT_TOKENS)
    );
}

#[test]
fn test_build_response_request_for_gpt_5_4_1m_uses_base_model_without_extra_flags() {
    let request = build_test_response_request(
        "gpt-5.4",
        true,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        Some("xhigh"),
        Some("unused"),
        Some("unused"),
        None,
        None,
    );

    assert_eq!(request["model"], serde_json::json!("gpt-5.4"));
    assert!(request.get("model_context_window").is_none());
    assert!(request.get("max_output_tokens").is_none());
    assert_eq!(request["prompt_cache_key"], "unused");
    assert!(request.get("prompt_cache_retention").is_none());
    assert_eq!(
        request["reasoning"],
        serde_json::json!({ "effort": "xhigh", "summary": "auto" })
    );
    assert_eq!(request["service_tier"], serde_json::json!("unused"));
    assert!(
        request["tools"]
            .as_array()
            .expect("tools should be an array")
            .contains(&serde_json::json!({ "type": "image_generation" }))
    );
}

#[test]
fn test_build_response_request_omits_image_generation_for_codex_models() {
    // Codex models reject the hosted image_generation tool, so it must not be
    // attached even in ChatGPT mode (issue #369).
    let request = build_test_response_request(
        "gpt-5.3-codex",
        true,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );

    assert!(
        !request["tools"]
            .as_array()
            .expect("tools should be an array")
            .contains(&serde_json::json!({ "type": "image_generation" })),
        "codex models must not receive the image_generation tool"
    );
}

#[test]
fn test_build_response_request_keeps_image_generation_for_non_codex_chatgpt_models() {
    let request = build_test_response_request(
        "gpt-5.5",
        true,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );

    assert!(
        request["tools"]
            .as_array()
            .expect("tools should be an array")
            .contains(&serde_json::json!({ "type": "image_generation" })),
        "non-codex ChatGPT models should still receive image_generation"
    );
}

#[test]
fn test_build_response_request_omits_long_context_for_plain_gpt_5_4() {
    let request = build_test_response_request(
        "gpt-5.4",
        true,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );

    assert!(request.get("model_context_window").is_none());
}

#[test]
fn test_build_response_request_uses_default_30m_cache_for_gpt_5_6() {
    let request = build_test_response_request(
        "gpt-5.6-sol",
        false,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );

    assert!(request.get("prompt_cache_retention").is_none());
    assert!(request.get("prompt_cache_options").is_none());
}

#[test]
fn test_build_response_request_defaults_extended_cache_retention_for_gpt_5_5() {
    let request = build_test_response_request(
        "gpt-5.5",
        false,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        None,
        None,
    );

    assert_eq!(request["prompt_cache_retention"], serde_json::json!("24h"));
    assert_eq!(
        request["max_output_tokens"],
        serde_json::json!(DEFAULT_MAX_OUTPUT_TOKENS)
    );
}

#[test]
fn test_build_response_request_respects_configured_cache_retention() {
    let request = build_test_response_request(
        "gpt-5.4",
        false,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        None,
        None,
        None,
        Some("in_memory"),
        None,
    );

    assert_eq!(
        request["prompt_cache_retention"],
        serde_json::json!("in_memory")
    );
}

#[test]
fn test_build_response_request_cache_policy_matches_shared_ttl() {
    for (model, retention, expected_field, expected_ttl) in [
        ("gpt-5.6-sol", None, None, 1800),
        ("gpt-5.6-sol", Some("24h"), None, 1800),
        ("gpt-6-astra", Some("in_memory"), None, 1800),
        ("gpt-5.5", None, Some("24h"), 1800),
        ("gpt-5.4", Some("in_memory"), Some("in_memory"), 300),
        ("gpt-4.1", None, Some("24h"), 1800),
        ("gpt-5-codex", None, Some("24h"), 1800),
        ("gpt-4o", None, None, 300),
    ] {
        let request =
            build_test_response_request(model, false, None, None, None, None, retention, None);
        assert_eq!(
            request
                .get("prompt_cache_retention")
                .and_then(Value::as_str),
            expected_field,
            "{model}"
        );
        assert_eq!(
            jcode_base::provider::openai::prompt_cache_ttl_for_model(Some(model), retention),
            expected_ttl,
            "{model}"
        );
    }
}

#[test]
fn test_oauth_requests_never_send_api_cache_retention_controls() {
    for model in ["gpt-5.4", "gpt-5.5", "gpt-5.6-sol", "gpt-6-astra"] {
        for retention in [None, Some("24h"), Some("in_memory")] {
            let request = build_test_response_request(
                model,
                true,
                None,
                None,
                None,
                Some("cache-key"),
                retention,
                None,
            );
            assert_eq!(request["prompt_cache_key"], "cache-key", "{model}");
            assert!(request.get("prompt_cache_retention").is_none(), "{model}");
            assert!(request.get("prompt_cache_options").is_none(), "{model}");
        }
    }
}

/// The whole point of the native path: a schema handed to the request builder
/// must leave as the Responses API's `text.format` json_schema block with
/// `strict: true`. Without that field the request is prompt-and-pray, which is
/// indistinguishable in a passing test unless the field itself is asserted.
#[test]
fn test_build_response_request_emits_native_json_schema_format() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"title": {"type": "string"}},
        "required": ["title"],
        "additionalProperties": false,
    });
    let request = OpenAIProvider::build_response_request(
        "gpt-5.6-sol",
        "system".to_string(),
        &[],
        &[],
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&schema),
    );

    let format = &request["text"]["format"];
    assert_eq!(format["type"], serde_json::json!("json_schema"));
    assert_eq!(format["strict"], serde_json::json!(true));
    assert_eq!(format["schema"]["properties"]["title"]["type"], "string");
}

/// A turn with no schema must be byte-identical to today's request: the native
/// field is opt-in, so every ordinary turn keeps its existing shape.
#[test]
fn test_build_response_request_omits_text_format_without_schema() {
    let request =
        build_test_response_request("gpt-5.6-sol", false, None, None, None, None, None, None);
    assert!(
        request.get("text").is_none(),
        "unschemaed turn gained a text field: {request}"
    );
}

/// The graceful-fallback decision lives in `set_response_schema`'s return
/// value: `false` is what sends the caller back to its own validate-and-retry
/// loop. A schema OpenAI cannot enforce strictly must therefore be refused
/// rather than sent in a mode that only asks for JSON, and an accepted one must
/// come back normalized (so a strict request is never rejected for a missing
/// `required`/`additionalProperties`).
#[test]
fn test_set_response_schema_refuses_schemas_strict_mode_cannot_enforce() {
    let creds = CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    };
    let provider = OpenAIProvider::new(creds);

    // `additionalProperties: true` is legal JSON Schema but not strictly
    // enforceable, so the gate must close.
    let unenforceable = serde_json::json!({
        "type": "object",
        "properties": {"title": {"type": "string"}},
        "additionalProperties": true,
    });
    assert!(
        !provider.set_response_schema(&unenforceable),
        "an unenforceable schema was accepted, so the caller will never fall back"
    );

    let enforceable = serde_json::json!({
        "type": "object",
        "properties": {"title": {"type": "string"}},
    });
    assert!(
        provider.set_response_schema(&enforceable),
        "a strictly enforceable schema was refused, so the native path is dead code"
    );
    let stored = provider
        .response_schema
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .expect("accepted schema was not stored");
    assert_eq!(
        stored["additionalProperties"],
        serde_json::json!(false),
        "accepted schema was stored un-normalized: {stored}"
    );
    assert_eq!(stored["required"], serde_json::json!(["title"]));

    provider.clear_response_schema();
    assert!(
        provider
            .response_schema
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none(),
        "clear left the schema attached, so the next turn would be constrained"
    );
}

/// The request the runtime actually sends, built from provider state rather
/// than from a directly-passed argument. This is the only assertion that
/// covers the whole emit path: `set_response_schema` -> provider field ->
/// `response_request_for_model` -> serialized body.
#[test]
fn test_generated_request_body_carries_json_schema_format_from_provider_state() {
    let _guard = jcode_base::storage::lock_test_env();
    let creds = CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    };
    let provider = OpenAIProvider::new(creds);

    let baseline = provider.response_request_for_model(
        "gpt-5.6-sol",
        &[serde_json::json!({"role": "user", "content": "hi"})],
        &[],
        "system",
        false,
    );
    assert!(
        baseline.get("text").is_none(),
        "unstructured turn already carries a text field: {baseline}"
    );

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "title": {"type": "string"},
            "tags": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["title"],
    });
    assert!(provider.set_response_schema(&schema));

    let request = provider.response_request_for_model(
        "gpt-5.6-sol",
        &[serde_json::json!({"role": "user", "content": "hi"})],
        &[],
        "system",
        false,
    );

    let format = &request["text"]["format"];
    assert_eq!(
        format["type"],
        serde_json::json!("json_schema"),
        "json_schema was not populated in the generated body: {request}"
    );
    assert_eq!(format["strict"], serde_json::json!(true));
    assert_eq!(format["name"], serde_json::json!("structured_output"));
    assert_eq!(
        format["schema"]["properties"]["title"]["type"],
        serde_json::json!("string"),
        "the caller's schema did not survive into the body: {request}"
    );
    assert_eq!(
        format["schema"]["properties"]["tags"]["items"]["type"],
        serde_json::json!("string"),
        "a nested schema node was lost: {request}"
    );
    // Strict mode requires every declared property listed as required and no
    // open object, so normalization must already have expanded `required`
    // beyond what the caller wrote (they listed only `title`).
    assert_eq!(
        format["schema"]["required"],
        serde_json::json!(["tags", "title"]),
        "strict normalization did not expand required: {request}"
    );
    assert_eq!(
        format["schema"]["additionalProperties"],
        serde_json::json!(false)
    );

    // Per-turn state: after the turn the next request must be plain again, or
    // every later completion would be silently constrained.
    provider.clear_response_schema();
    let after = provider.response_request_for_model(
        "gpt-5.6-sol",
        &[serde_json::json!({"role": "user", "content": "hi"})],
        &[],
        "system",
        false,
    );
    assert!(
        after.get("text").is_none(),
        "the schema leaked into the next turn: {after}"
    );
}
