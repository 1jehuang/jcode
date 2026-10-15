use super::*;

fn make_metadata_provider() -> CopilotApiProvider {
    let provider = make_test_provider(vec!["gpt-6.1-sol".into(), "claude-sonnet-5.5".into()]);
    let models: Vec<copilot_auth::CopilotModelInfo> = serde_json::from_value(json!([
        {
            "id": "gpt-6.1-sol",
            "supported_endpoints": ["/responses"],
            "capabilities": {
                "limits": {"max_context_window_tokens": 400000, "max_prompt_tokens": 272000, "max_output_tokens": 128000},
                "supports": {"reasoning_effort": ["none", "low", "medium", "high", "xhigh", "max"], "vision": true, "tool_calls": true, "parallel_tool_calls": true, "streaming": true}
            }
        },
        {
            "id": "claude-sonnet-5.5",
            "supported_endpoints": ["/chat/completions"],
            "capabilities": {
                "limits": {"max_context_window_tokens": 328000, "max_prompt_tokens": 200000, "max_output_tokens": 128000},
                "supports": {"reasoning_effort": ["low", "medium", "high", "xhigh", "max"], "vision": true, "tool_calls": true, "streaming": true}
            }
        }
    ])).unwrap();
    provider.install_model_metadata(
        models
            .iter()
            .map(|model| (model.id.clone(), model.metadata()))
            .collect(),
    );
    provider.set_model("gpt-6.1-sol").unwrap();
    provider
}

#[test]
fn should_use_enterprise_catalog_context_window() {
    let provider = make_metadata_provider();
    assert_eq!(provider.context_window(), 400_000);
}

#[test]
fn should_budget_prompt_below_total_context_window() {
    let provider = make_metadata_provider();
    assert_eq!(provider.input_token_limit(), 272_000);
}

#[test]
fn should_use_enterprise_catalog_output_limit() {
    let provider = make_metadata_provider();
    assert_eq!(provider.max_output_tokens_for("gpt-6.1-sol"), 128_000);
}

#[test]
fn should_expose_only_advertised_claude_reasoning_levels() {
    let provider = make_metadata_provider();
    provider.set_model("claude-sonnet-5.5").unwrap();
    assert_eq!(
        provider.available_efforts(),
        vec!["low", "medium", "high", "xhigh", "max"]
    );
}

#[test]
fn should_change_prompt_budget_when_model_changes() {
    let provider = make_metadata_provider();
    provider.set_model("claude-sonnet-5.5").unwrap();
    assert_eq!(provider.input_token_limit(), 200_000);
}

#[test]
fn should_support_images_advertised_by_catalog() {
    let provider = make_metadata_provider();
    assert!(provider.supports_image_input());
}

#[test]
fn should_preserve_metadata_in_fork() {
    let provider = make_metadata_provider();
    assert_eq!(
        provider.fork().model_metadata("gpt-6.1-sol"),
        provider.model_metadata("gpt-6.1-sol")
    );
}

#[test]
fn should_restore_metadata_from_serialized_catalog() {
    let provider = make_metadata_provider();
    let metadata = provider.model_metadata.read().unwrap().clone();
    let catalog = PersistedCatalog {
        models: provider.available_models_display(),
        fetched_at_rfc3339: Utc::now().to_rfc3339(),
        model_metadata: metadata.clone(),
    };
    let restored: PersistedCatalog =
        serde_json::from_slice(&serde_json::to_vec(&catalog).unwrap()).unwrap();
    let restored_provider = make_test_provider(restored.models);
    restored_provider.install_model_metadata(restored.model_metadata);
    assert_eq!(
        restored_provider.model_metadata("gpt-6.1-sol"),
        metadata.get("gpt-6.1-sol").cloned()
    );
}

