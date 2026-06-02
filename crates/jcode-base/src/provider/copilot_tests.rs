use super::*;

fn make_test_provider(fetched: Vec<String>) -> CopilotApiProvider {
    CopilotApiProvider {
        client: crate::provider::shared_http_client(),
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
        created_at: std::time::Instant::now(),
    }
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
fn context_window_handles_dot_and_dash_names() {
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider("claude-opus-4.6", Some("copilot")),
        Some(200_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider("claude-opus-4-6", Some("copilot")),
        Some(200_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider(
            "claude-opus-4.6-fast",
            Some("copilot")
        ),
        Some(200_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider(
            "claude-sonnet-4.6",
            Some("copilot")
        ),
        Some(128_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider(
            "claude-sonnet-4-6",
            Some("copilot")
        ),
        Some(128_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider("gpt-5.4", Some("copilot")),
        Some(128_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider("gpt-5.4-pro", Some("copilot")),
        Some(128_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider("gpt-5.3-codex", Some("copilot")),
        Some(128_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider(
            "gemini-3-pro-preview",
            Some("copilot")
        ),
        Some(1_000_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider("gemini-2.5-pro", Some("copilot")),
        Some(1_000_000)
    );
    assert_eq!(
        crate::provider::context_limit_for_model_with_provider("unknown-model", Some("copilot")),
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
                },
                ContentBlock::ToolUse {
                    id: "call_b".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "b"}),
                },
                ContentBlock::ToolUse {
                    id: "call_c".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "c"}),
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
            }],
        ),
    ];

    let built = CopilotApiProvider::build_messages("", &messages);

    assert_eq!(built[1]["tool_calls"][0]["id"], "call_with_dots_orphan");
    assert_eq!(built[2]["tool_call_id"], "call_with_dots_orphan");
}
