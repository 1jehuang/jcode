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
    assert_eq!(
        request["reasoning"],
        serde_json::json!({"effort": "high", "summary": "auto"})
    );
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
        Some("flex"),
        Some("jcode-test-cache"),
        Some("24h"),
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
    if let Some(service_tier) = base_request.get("service_tier") {
        continuation["service_tier"] = service_tier.clone();
    }
    if let Some(prompt_cache_key) = base_request.get("prompt_cache_key") {
        continuation["prompt_cache_key"] = prompt_cache_key.clone();
    }
    if let Some(prompt_cache_retention) = base_request.get("prompt_cache_retention") {
        continuation["prompt_cache_retention"] = prompt_cache_retention.clone();
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
    assert_eq!(continuation["service_tier"], "flex");
    assert_eq!(continuation["prompt_cache_key"], "jcode-test-cache");
    assert_eq!(continuation["prompt_cache_retention"], "24h");
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

#[test]
fn test_websocket_continuation_delta_skips_reasoning_items() {
    let input = vec![
        serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "first" }]
        }),
        serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "ok" }]
        }),
        serde_json::json!({
            "type": "reasoning",
            "id": "rs_duplicate_from_previous_response",
            "summary": []
        }),
        serde_json::json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "done"
        }),
        serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "continue" }]
        }),
    ];

    let (delta, skipped_reasoning) = persistent_ws_incremental_items(&input, 2);

    assert_eq!(skipped_reasoning, 1);
    assert_eq!(delta.len(), 2);
    assert!(
        delta
            .iter()
            .all(|item| item.get("type").and_then(|value| value.as_str()) != Some("reasoning")),
        "previous_response_id deltas must not replay rs_* reasoning items"
    );
    assert_eq!(delta[0]["type"], "function_call_output");
    assert_eq!(delta[1]["type"], "message");
}

#[test]
fn max_and_swarm_efforts_are_preserved_at_the_strongest_api_level() {
    let provider = OpenAIProvider::new_browser_only();
    // The swarm sentinel is a valid stored effort...
    assert_eq!(
        OpenAIProvider::normalize_reasoning_effort("swarm").as_deref(),
        Some("swarm")
    );
    // ...but maps to the strongest real effort when building the request.
    assert_eq!(
        provider.api_reasoning_effort(Some("swarm")).as_deref(),
        Some("max")
    );
    assert_eq!(
        OpenAIProvider::normalize_reasoning_effort("max").as_deref(),
        Some("max")
    );
    assert_eq!(
        OpenAIProvider::normalize_reasoning_effort("minimal").as_deref(),
        Some("minimal")
    );
    assert_eq!(
        provider.api_reasoning_effort(Some("max")).as_deref(),
        Some("max")
    );
    assert_eq!(
        provider.api_reasoning_effort(Some("high")).as_deref(),
        Some("high")
    );
    assert_eq!(provider.api_reasoning_effort(None), None);

    let request = OpenAIProvider::build_response_request(
        "gpt-5.4",
        "system".to_string(),
        &[],
        &[],
        false,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        provider.api_reasoning_effort(Some("swarm")).as_deref(),
        None,
        None,
        None,
        None,
    );
    assert_eq!(request["reasoning"]["effort"], serde_json::json!("max"));
    assert_eq!(request["reasoning"]["summary"], serde_json::json!("auto"));
}

/// GPT-5.6 Sol is jcode's default OpenAI model and defaults to `low`
/// reasoning effort when the user has not configured one. The default must
/// surface in `reasoning_effort()` (status/UI), resolve per-model (other
/// models keep their API-side default), and land in the request payload.
#[test]
fn gpt_5_6_sol_defaults_to_low_reasoning_effort() {
    assert_eq!(
        OpenAIProvider::default_reasoning_effort_for_model("gpt-5.6-sol").as_deref(),
        Some("low"),
    );
    // Long-context and cased variants resolve through canonicalization.
    assert_eq!(
        OpenAIProvider::default_reasoning_effort_for_model("GPT-5.6-Sol").as_deref(),
        Some("low"),
    );
    // Other models keep the model's own default (no forced effort).
    assert_eq!(
        OpenAIProvider::default_reasoning_effort_for_model("gpt-5.5"),
        None
    );
    assert_eq!(
        OpenAIProvider::default_reasoning_effort_for_model("gpt-5.6"),
        None,
        "the clean 5.6 release is not the Sol quality profile"
    );

    // With no stored effort, the surfaced status reflects the Sol default.
    let _guard = jcode_base::storage::lock_test_env();
    jcode_base::auth::codex::set_active_account_override(Some("sol-low-default-test".to_string()));
    jcode_base::provider::populate_account_models(vec!["gpt-5.6-sol".to_string()]);
    let provider = OpenAIProvider::new(CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    });
    provider.set_model("gpt-5.6-sol").unwrap();
    *provider
        .reasoning_effort
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    assert_eq!(provider.reasoning_effort().as_deref(), Some("low"));

    // An explicit user override still wins over the Sol default.
    provider.set_reasoning_effort("high").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));
    jcode_base::auth::codex::set_active_account_override(None);

    // The default drives the request payload.
    let request = OpenAIProvider::build_response_request(
        "gpt-5.6-sol",
        "system".to_string(),
        &[],
        &[],
        false,
        Some(DEFAULT_MAX_OUTPUT_TOKENS),
        OpenAIProvider::default_reasoning_effort_for_model("gpt-5.6-sol").as_deref(),
        None,
        None,
        None,
        None,
    );
    assert_eq!(request["reasoning"]["effort"], serde_json::json!("low"));
}

/// Regression guard for issue #203: prompt overlays (`AGENTS.md`,
/// `.jcode/prompt-overlay.md`) were silently dropped in ChatGPT/Codex OAuth
/// mode because the provider replaced the caller's fully-built system prompt
/// with a hardcoded ChatGPT instruction block. The composed system prompt must
/// reach the Responses API verbatim in *both* ChatGPT-OAuth and API-key modes,
/// otherwise user boot instructions are counted in prompt accounting but never
/// actually sent.
#[test]
fn test_build_response_request_passes_system_prompt_through_verbatim() {
    let system = "BASE PROMPT\n\n# Project Instructions (AGENTS.md)\nmarker-xyz";

    for is_chatgpt_mode in [true, false] {
        let request = OpenAIProvider::build_response_request(
            "gpt-5.4",
            system.to_string(),
            &[],
            &[],
            is_chatgpt_mode,
            Some(DEFAULT_MAX_OUTPUT_TOKENS),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            request["instructions"],
            serde_json::json!(system),
            "system prompt must not be rewritten (is_chatgpt_mode={is_chatgpt_mode})"
        );
    }
}