#[test]
fn should_include_image_data_in_native_chat_payload() {
    let messages = vec![make_msg(
        Role::User,
        vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "aW1hZ2U=".into(),
        }],
    )];
    assert_eq!(
        CopilotApiProvider::build_messages("", &messages)[0]["content"][0]["image_url"]["url"],
        "data:image/png;base64,aW1hZ2U="
    );
}
fn make_test_provider(fetched: Vec<String>) -> CopilotApiProvider {
    CopilotApiProvider {
        client: jcode_base::provider::shared_http_client(),
        endpoints: copilot_auth::CopilotEndpoints::from_enterprise_url(None).unwrap(),
        model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        github_token: "test-token".to_string(),
        bearer_token: Arc::new(tokio::sync::RwLock::new(None)),
        fetched_models: Arc::new(RwLock::new(fetched)),
        catalog_source: Arc::new(RwLock::new(CatalogSource::Live)),
        session_id: "test-session".to_string(),
        machine_id: "test-machine".to_string(),
        init_ready: Arc::new(tokio::sync::Notify::new()),
        init_done: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        premium_mode: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        user_turn_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        reasoning_effort: Arc::new(RwLock::new(None)),
        model_efforts: Arc::new(RwLock::new(Default::default())),
        model_endpoints: Arc::new(RwLock::new(Default::default())),
        model_metadata: Arc::new(RwLock::new(Default::default())),
        created_at: std::time::Instant::now(),
    }
}

#[test]
fn should_reject_unsupported_none_effort_for_claude() {
    let provider = make_metadata_provider();
    provider.set_model("claude-sonnet-5.5").unwrap();
    assert!(provider.set_reasoning_effort("none").is_err());
}

#[test]
fn should_clear_unsupported_effort_when_switching_models() {
    let provider = make_metadata_provider();
    provider.set_reasoning_effort("none").unwrap();
    provider.set_model("claude-sonnet-5.5").unwrap();
    assert_ne!(provider.reasoning_effort(), Some("none".into()));
}

#[tokio::test]
async fn should_reject_non_streaming_model_before_network_request() {
    let provider = make_metadata_provider();
    let mut metadata = provider.model_metadata.read().unwrap().clone();
    metadata.get_mut("gpt-6.1-sol").unwrap().supports_streaming = Some(false);
    provider.install_model_metadata(metadata);

    assert!(matches!(
        provider.complete(&[], &[], "", None).await,
        Err(error) if error.to_string().contains("does not support streaming")
    ));
}

#[test]
fn should_keep_public_chat_endpoint_unchanged() {
    let provider = make_test_provider(Vec::new());

    assert_eq!(
        provider.inference_url(false),
        "https://api.githubcopilot.com/chat/completions"
    );
}

#[test]
fn should_keep_public_responses_endpoint_unchanged() {
    let provider = make_test_provider(Vec::new());

    assert_eq!(
        provider.inference_url(true),
        "https://api.githubcopilot.com/responses"
    );
}

#[test]
fn should_route_enterprise_chat_to_captured_endpoint() {
    let mut provider = make_test_provider(Vec::new());
    provider.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("hiqfi.ghe.com")).unwrap();

    assert_eq!(
        provider.inference_url(false),
        "https://copilot-api.hiqfi.ghe.com/chat/completions"
    );
}

#[test]
fn should_route_enterprise_responses_to_captured_endpoint() {
    let mut provider = make_test_provider(Vec::new());
    provider.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("hiqfi.ghe.com")).unwrap();

    assert_eq!(
        provider.inference_url(true),
        "https://copilot-api.hiqfi.ghe.com/responses"
    );
}

#[test]
fn should_keep_public_catalog_cache_filename_unchanged() {
    let provider = make_test_provider(Vec::new());

    assert_eq!(
        provider.persisted_catalog_filename(),
        "copilot_models_cache.json"
    );
}

#[test]
fn should_namespace_enterprise_catalog_cache_by_host() {
    let mut provider = make_test_provider(Vec::new());
    provider.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("hiqfi.ghe.com")).unwrap();

    assert_eq!(
        provider.persisted_catalog_filename(),
        "copilot_models_cache.hiqfi.ghe.com.json"
    );
}

#[test]
fn should_isolate_catalog_caches_between_enterprise_hosts() {
    let mut first = make_test_provider(Vec::new());
    first.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("first.ghe.com")).unwrap();
    let mut second = make_test_provider(Vec::new());
    second.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("second.ghe.com")).unwrap();

    assert_ne!(
        first.persisted_catalog_filename(),
        second.persisted_catalog_filename()
    );
}

#[test]
fn should_share_catalog_cache_for_normalized_enterprise_host() {
    let mut first = make_test_provider(Vec::new());
    first.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("hiqfi.ghe.com")).unwrap();
    let mut second = make_test_provider(Vec::new());
    second.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("https://HIQFI.GHE.COM/"))
            .unwrap();

    assert_eq!(
        first.persisted_catalog_filename(),
        second.persisted_catalog_filename()
    );
}

#[test]
fn should_preserve_selected_enterprise_model_in_live_catalog() {
    let mut provider = make_test_provider(Vec::new());
    provider.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("hiqfi.ghe.com")).unwrap();
    provider.set_model("gpt-6.1-sol").unwrap();
    let models: Vec<copilot_auth::CopilotModelInfo> = serde_json::from_value(json!([
        {"id": "claude-sonnet-5.5", "model_picker_enabled": true},
        {"id": "gpt-6.1-sol", "model_picker_enabled": true}
    ]))
    .unwrap();

    assert_eq!(provider.choose_catalog_default(&models), "gpt-6.1-sol");
}

#[test]
fn should_choose_available_enterprise_picker_model_when_legacy_default_is_missing() {
    let mut provider = make_test_provider(Vec::new());
    provider.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("hiqfi.ghe.com")).unwrap();
    let models: Vec<copilot_auth::CopilotModelInfo> = serde_json::from_value(json!([
        {"id": "hidden-model"},
        {"id": "claude-sonnet-5.5", "model_picker_enabled": true}
    ]))
    .unwrap();

    assert_eq!(
        provider.choose_catalog_default(&models),
        "claude-sonnet-5.5"
    );
}

#[test]
fn should_choose_available_enterprise_model_when_no_picker_models_exist() {
    let mut provider = make_test_provider(Vec::new());
    provider.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("hiqfi.ghe.com")).unwrap();
    let models: Vec<copilot_auth::CopilotModelInfo> =
        serde_json::from_value(json!([{"id": "gpt-6.1-sol"}])).unwrap();

    assert_eq!(provider.choose_catalog_default(&models), "gpt-6.1-sol");
}

#[test]
fn should_preserve_public_legacy_default_selection() {
    let provider = make_test_provider(Vec::new());
    provider.set_model("gpt-6.1-sol").unwrap();
    let models: Vec<copilot_auth::CopilotModelInfo> = serde_json::from_value(json!([
        {"id": "gpt-6.1-sol", "model_picker_enabled": true},
        {"id": "claude-opus-4.6", "model_picker_enabled": true}
    ]))
    .unwrap();

    assert_eq!(provider.choose_catalog_default(&models), "claude-opus-4.6");
}

#[test]
fn should_refresh_expired_bearer_at_captured_enterprise_endpoint() {
    use std::io::{Read, Write};

    const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    const ACCEPT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);
    const RESPONSE_BODY: &str = r#"{"token":"refreshed-test-bearer","expires_at":4102444800}"#;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let request = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + REQUEST_TIMEOUT;
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "runtime did not contact the captured endpoint"
                    );
                    std::thread::sleep(ACCEPT_POLL_INTERVAL);
                }
                Err(error) => panic!("failed to accept token exchange: {error}"),
            }
        };
        socket.set_read_timeout(Some(REQUEST_TIMEOUT)).unwrap();
        let mut received = Vec::new();
        let mut buffer = [0; 1024];
        while !received.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).unwrap();
            assert_ne!(count, 0, "token exchange closed before sending headers");
            received.extend_from_slice(&buffer[..count]);
        }
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            RESPONSE_BODY.len(),
            RESPONSE_BODY
        )
        .unwrap();
        String::from_utf8(received).unwrap()
    });
    let mut provider = make_test_provider(Vec::new());
    provider.endpoints =
        copilot_auth::CopilotEndpoints::from_enterprise_url(Some("hiqfi.ghe.com")).unwrap();
    provider.endpoints.token_url = format!("http://{address}/api/v3/copilot_internal/v2/token");
    provider.client = reqwest::Client::builder()
        .no_proxy()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap();
    *provider.bearer_token.blocking_write() = Some(copilot_auth::CopilotApiToken {
        token: "expired-test-bearer".to_string(),
        expires_at: 0,
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(provider.get_bearer_token()).unwrap();

    assert_eq!(
        request.join().unwrap().lines().next(),
        Some("GET /api/v3/copilot_internal/v2/token HTTP/1.1")
    );
}

#[test]
fn available_models_display_returns_fetched_when_populated() {
    let fetched = vec![
        "claude-opus-4.6".to_string(),
        "claude-sonnet-4.6".to_string(),
        "gpt-5.3-codex".to_string(),
        "gemini-3-pro-preview".to_string(),
    ];
    let provider = make_test_provider(fetched.clone());
    let display = provider.available_models_display();
    assert_eq!(display, fetched);
}

#[test]
fn available_models_display_returns_fallback_when_empty() {
    let provider = make_test_provider(Vec::new());
    let display = provider.available_models_display();
    let expected: Vec<String> = FALLBACK_MODELS.iter().map(|m| m.to_string()).collect();
    assert_eq!(display, expected);
}

#[test]
fn available_models_static_always_returns_fallback() {
    let fetched = vec!["claude-opus-4.6".to_string(), "gpt-5.3-codex".to_string()];
    let provider = make_test_provider(fetched);
    let static_models = provider.available_models();
    let expected: Vec<&str> = FALLBACK_MODELS.to_vec();
    assert_eq!(static_models, expected);
}

#[test]
fn set_model_accepts_any_model_id() {
    let provider = make_test_provider(Vec::new());
    assert!(provider.set_model("claude-opus-4.6").is_ok());
    assert_eq!(provider.model(), "claude-opus-4.6");

    assert!(provider.set_model("some-new-model-2026").is_ok());
    assert_eq!(provider.model(), "some-new-model-2026");
}

#[test]
fn set_model_rejects_empty() {
    let provider = make_test_provider(Vec::new());
    assert!(provider.set_model("").is_err());
    assert!(provider.set_model("   ").is_err());
}

#[test]
fn gpt5_copilot_models_use_max_completion_tokens() {
    assert_eq!(
        CopilotApiProvider::max_token_parameter_for_model("gpt-5.4"),
        "max_completion_tokens"
    );
    assert_eq!(
        CopilotApiProvider::max_token_parameter_for_model(" GPT-5.4-pro "),
        "max_completion_tokens"
    );
    assert_eq!(
        CopilotApiProvider::max_token_parameter_for_model("gpt-5.3-codex"),
        "max_completion_tokens"
    );
}

#[test]
fn non_gpt5_copilot_models_keep_max_tokens() {
    assert_eq!(
        CopilotApiProvider::max_token_parameter_for_model("claude-sonnet-4.6"),
        "max_tokens"
    );
    assert_eq!(
        CopilotApiProvider::max_token_parameter_for_model("gemini-3-pro-preview"),
        "max_tokens"
    );
    assert_eq!(
        CopilotApiProvider::max_token_parameter_for_model("gpt-4.1"),
        "max_tokens"
    );
}

#[test]
fn only_gpt_5_6_family_uses_responses_api() {
    assert!(copilot_model_uses_responses_api("gpt-5.6-terra"));
    assert!(copilot_model_uses_responses_api("gpt-5.6-sol"));
    assert!(copilot_model_uses_responses_api("gpt-5.6-luna"));
    assert!(!copilot_model_uses_responses_api("gpt-5.4"));
    assert!(!copilot_model_uses_responses_api("claude-sonnet-4.6"));
    assert_eq!(copilot_api_path(true), "responses");
    assert_eq!(copilot_api_path(false), "chat/completions");
}

#[test]
fn should_route_response_only_catalog_model_to_responses() {
    let catalog = std::collections::HashMap::from([(
        "gpt-6.1-sol".to_string(),
        vec!["/responses".to_string()],
    )]);

    assert!(copilot_model_uses_responses_api_with_catalog(
        &catalog,
        "gpt-6.1-sol"
    ));
}

#[test]
fn should_route_chat_only_catalog_model_to_chat_despite_name_fallback() {
    let catalog = std::collections::HashMap::from([(
        "gpt-5.6-sol".to_string(),
        vec!["/chat/completions".to_string()],
    )]);

    assert!(!copilot_model_uses_responses_api_with_catalog(
        &catalog,
        "gpt-5.6-sol"
    ));
}

#[test]
fn should_keep_legacy_routing_when_both_endpoints_are_advertised() {
    let catalog = std::collections::HashMap::from([(
        "gpt-5.6-sol".to_string(),
        vec!["/responses".to_string(), "/chat/completions".to_string()],
    )]);

    assert!(copilot_model_uses_responses_api_with_catalog(
        &catalog,
        "gpt-5.6-sol"
    ));
}

#[test]
fn should_keep_legacy_routing_when_endpoint_metadata_is_empty() {
    let catalog = std::collections::HashMap::from([("gpt-5.6-sol".to_string(), Vec::new())]);

    assert!(copilot_model_uses_responses_api_with_catalog(
        &catalog,
        "gpt-5.6-sol"
    ));
}

#[test]
fn should_keep_claude_on_chat_when_catalog_is_unavailable() {
    let catalog = std::collections::HashMap::new();

    assert!(!copilot_model_uses_responses_api_with_catalog(
        &catalog,
        "claude-sonnet-5.5"
    ));
}

#[test]
fn should_accept_advertised_endpoint_without_leading_slash() {
    let catalog = std::collections::HashMap::from([(
        "gpt-6.1-sol".to_string(),
        vec!["responses".to_string()],
    )]);

    assert!(copilot_model_uses_responses_api_with_catalog(
        &catalog,
        "gpt-6.1-sol"
    ));
}

#[test]
fn should_use_provider_catalog_for_modern_model_routing() {
    let provider = make_test_provider(Vec::new());
    provider
        .model_endpoints
        .write()
        .unwrap()
        .insert("gpt-6.1-sol".to_string(), vec!["/responses".to_string()]);

    assert!(provider.uses_responses_api("gpt-6.1-sol"));
}

#[test]
fn context_window_handles_dot_and_dash_names() {
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "claude-opus-4.6",
            Some("copilot")
        ),
        Some(200_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "claude-opus-4-6",
            Some("copilot")
        ),
        Some(200_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "claude-opus-4.6-fast",
            Some("copilot")
        ),
        Some(200_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "claude-sonnet-4.6",
            Some("copilot")
        ),
        Some(128_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "claude-sonnet-4-6",
            Some("copilot")
        ),
        Some(128_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider("gpt-5.4", Some("copilot")),
        Some(128_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider("gpt-5.4-pro", Some("copilot")),
        Some(128_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "gpt-5.3-codex",
            Some("copilot")
        ),
        Some(128_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "gemini-3-pro-preview",
            Some("copilot")
        ),
        Some(1_000_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "gemini-2.5-pro",
            Some("copilot")
        ),
        Some(1_000_000)
    );
    assert_eq!(
        jcode_base::provider::context_limit_for_model_with_provider(
            "unknown-model",
            Some("copilot")
        ),
        Some(128_000)
    );
}

#[test]
fn has_credentials_returns_bool() {
    let _ = CopilotApiProvider::has_credentials();
}

#[test]
fn fork_preserves_fetched_models() {
    let fetched = vec!["model-a".to_string(), "model-b".to_string()];
    let provider = make_test_provider(fetched.clone());
    let forked = provider.fork();
    assert_eq!(forked.available_models_display(), fetched);
}

fn make_msg(role: Role, blocks: Vec<ContentBlock>) -> ChatMessage {
    ChatMessage {
        role,
        content: blocks,
        timestamp: None,
        tool_duration_ms: None,
    }
}

#[test]
fn build_messages_pairs_tool_use_with_tool_result() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "let me check".into(),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "echo hi"}),
                    thought_signature: None,
                },
            ],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "hi\n".into(),
                is_error: None,
            }],
        ),
    ];

    let built = CopilotApiProvider::build_messages("system prompt", &messages);

    assert_eq!(built.len(), 4);
    assert_eq!(built[0]["role"], "system");
    assert_eq!(built[1]["role"], "user");
    assert_eq!(built[1]["content"], "hello");
    assert_eq!(built[2]["role"], "assistant");
    assert!(built[2]["tool_calls"].is_array());
    assert_eq!(built[2]["tool_calls"][0]["id"], "call_1");
    assert_eq!(built[3]["role"], "tool");
    assert_eq!(built[3]["tool_call_id"], "call_1");
    assert_eq!(built[3]["content"], "hi\n");
}

#[test]
fn build_messages_injects_missing_tool_output() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "go".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_orphan".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "crash"}),
                thought_signature: None,
            }],
        ),
    ];

    let built = CopilotApiProvider::build_messages("", &messages);

    assert_eq!(built.len(), 3);
    assert_eq!(built[1]["role"], "assistant");
    assert_eq!(built[2]["role"], "tool");
    assert_eq!(built[2]["tool_call_id"], "call_orphan");
    assert!(built[2]["content"].as_str().unwrap().contains("missing"));
}

#[test]
fn build_messages_handles_batch_multiple_tool_calls() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "do things".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: "call_a".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "a"}),
                    thought_signature: None,
                },
                ContentBlock::ToolUse {
                    id: "call_b".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "b"}),
                    thought_signature: None,
                },
                ContentBlock::ToolUse {
                    id: "call_c".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "c"}),
                    thought_signature: None,
                },
            ],
        ),
        make_msg(
            Role::User,
            vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_a".into(),
                    content: "result_a".into(),
                    is_error: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_b".into(),
                    content: "result_b".into(),
                    is_error: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_c".into(),
                    content: "result_c".into(),
                    is_error: None,
                },
            ],
        ),
    ];

    let built = CopilotApiProvider::build_messages("", &messages);

    assert_eq!(built[0]["role"], "user");
    assert_eq!(built[1]["role"], "assistant");
    let tc = built[1]["tool_calls"].as_array().unwrap();
    assert_eq!(tc.len(), 3);

    assert_eq!(built[2]["role"], "tool");
    assert_eq!(built[2]["tool_call_id"], "call_a");
    assert_eq!(built[2]["content"], "result_a");
    assert_eq!(built[3]["role"], "tool");
    assert_eq!(built[3]["tool_call_id"], "call_b");
    assert_eq!(built[3]["content"], "result_b");
    assert_eq!(built[4]["role"], "tool");
    assert_eq!(built[4]["tool_call_id"], "call_c");
    assert_eq!(built[4]["content"], "result_c");
}

#[test]
fn build_messages_skips_empty_user_text() {
    let messages = vec![
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "read".into(),
                input: serde_json::json!({"file": "x"}),
                thought_signature: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "file content".into(),
                is_error: None,
            }],
        ),
    ];

    let built = CopilotApiProvider::build_messages("", &messages);

    assert_eq!(built.len(), 2);
    assert_eq!(built[0]["role"], "assistant");
    assert_eq!(built[1]["role"], "tool");
    assert_eq!(built[1]["content"], "file content");
}

#[test]
fn is_user_initiated_empty_messages() {
    let messages: Vec<ChatMessage> = vec![];
    assert!(CopilotApiProvider::is_user_initiated_raw(&messages));
}

#[test]
fn is_user_initiated_user_text_message() {
    let messages = vec![make_msg(
        Role::User,
        vec![ContentBlock::Text {
            text: "Hello".into(),
            cache_control: None,
        }],
    )];
    assert!(CopilotApiProvider::is_user_initiated_raw(&messages));
}

#[test]
fn is_user_initiated_tool_result_is_agent() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "file_read".into(),
                input: json!({}),
                thought_signature: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "file content".into(),
                is_error: None,
            }],
        ),
    ];
    assert!(!CopilotApiProvider::is_user_initiated_raw(&messages));
}

#[test]
fn is_user_initiated_assistant_last_is_user_initiated() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "Hi there".into(),
                cache_control: None,
            }],
        ),
    ];
    assert!(CopilotApiProvider::is_user_initiated_raw(&messages));
}

#[test]
fn is_user_initiated_tool_result_with_memory_injection() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "bash".into(),
                input: json!({}),
                thought_signature: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "output".into(),
                is_error: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "<system-reminder>\nSome memory context\n</system-reminder>".into(),
                cache_control: None,
            }],
        ),
    ];
    assert!(!CopilotApiProvider::is_user_initiated_raw(&messages));
}

#[test]
fn is_user_initiated_user_text_after_tool_result_without_system_reminder() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "bash".into(),
                input: json!({}),
                thought_signature: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "output".into(),
                is_error: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "Now do something else".into(),
                cache_control: None,
            }],
        ),
    ];
    assert!(CopilotApiProvider::is_user_initiated_raw(&messages));
}

#[test]
fn is_user_initiated_multiple_memory_injections_after_tool_result() {
    let messages = vec![
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "bash".into(),
                input: json!({}),
                thought_signature: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "output".into(),
                is_error: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "<system-reminder>\nMemory 1\n</system-reminder>".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "<system-reminder>\nMemory 2\n</system-reminder>".into(),
                cache_control: None,
            }],
        ),
    ];
    assert!(!CopilotApiProvider::is_user_initiated_raw(&messages));
}

#[test]
fn build_messages_sanitizes_tool_ids_with_dots() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "chatcmpl-BF2xX.tool_call.0".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "echo hi"}),
                thought_signature: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "chatcmpl-BF2xX.tool_call.0".into(),
                content: "hi\n".into(),
                is_error: None,
            }],
        ),
    ];

    let built = CopilotApiProvider::build_messages("", &messages);

    let sanitized_id = "chatcmpl-BF2xX_tool_call_0";
    assert_eq!(built[1]["tool_calls"][0]["id"], sanitized_id);
    assert_eq!(built[2]["tool_call_id"], sanitized_id);
}

#[test]
fn build_messages_sanitizes_anthropic_style_ids() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "test".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_01XFDUDYJgAACzvnptvVer6u".into(),
                name: "read".into(),
                input: serde_json::json!({"file_path": "foo.rs"}),
                thought_signature: None,
            }],
        ),
        make_msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_01XFDUDYJgAACzvnptvVer6u".into(),
                content: "file content".into(),
                is_error: None,
            }],
        ),
    ];

    let built = CopilotApiProvider::build_messages("", &messages);

    assert_eq!(
        built[1]["tool_calls"][0]["id"],
        "toolu_01XFDUDYJgAACzvnptvVer6u"
    );
    assert_eq!(built[2]["tool_call_id"], "toolu_01XFDUDYJgAACzvnptvVer6u");
}

#[test]
fn build_messages_sanitizes_missing_tool_output_ids() {
    let messages = vec![
        make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "go".into(),
                cache_control: None,
            }],
        ),
        make_msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call.with.dots.orphan".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "crash"}),
                thought_signature: None,
            }],
        ),
    ];

    let built = CopilotApiProvider::build_messages("", &messages);

    assert_eq!(built[1]["tool_calls"][0]["id"], "call_with_dots_orphan");
    assert_eq!(built[2]["tool_call_id"], "call_with_dots_orphan");
}

fn sonnet5_provider() -> CopilotApiProvider {
    let provider = make_test_provider(vec![]);
    provider.set_model("claude-sonnet-5").unwrap();
    provider
}

#[test]
fn sonnet5_reasoning_effort_serialized_when_set() {
    let provider = sonnet5_provider();
    Provider::set_reasoning_effort(&provider, "high").unwrap();
    let mut body = serde_json::json!({"model": "claude-sonnet-5"});
    provider.add_reasoning_effort_parameter(&mut body, "claude-sonnet-5", false);
    assert_eq!(body["reasoning_effort"], "high");
    assert_eq!(
        Provider::reasoning_effort(&provider).as_deref(),
        Some("high")
    );
}

#[test]
fn sonnet5_reasoning_effort_absent_when_unset() {
    let provider = sonnet5_provider();
    let mut body = serde_json::json!({"model": "claude-sonnet-5"});
    provider.add_reasoning_effort_parameter(&mut body, "claude-sonnet-5", false);
    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(Provider::reasoning_effort(&provider), None);
}

#[test]
fn sonnet5_rejects_invalid_efforts() {
    let provider = sonnet5_provider();
    for bad in ["none", "minimal", "banana", ""] {
        let err = Provider::set_reasoning_effort(&provider, bad).unwrap_err();
        assert!(
            err.to_string().contains("Unsupported reasoning effort"),
            "{bad}: {err}"
        );
    }
    assert_eq!(Provider::reasoning_effort(&provider), None);
}

#[test]
fn sonnet5_accepts_all_supported_efforts() {
    let provider = sonnet5_provider();
    for effort in ["low", "medium", "high", "xhigh", "max"] {
        Provider::set_reasoning_effort(&provider, effort).unwrap();
        assert_eq!(
            Provider::reasoning_effort(&provider).as_deref(),
            Some(effort)
        );
    }
    assert_eq!(
        Provider::available_efforts(&provider),
        vec!["low", "medium", "high", "xhigh", "max"]
    );
}

#[test]
fn non_sonnet_models_have_no_reasoning_effort() {
    let provider = make_test_provider(vec![]);
    provider.set_model("gpt-5.1-codex").unwrap();
    assert!(Provider::available_efforts(&provider).is_empty());
    let err = Provider::set_reasoning_effort(&provider, "high").unwrap_err();
    assert!(err.to_string().contains("not supported"));
    let mut body = serde_json::json!({"model": "gpt-5.1-codex"});
    provider.add_reasoning_effort_parameter(&mut body, "gpt-5.1-codex", false);
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn effort_not_serialized_after_switching_away_from_sonnet5() {
    let provider = sonnet5_provider();
    Provider::set_reasoning_effort(&provider, "max").unwrap();
    provider.set_model("gpt-5.1-codex").unwrap();
    let mut body = serde_json::json!({"model": "gpt-5.1-codex"});
    provider.add_reasoning_effort_parameter(&mut body, "gpt-5.1-codex", false);
    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(Provider::reasoning_effort(&provider), None);
}

#[test]
fn fork_preserves_reasoning_effort() {
    let provider = sonnet5_provider();
    Provider::set_reasoning_effort(&provider, "xhigh").unwrap();
    let forked = Provider::fork(&provider);
    assert_eq!(forked.reasoning_effort().as_deref(), Some("xhigh"));
}

#[test]
fn catalog_reasoning_efforts_parse_from_models_payload() {
    let info: copilot_auth::CopilotModelInfo = serde_json::from_value(json!({
        "id": "gpt-6.1-sol",
        "capabilities": {"supports": {"reasoning_effort": ["low", "Medium", "high"]}}
    }))
    .unwrap();
    assert_eq!(info.reasoning_efforts(), vec!["low", "medium", "high"]);
    let bare: copilot_auth::CopilotModelInfo =
        serde_json::from_value(json!({"id": "gpt-4o", "capabilities": {}})).unwrap();
    assert!(bare.reasoning_efforts().is_empty());
}

#[test]
fn reasoning_effort_validated_against_catalog_and_sent_per_api() {
    let provider = make_test_provider(vec![]);
    provider.model_efforts.write().unwrap().insert(
        "gpt-6.1-sol".to_string(),
        vec!["low".into(), "medium".into(), "high".into()],
    );
    provider.set_model("gpt-6.1-sol").unwrap();
    assert!(provider.set_reasoning_effort("max").is_err());
    provider.set_reasoning_effort("High").unwrap();
    assert_eq!(provider.available_efforts(), vec!["low", "medium", "high"]);

    let mut responses = json!({});
    provider.add_reasoning_effort_parameter(&mut responses, "gpt-6.1-sol", true);
    assert_eq!(responses, json!({"reasoning": {"effort": "high"}}));
    let mut chat = json!({});
    provider.add_reasoning_effort_parameter(&mut chat, "gpt-6.1-sol", false);
    assert_eq!(chat, json!({"reasoning_effort": "high"}));

    // Switching to a model without catalog support never sends the level.
    let mut other = json!({});
    provider.add_reasoning_effort_parameter(&mut other, "gpt-4o", false);
    assert_eq!(other, json!({}));
}

#[test]
fn sonnet5_effort_fallback_without_catalog() {
    let provider = make_test_provider(vec![]);
    provider.set_model("claude-sonnet-5").unwrap();
    provider.set_reasoning_effort("max").unwrap();
    provider.set_model("gpt-6.1-sol").unwrap();
    assert!(provider.set_reasoning_effort("high").is_err());
    assert_eq!(provider.reasoning_effort(), None);
}
