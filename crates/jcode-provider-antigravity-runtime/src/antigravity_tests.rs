use super::*;
use chrono::Utc;
use jcode_provider_antigravity::{
    FetchAvailableModelsResponse, parse_fetch_available_models_response,
};
use jcode_provider_core::Provider;
use tokio_stream::StreamExt;

#[test]
fn malformed_credentials_report_catalog_failure_without_losing_models() {
    let _guard = jcode_base::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    jcode_base::env::set_var("JCODE_HOME", home.path());
    // An existing malformed credential file must fail before any network or
    // fallback to credentials from a locally installed third-party client.
    std::fs::write(
        jcode_base::auth::antigravity::tokens_path().unwrap(),
        "not-json",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        let provider = context_test_provider("gemini-3-flash", Some(128_000));
        let before = provider.available_models_display();
        let result = provider.refresh_model_catalog().await;
        assert_eq!(provider.available_models_display(), before);
        result
    });
    match previous {
        Some(value) => jcode_base::env::set_var("JCODE_HOME", value),
        None => jcode_base::env::remove_var("JCODE_HOME"),
    }
    assert!(result.is_err());
}

fn context_test_provider(model: &str, limit: Option<u64>) -> AntigravityProvider {
    let provider = AntigravityProvider::new();
    provider.set_model(model).unwrap();
    *provider.backend_default_model.write().unwrap() = Some("gemini-3-flash".into());
    *provider.fetched_catalog.write().unwrap() = vec![CatalogModel {
        id: "gemini-3-flash".into(),
        display_name: None,
        reset_time: None,
        tag_title: None,
        model_provider: None,
        max_tokens: limit,
        max_output_tokens: Some(8_192),
        recommended: true,
        available: true,
        remaining_fraction_milli: None,
    }];
    provider
}

#[test]
fn neon_requires_an_advertised_antigravity_route() {
    let provider = context_test_provider("gemini-4-neon", None);
    let ids = provider.available_models_for_switching();
    assert_eq!(ids, ["gemini-3-flash"]);
    assert!(
        ensure_account_model("gemini-4-neon", &ids)
            .unwrap_err()
            .to_string()
            .contains("gemini-4-neon")
    );
    assert!(ensure_account_model("gemini-4-neon", &["gemini-4-neon".into()]).is_ok());
}

#[test]
fn context_window_uses_route_catalog_instead_of_family_maximum() {
    let provider = context_test_provider("gemini-3-flash", Some(128_000));
    assert_eq!(provider.context_window(), 128_000);
    assert!(provider.supports_compaction());
    assert!(provider.uses_jcode_compaction());
}

#[test]
fn context_window_resolves_default_alias_before_lookup() {
    let provider = context_test_provider("default", Some(128_000));
    assert_eq!(provider.context_window(), 128_000);
    provider.set_model("antigravity:default").unwrap();
    assert_eq!(provider.context_window(), 128_000);
}

#[test]
fn context_window_missing_or_zero_catalog_limit_uses_resolved_family() {
    for limit in [None, Some(0)] {
        let provider = context_test_provider("default", limit);
        assert_eq!(provider.context_window(), 1_000_000);
    }
}

#[test]
fn context_window_prefers_remapped_model_catalog_limit() {
    let provider = context_test_provider("gemini-3.1-pro-high", Some(128_000));
    let mut source = provider.fetched_catalog()[0].clone();
    source.id = "gemini-3.1-pro-high".into();
    let mut target = source.clone();
    target.id = "gemini-pro-agent".into();
    target.max_tokens = Some(256_000);
    *provider.fetched_catalog.write().unwrap() = vec![source.clone(), target];
    assert_eq!(provider.context_window(), 256_000);
    *provider.fetched_catalog.write().unwrap() = vec![source];
    assert_eq!(provider.context_window(), 128_000);
    provider.set_model("unknown-test-model").unwrap();
    assert_eq!(provider.context_window(), 200_000);
}

#[test]
fn antigravity_usage_counts_cached_prompt_once_and_includes_tool_prompts() {
    let usage: jcode_provider_gemini::GeminiUsageMetadata =
        serde_json::from_value(serde_json::json!({
            "promptTokenCount": 110_000,
            "cachedContentTokenCount": 90_000,
            "toolUsePromptTokenCount": 2_000,
            "candidatesTokenCount": 500,
            "thoughtsTokenCount": 1_000
        }))
        .unwrap();
    let observed = jcode_base::compaction::effective_context_tokens_from_usage(
        "antigravity",
        usage.input_token_count().unwrap(),
        usage.cached_content_token_count,
        None,
    );
    assert_eq!(observed, 112_000);
    assert_eq!(usage.output_token_count(), Some(1_500));
    let provider = context_test_provider("gemini-3-flash", Some(128_000));
    assert!(observed as f64 / provider.context_window() as f64 > 0.8);
}

#[test]
fn parse_fetch_available_models_response_discovers_metadata_and_priority_order() {
    let response: FetchAvailableModelsResponse = serde_json::from_value(serde_json::json!({
        "defaultAgentModelId": "gemini-3.1-pro-high",
        "commandModelIds": ["gemini-3-flash"],
        "models": {
            "claude-opus-4-6-thinking": {
                "displayName": "Claude Opus 4.6 (Thinking)",
                "quotaInfo": { "remainingFraction": 1, "resetTime": "2026-04-24T20:53:26Z" },
                "recommended": true,
                "modelProvider": "MODEL_PROVIDER_ANTHROPIC"
            },
            "gemini-3.1-pro-high": {
                "displayName": "Gemini 3.1 Pro (High)",
                "quotaInfo": { "remainingFraction": 0.25 }
            },
            "gemini-3-flash": {
                "displayName": "Gemini 3 Flash",
                "quotaInfo": { "remainingFraction": 0, "resetTime": "2026-04-24T21:53:26Z" }
            },
            "gpt-oss-120b-medium": {}
        }
    }))
    .expect("parse response");

    let parsed = parse_fetch_available_models_response(&response);
    assert_eq!(
        parsed.default_model_id.as_deref(),
        Some("gemini-3.1-pro-high")
    );
    let parsed = parsed.models;
    assert_eq!(parsed[0].id, "claude-opus-4-6-thinking");
    assert_eq!(parsed[1].id, "gemini-3.1-pro-high");
    assert_eq!(parsed[2].id, "gpt-oss-120b-medium");
    assert_eq!(
        parsed[0].display_name.as_deref(),
        Some("Claude Opus 4.6 (Thinking)")
    );
    assert_eq!(parsed[1].remaining_fraction_milli, Some(250));
    let flash = parsed
        .iter()
        .find(|model| model.id == "gemini-3-flash")
        .expect("gemini flash model");
    assert!(!flash.available);
    assert_eq!(flash.remaining_fraction_milli, Some(0));
}

#[test]
fn client_metadata_uses_backend_accepted_platform() {
    assert_eq!(metadata_platform(), "PLATFORM_UNSPECIFIED");
    assert!(client_metadata_header().contains("\"platform\":\"PLATFORM_UNSPECIFIED\""));
}

#[test]
fn available_models_display_includes_dynamic_cache_and_current_override() {
    let provider = AntigravityProvider::new();
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        CatalogModel {
            id: "claude-opus-4-6-thinking".to_string(),
            display_name: None,
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: true,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
        CatalogModel {
            id: "gemini-3-pro-high".to_string(),
            display_name: None,
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
    ];
    provider
        .set_model("custom-antigravity-model")
        .expect("set custom model");

    let models = provider.available_models_display();

    assert!(models.contains(&"claude-opus-4-6-thinking".to_string()));
    assert!(models.contains(&"gemini-3-pro-high".to_string()));
    assert!(models.contains(&"custom-antigravity-model".to_string()));
}

#[test]
fn available_models_display_seeds_from_persisted_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let previous = std::env::var_os("JCODE_HOME");
    jcode_base::env::set_var("JCODE_HOME", temp.path());

    let path = jcode_base::provider::antigravity::persisted_catalog_path().expect("catalog path");
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec![CatalogModel {
                id: "claude-opus-4-6-thinking".to_string(),
                display_name: Some("Claude Opus 4.6 (Thinking)".to_string()),
                reset_time: None,
                tag_title: None,
                model_provider: None,
                max_tokens: None,
                max_output_tokens: None,
                recommended: true,
                available: true,
                remaining_fraction_milli: Some(1000),
            }],
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            default_model_id: Some("gemini-3-flash".to_string()),
            endpoint: None,
        },
    )
    .expect("write persisted catalog");

    let provider = AntigravityProvider::new();
    assert!(
        provider
            .available_models_display()
            .contains(&"claude-opus-4-6-thinking".to_string())
    );
    assert_eq!(
        provider.backend_default_model().as_deref(),
        Some("gemini-3-flash")
    );

    if let Some(previous) = previous {
        jcode_base::env::set_var("JCODE_HOME", previous);
    } else {
        jcode_base::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn catalog_detail_mentions_quota_and_reset() {
    let detail = catalog_model_detail(&CatalogModel {
        id: "claude-opus-4-6-thinking".to_string(),
        display_name: Some("Claude Opus 4.6 (Thinking)".to_string()),
        reset_time: Some("2026-04-24T20:53:26Z".to_string()),
        tag_title: Some("New".to_string()),
        model_provider: Some("MODEL_PROVIDER_ANTHROPIC".to_string()),
        max_tokens: Some(250_000),
        max_output_tokens: Some(64_000),
        recommended: true,
        available: true,
        remaining_fraction_milli: Some(1000),
    });

    assert!(detail.contains("recommended"));
    assert!(detail.contains("quota 100.0%"));
    assert!(detail.contains("resets 2026-04-24T20:53:26Z"));
}

#[test]
fn catalog_stale_handles_invalid_timestamp() {
    assert!(catalog_is_stale("not-a-time"));
}

#[test]
fn resolve_model_for_request_maps_default_alias_to_real_model() {
    let provider = AntigravityProvider::new();

    // With no backend default and no catalog, the alias resolves to the
    // hardcoded fallback rather than the literal "default" (which 404s).
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = None;
    *provider.fetched_catalog.write().expect("catalog lock") = Vec::new();
    assert_eq!(
        provider.resolve_model_for_request("default"),
        DEFAULT_FALLBACK_MODEL
    );
    assert_eq!(
        provider.resolve_model_for_request("  "),
        DEFAULT_FALLBACK_MODEL
    );

    // A backend-advertised default takes precedence over the fallback.
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = Some("gemini-3.5-flash-low".to_string());
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-3.5-flash-low"
    );

    // Explicit model ids are always passed through untouched.
    assert_eq!(
        provider.resolve_model_for_request("claude-sonnet-4-6"),
        "claude-sonnet-4-6"
    );

    // ...except the catalog-advertised-but-unserviceable `gemini-3.1-pro-high`,
    // which is remapped to the equivalent working "Gemini 3.1 Pro (High)" id.
    assert_eq!(
        provider.resolve_model_for_request("gemini-3.1-pro-high"),
        "gemini-pro-agent"
    );
    // Its sibling `-low` works as-is and must not be remapped.
    assert_eq!(
        provider.resolve_model_for_request("gemini-3.1-pro-low"),
        "gemini-3.1-pro-low"
    );
}

#[test]
fn remap_unsupported_model_only_touches_broken_pro_high() {
    assert_eq!(
        remap_unsupported_model("gemini-3.1-pro-high"),
        "gemini-pro-agent"
    );
    for model in [
        "gemini-3.1-pro-low",
        "gemini-pro-agent",
        "gemini-3-flash",
        "claude-sonnet-4-6",
        "gpt-oss-120b-medium",
        "default",
    ] {
        assert_eq!(remap_unsupported_model(model), model);
    }
}

#[test]
fn resolve_model_for_request_default_prefers_gemini_catalog_model() {
    let provider = AntigravityProvider::new();
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = None;
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        CatalogModel {
            id: "claude-opus-4-6-thinking".to_string(),
            display_name: None,
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: true,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
        CatalogModel {
            id: "gemini-3-flash".to_string(),
            display_name: None,
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
    ];

    // Even though Claude is listed first (and recommended), the default alias
    // resolves to the Gemini model, which works reliably with tool use on the
    // Cloud Code backend. Claude on this backend rejects jcode's tool schemas.
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-3-flash"
    );
}

#[test]
fn resolve_model_for_request_default_falls_back_to_any_catalog_model_without_gemini() {
    let provider = AntigravityProvider::new();
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = None;
    *provider.fetched_catalog.write().expect("catalog lock") = vec![CatalogModel {
        id: "claude-opus-4-6-thinking".to_string(),
        display_name: None,
        reset_time: None,
        tag_title: None,
        model_provider: None,
        max_tokens: None,
        max_output_tokens: None,
        recommended: true,
        available: true,
        remaining_fraction_milli: Some(1000),
    }];

    // With no Gemini model available, fall back to the first available catalog
    // model rather than the hardcoded default.
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "claude-opus-4-6-thinking"
    );
}

#[tokio::test]
async fn complete_uses_native_https_transport_not_cli_subprocess() {
    let _guard = jcode_base::storage::lock_test_env();
    let provider = AntigravityProvider::new();
    let mut stream = provider
        .complete(&[], &[], "say hello", None)
        .await
        .expect("create stream");

    let first_event = stream
        .next()
        .await
        .expect("first event")
        .expect("connection event");

    match first_event {
        StreamEvent::ConnectionType { connection } => {
            assert_eq!(connection, "https");
            assert_ne!(connection, "cli subprocess");
        }
        other => panic!("expected connection type, got {other:?}"),
    }
}

#[test]
fn model_is_claude_detects_anthropic_models_only() {
    assert!(model_is_claude("claude-sonnet-4-6"));
    assert!(model_is_claude("claude-opus-4-6-thinking"));
    assert!(model_is_claude("CLAUDE-SONNET"));
    assert!(!model_is_claude("gemini-3-flash"));
    assert!(!model_is_claude("gpt-oss-120b-medium"));
    assert!(!model_is_claude("default"));
}

#[test]
fn flatten_schema_combiners_collapses_anyof_to_first_branch() {
    // Mirrors the real `bg` tool's `status_filter` schema that the Antigravity
    // Claude backend rejects.
    let schema = serde_json::json!({
        "anyOf": [
            { "type": "string" },
            { "items": { "type": "string" }, "type": "array" }
        ],
        "description": "Status filter string or array."
    });

    let flattened = flatten_schema_combiners(&schema);

    assert!(flattened.get("anyOf").is_none(), "anyOf must be removed");
    assert_eq!(flattened["type"], serde_json::json!("string"));
    // Sibling metadata is preserved onto the chosen branch.
    assert_eq!(
        flattened["description"],
        serde_json::json!("Status filter string or array.")
    );
}

#[test]
fn flatten_schema_combiners_recurses_into_nested_properties() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "status_filter": {
                "oneOf": [
                    { "type": "string" },
                    { "type": "array" }
                ]
            },
            "name": { "type": "string" }
        }
    });

    let flattened = flatten_schema_combiners(&schema);

    assert_eq!(
        flattened["properties"]["status_filter"]["type"],
        serde_json::json!("string")
    );
    assert!(
        flattened["properties"]["status_filter"]
            .get("oneOf")
            .is_none()
    );
    // Untouched branches are preserved verbatim.
    assert_eq!(
        flattened["properties"]["name"]["type"],
        serde_json::json!("string")
    );
}

#[test]
fn flatten_schema_combiners_collapses_allof_inside_array_items() {
    let schema = serde_json::json!({
        "type": "array",
        "items": {
            "allOf": [
                { "type": "object", "properties": { "tool": { "type": "string" } } }
            ]
        }
    });

    let flattened = flatten_schema_combiners(&schema);

    assert!(flattened["items"].get("allOf").is_none());
    assert_eq!(flattened["items"]["type"], serde_json::json!("object"));
}

#[test]
fn flatten_schema_combiners_leaves_combiner_free_schema_unchanged() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "task_ids": { "type": "array", "items": { "type": "string" } },
            "intent": { "type": "string" }
        },
        "required": ["intent"]
    });

    assert_eq!(flatten_schema_combiners(&schema), schema);
}

#[test]
fn model_is_gemini_detects_gemini_models_only() {
    assert!(model_is_gemini("gemini-3-flash"));
    assert!(model_is_gemini("gemini-2.5-pro"));
    assert!(model_is_gemini("GEMINI-3-FLASH"));
    assert!(!model_is_gemini("claude-sonnet-4-6"));
    assert!(!model_is_gemini("gpt-oss-120b-medium"));
    assert!(!model_is_gemini("default"));
}

#[test]
fn strip_numeric_schema_bounds_drops_array_and_string_and_object_bounds() {
    // Mirrors the real `batch` tool schema that the gpt-oss backend rejects
    // because it re-encodes the integer bound as the string "10".
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "tool_calls": {
                "type": "array",
                "items": { "type": "object" },
                "minItems": 1,
                "maxItems": 10
            },
            "name": { "type": "string", "minLength": 1, "maxLength": 64 }
        },
        "minProperties": 1,
        "maxProperties": 5
    });

    let stripped = strip_numeric_schema_bounds(&schema);

    let tool_calls = &stripped["properties"]["tool_calls"];
    assert!(tool_calls.get("minItems").is_none());
    assert!(tool_calls.get("maxItems").is_none());
    // Structural keys are preserved.
    assert_eq!(tool_calls["type"], serde_json::json!("array"));
    assert_eq!(tool_calls["items"]["type"], serde_json::json!("object"));

    let name = &stripped["properties"]["name"];
    assert!(name.get("minLength").is_none());
    assert!(name.get("maxLength").is_none());
    assert_eq!(name["type"], serde_json::json!("string"));

    assert!(stripped.get("minProperties").is_none());
    assert!(stripped.get("maxProperties").is_none());
}

#[test]
fn antigravity_gemini_uses_the_same_dialect_as_the_native_gemini_provider() {
    // Antigravity's Gemini route reaches the same `generateContent` validator
    // as the native Gemini provider, so it must apply the identical dialect.
    // Historically it applied a laxer one and inherited every Gemini schema
    // outage a release late (#754), which is what the shared dialect prevents.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "status_filter": {
                "anyOf": [
                    { "type": "string" },
                    { "items": { "type": "string" }, "type": "array" }
                ]
            },
            "tool_calls": { "type": "array", "minItems": 1, "maxItems": 10 },
            "data": {
                "type": "object",
                "propertyNames": { "type": "string" },
                "additionalProperties": { "type": "string" }
            }
        }
    });

    let out = antigravity_compatible_schema(&schema, "gemini-3-flash");

    // Byte-identical to what the native Gemini provider would send.
    assert_eq!(
        out,
        jcode_provider_gemini::gemini_compatible_schema(&schema),
        "the two Gemini routes must not drift apart"
    );

    // Both keywords Gemini rejects are gone, at depth.
    assert!(out["properties"]["data"].get("propertyNames").is_none());
    assert!(
        out["properties"]["data"]
            .get("additionalProperties")
            .is_none()
    );

    // Everything load-bearing survives: combiner branches, types, bounds.
    assert_eq!(
        out["properties"]["status_filter"],
        schema["properties"]["status_filter"]
    );
    assert_eq!(
        out["properties"]["tool_calls"],
        schema["properties"]["tool_calls"]
    );
    assert_eq!(out["properties"]["data"]["type"], "object");
}

#[test]
fn antigravity_compatible_schema_flattens_combiners_for_claude_but_keeps_bounds() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "status_filter": {
                "anyOf": [
                    { "type": "string" },
                    { "items": { "type": "string" }, "type": "array" }
                ]
            },
            "tool_calls": { "type": "array", "minItems": 1, "maxItems": 10 }
        }
    });

    let out = antigravity_compatible_schema(&schema, "claude-sonnet-4-6");

    // Combiner collapsed (Anthropic strictness)...
    assert!(out["properties"]["status_filter"].get("anyOf").is_none());
    assert_eq!(
        out["properties"]["status_filter"]["type"],
        serde_json::json!("string")
    );
    // ...but numeric bounds are retained for Claude (it accepts them).
    assert_eq!(
        out["properties"]["tool_calls"]["maxItems"],
        serde_json::json!(10)
    );
}

#[test]
fn antigravity_compatible_schema_strips_bounds_and_combiners_for_gpt_oss() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "status_filter": {
                "anyOf": [
                    { "type": "string" },
                    { "items": { "type": "string" }, "type": "array" }
                ]
            },
            "tool_calls": { "type": "array", "minItems": 1, "maxItems": 10 }
        }
    });

    let out = antigravity_compatible_schema(&schema, "gpt-oss-120b-medium");

    // Combiner collapsed AND numeric bounds dropped (OpenAI-compatible bridge).
    assert!(out["properties"]["status_filter"].get("anyOf").is_none());
    assert_eq!(
        out["properties"]["status_filter"]["type"],
        serde_json::json!("string")
    );
    assert!(out["properties"]["tool_calls"].get("minItems").is_none());
    assert!(out["properties"]["tool_calls"].get("maxItems").is_none());
    assert_eq!(
        out["properties"]["tool_calls"]["type"],
        serde_json::json!("array")
    );
}

#[test]
fn is_retryable_empty_turn_detects_malformed_function_call() {
    // Empty content + MALFORMED_FUNCTION_CALL is the transient Gemini-3 failure we
    // retry transparently.
    let response: CodeAssistGenerateResponse = serde_json::from_value(serde_json::json!({
        "response": {
            "candidates": [{
                "content": {},
                "finishReason": "MALFORMED_FUNCTION_CALL",
                "finishMessage": "Malformed function call: print(default_api.read(...))"
            }]
        }
    }))
    .expect("decode malformed response");
    assert!(is_retryable_empty_turn(&response));
}

#[test]
fn is_retryable_empty_turn_ignores_normal_and_productive_turns() {
    // A normal STOP turn with text is never retried.
    let with_text: CodeAssistGenerateResponse = serde_json::from_value(serde_json::json!({
        "response": {
            "candidates": [{
                "content": {"parts": [{"text": "hello"}]},
                "finishReason": "STOP"
            }]
        }
    }))
    .expect("decode text response");
    assert!(!is_retryable_empty_turn(&with_text));

    // A turn with a function call is productive even with no text.
    let with_call: CodeAssistGenerateResponse = serde_json::from_value(serde_json::json!({
        "response": {
            "candidates": [{
                "content": {"parts": [{"functionCall": {"name": "read", "args": {}}}]},
                "finishReason": "STOP"
            }]
        }
    }))
    .expect("decode function call response");
    assert!(!is_retryable_empty_turn(&with_call));

    // An empty STOP turn (legitimately empty answer) is not retried in a loop.
    let empty_stop: CodeAssistGenerateResponse = serde_json::from_value(serde_json::json!({
        "response": {
            "candidates": [{ "content": {}, "finishReason": "STOP" }]
        }
    }))
    .expect("decode empty stop response");
    assert!(!is_retryable_empty_turn(&empty_stop));
}

#[test]
fn pseudo_tool_call_turn_only_matches_the_recovery_marker_as_a_call() {
    let response = |text: &str| {
        serde_json::from_value::<CodeAssistGenerateResponse>(serde_json::json!({
            "response": {
                "candidates": [{
                    "content": {"parts": [{"text": text}]},
                    "finishReason": "STOP"
                }]
            }
        }))
        .expect("decode response")
    };

    assert!(jcode_provider_antigravity::is_pseudo_tool_call_turn(
        &response("  [previous tool call] todo({\"todos\":[]})")
    ));
    assert!(!jcode_provider_antigravity::is_pseudo_tool_call_turn(
        &response("I saw [previous tool call] todo({}) in the transcript")
    ));
    assert!(!jcode_provider_antigravity::is_pseudo_tool_call_turn(
        &response("[previous tool call] was only a history marker")
    ));
    assert!(!jcode_provider_antigravity::is_pseudo_tool_call_turn(
        &response("ordinary answer")
    ));
}

/// End-to-end guard for the turn-2 HTTP 404: `--provider antigravity` gives the
/// agent this runtime directly, and session restore calls `set_model` with the
/// routing spec `antigravity:<model>`. The id we store is the id we put on the
/// wire, so it must be the bare model name.
#[test]
fn set_model_stores_bare_id_for_prefixed_session_restore_spec() {
    let provider = AntigravityProvider::new();

    provider
        .set_model("antigravity:gemini-3-flash")
        .expect("prefixed session-restore spec must be accepted");
    assert_eq!(
        provider.model(),
        "gemini-3-flash",
        "the wire model id must never carry the antigravity: routing prefix"
    );

    provider
        .set_model("gemini-3-flash")
        .expect("bare id must still be accepted");
    assert_eq!(provider.model(), "gemini-3-flash");

    assert!(
        provider.set_model("antigravity:").is_err(),
        "a prefix with no model must be rejected, not stored as an empty model"
    );
    assert!(provider.set_model("   ").is_err());
}

#[test]
fn resolve_model_for_request_default_prefers_gemini_4_neon_low_when_advertised() {
    assert_eq!(DEFAULT_FALLBACK_MODEL, "gemini-4-neon-low");

    let provider = AntigravityProvider::new();
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = Some("gemini-4-neon-high".to_string());
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        CatalogModel {
            id: "gemini-3-flash".to_string(),
            display_name: Some("Gemini 3 Flash".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: Some(1_048_576),
            max_output_tokens: Some(65_536),
            recommended: true,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
        CatalogModel {
            id: "gemini-4-neon-high".to_string(),
            display_name: Some("Gemini 4 Neon (High)".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: Some(1_048_576),
            max_output_tokens: Some(65_536),
            recommended: true,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
        CatalogModel {
            id: "gemini-4-neon-low".to_string(),
            display_name: Some("Gemini 4 Neon (Low)".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: Some(1_048_576),
            max_output_tokens: Some(65_536),
            recommended: true,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
    ];

    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-4-neon-low"
    );
}

#[test]
fn antigravity_reasoning_effort_switches_sibling_models_and_supports_swarm() {
    let provider = AntigravityProvider::new();
    *provider.fetched_catalog.write().unwrap() = vec![
        "gemini-4-neon-low",
        "gemini-4-neon-medium",
        "gemini-4-neon-high",
        "gemini-4-neon-xhigh",
        "gemini-4-hydrogen-tiered-high",
        "gemini-4-hydrogen-tiered-xhigh",
        "claude-opus-5-5-low",
        "claude-opus-5-5-medium",
        "claude-opus-5-5-high",
        "gpt-oss-120b-medium",
        "gemini-3-flash",
        "gemini-5-future-low",
        "gemini-5-future-xhigh",
    ]
    .into_iter()
    .map(|id| CatalogModel {
        id: id.to_string(),
        display_name: None,
        reset_time: None,
        tag_title: None,
        model_provider: None,
        max_tokens: Some(1_048_576),
        max_output_tokens: Some(65_536),
        recommended: true,
        available: true,
        remaining_fraction_milli: Some(1000),
    })
    .collect();

    provider.set_model("gemini-4-neon-low").unwrap();
    assert_eq!(
        provider.available_efforts(),
        vec!["low", "medium", "high", "xhigh", "swarm"]
    );
    assert_eq!(provider.reasoning_effort().as_deref(), Some("low"));

    provider.set_reasoning_effort("high").unwrap();
    assert_eq!(provider.model(), "gemini-4-neon-high");
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));

    provider.set_reasoning_effort("xhigh").unwrap();
    assert_eq!(provider.model(), "gemini-4-neon-xhigh");
    assert_eq!(provider.reasoning_effort().as_deref(), Some("xhigh"));

    // Swarm mode preserves sentinel in reasoning_effort() and routes requests
    // to the configured root effort sibling.
    provider.set_reasoning_effort("swarm").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("swarm"));
    assert_eq!(provider.model(), "gemini-4-neon-xhigh");
    assert_eq!(
        provider.resolve_model_for_request(&provider.model()),
        "gemini-4-neon-low"
    );

    // Forking inherits swarm state independently.
    let forked = provider.fork();
    assert_eq!(forked.reasoning_effort().as_deref(), Some("swarm"));
    forked.set_reasoning_effort("medium").unwrap();
    assert_eq!(forked.model(), "gemini-4-neon-medium");
    assert_eq!(provider.reasoning_effort().as_deref(), Some("swarm"));

    // Explicit set_model clears swarm mode and reflects the chosen tier.
    provider.set_model("gemini-4-hydrogen-tiered-high").unwrap();
    assert_eq!(provider.available_efforts(), vec!["high", "xhigh", "swarm"]);
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));
    assert!(provider.set_reasoning_effort("low").is_err());

    // Discovered future models absent from fallback arrays still get dynamic effort switching.
    provider.set_model("gemini-5-future-low").unwrap();
    assert_eq!(provider.available_efforts(), vec!["low", "xhigh", "swarm"]);
    provider.set_reasoning_effort("xhigh").unwrap();
    assert_eq!(provider.model(), "gemini-5-future-xhigh");

    // Non-effort and single-variant models do not advertise bogus effort ladders.
    provider.set_model("gemini-3-flash").unwrap();
    assert!(provider.available_efforts().is_empty());
    assert_eq!(provider.reasoning_effort(), None);
    assert!(provider.set_reasoning_effort("high").is_err());

    provider.set_model("gpt-oss-120b-medium").unwrap();
    assert!(provider.available_efforts().is_empty());
    assert_eq!(provider.reasoning_effort(), None);
}

fn write_test_antigravity_tokens() {
    let tokens = antigravity_auth::AntigravityTokens {
        access_token: "test-access-token".into(),
        refresh_token: "test-refresh-token".into(),
        expires_at: Utc::now().timestamp_millis() + 3_600_000,
        email: Some("test@example.com".into()),
        project_id: Some("test-project-1".into()),
    };
    antigravity_auth::save_tokens(&tokens).unwrap();
}

#[test]
fn antigravity_streams_gemini_thoughts_live_over_sse() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let _guard = jcode_base::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_endpoint = std::env::var_os(jcode_provider_antigravity::ENDPOINT_ENV);
    jcode_base::env::set_var("JCODE_HOME", home.path());
    write_test_antigravity_tokens();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let outcome = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        jcode_base::env::set_var(jcode_provider_antigravity::ENDPOINT_ENV, &endpoint);

        let (release_chunk2_tx, release_chunk2_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(listener.accept().await.unwrap().0);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            assert!(
                line.starts_with("POST /v1internal:streamGenerateContent?alt=sse "),
                "unexpected request line: {line}"
            );
            let mut length = 0usize;
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                    length = n.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).await.unwrap();
            let req_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(req_json["model"], "gemini-3-flash");
            assert_eq!(
                req_json["request"]["generationConfig"]["thinkingConfig"]["includeThoughts"],
                serde_json::json!(true),
                "Antigravity request must enable includeThoughts: {req_json}"
            );

            let stream = reader.get_mut();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            stream
                .write_all(
                    b"data: {\"response\":{\"modelVersion\":\"gemini-3-flash\",\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Analyzing problem...\",\"thought\":true}]}}]}}\r\n\r\n",
                )
                .await
                .unwrap();
            stream.flush().await.unwrap();

            // Do not send chunk 2 until client receives chunk 1 live!
            release_chunk2_rx.await.unwrap();

            stream
                .write_all(
                    b"data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\" Checking answer.\",\"thought\":true}]}}]}}\r\n\r\n",
                )
                .await
                .unwrap();
            stream
                .write_all(
                    b"data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"42\"}]}}]}}\r\n\r\n",
                )
                .await
                .unwrap();
            stream
                .write_all(
                    b"data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"\",\"thoughtSignature\":\"sig-live-1\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":20,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":35}}}\r\n\r\n",
                )
                .await
                .unwrap();
            stream.flush().await.unwrap();
        });

        let mut provider = context_test_provider("gemini-3-flash", Some(128_000));
        provider.client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut events = provider.complete(&[], &[], "system", None).await.unwrap();

        let mut release_tx = Some(release_chunk2_tx);
        let mut thoughts = Vec::new();
        let mut texts = Vec::new();
        let mut signature = None;
        let mut saw_thinking_start = false;
        let mut saw_thinking_end = false;
        let mut saw_thinking_done = false;
        let mut stop_reason = None;

        while let Some(ev) = events.next().await {
            match ev.unwrap() {
                StreamEvent::ThinkingStart => saw_thinking_start = true,
                StreamEvent::ThinkingDelta(delta) => {
                    thoughts.push(delta);
                    if let Some(tx) = release_tx.take() {
                        let _ = tx.send(());
                    }
                }
                StreamEvent::ThinkingEnd => saw_thinking_end = true,
                StreamEvent::ThinkingDone { .. } => saw_thinking_done = true,
                StreamEvent::TextDelta(delta) => texts.push(delta),
                StreamEvent::ThinkingSignatureDelta(sig) => signature = Some(sig),
                StreamEvent::MessageEnd { stop_reason: sr } => stop_reason = sr,
                _ => {}
            }
        }

        server.await.unwrap();
        (
            saw_thinking_start,
            saw_thinking_end,
            saw_thinking_done,
            thoughts,
            texts,
            signature,
            stop_reason,
        )
    });

    match prev_home {
        Some(value) => jcode_base::env::set_var("JCODE_HOME", value),
        None => jcode_base::env::remove_var("JCODE_HOME"),
    }
    match prev_endpoint {
        Some(value) => jcode_base::env::set_var(jcode_provider_antigravity::ENDPOINT_ENV, value),
        None => jcode_base::env::remove_var(jcode_provider_antigravity::ENDPOINT_ENV),
    }

    assert!(outcome.0, "expected ThinkingStart");
    assert!(outcome.1, "expected ThinkingEnd");
    assert!(outcome.2, "expected ThinkingDone");
    assert_eq!(outcome.3, vec!["Analyzing problem...", " Checking answer."]);
    assert_eq!(outcome.4, vec!["42"]);
    assert_eq!(outcome.5.as_deref(), Some("sig-live-1"));
    assert_eq!(outcome.6.as_deref(), Some("STOP"));
}

#[test]
fn antigravity_retries_malformed_function_call_after_thought_only_turn() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let _guard = jcode_base::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_endpoint = std::env::var_os(jcode_provider_antigravity::ENDPOINT_ENV);
    jcode_base::env::set_var("JCODE_HOME", home.path());
    write_test_antigravity_tokens();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let tool_names = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        jcode_base::env::set_var(jcode_provider_antigravity::ENDPOINT_ENV, &endpoint);

        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let mut reader = BufReader::new(listener.accept().await.unwrap().0);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let mut length = 0usize;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).await.unwrap();
                let req_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let stream = reader.get_mut();
                if attempt == 0 {
                    assert_eq!(
                        req_json["request"]["toolConfig"]["functionCallingConfig"]["mode"],
                        "AUTO"
                    );
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Thinking before malformed call\",\"thought\":true}]}}]}}\r\n\r\ndata: {\"response\":{\"candidates\":[{\"finishReason\":\"MALFORMED_FUNCTION_CALL\"}]}}\r\n\r\n",
                        )
                        .await
                        .unwrap();
                } else {
                    assert_eq!(
                        req_json["request"]["toolConfig"]["functionCallingConfig"]["mode"],
                        "ANY"
                    );
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"read\",\"args\":{\"path\":\"README.md\"}},\"thoughtSignature\":\"sig-retry\"}]},\"finishReason\":\"STOP\"}]}}\r\n\r\n",
                        )
                        .await
                        .unwrap();
                }
                stream.flush().await.unwrap();
                let _ = stream.shutdown().await;
            }
        });

        let mut provider = context_test_provider("gemini-3-flash", Some(128_000));
        provider.client = reqwest::Client::builder().no_proxy().build().unwrap();
        let tools = vec![ToolDefinition {
            name: "read".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            defer_loading: false,
        }];
        let mut events = provider.complete(&[], &tools, "system", None).await.unwrap();
        let mut names = Vec::new();
        let mut thinking_starts = 0usize;
        let mut thinking_ends = 0usize;
        while let Some(ev) = events.next().await {
            match ev.unwrap() {
                StreamEvent::ThinkingStart => thinking_starts += 1,
                StreamEvent::ThinkingEnd => thinking_ends += 1,
                StreamEvent::ToolUseStart { name, .. } => names.push(name),
                _ => {}
            }
        }
        assert_eq!(thinking_starts, 1);
        assert_eq!(thinking_ends, 1, "thinking block must be cleanly closed before retry");
        server.await.unwrap();
        names
    });

    match prev_home {
        Some(value) => jcode_base::env::set_var("JCODE_HOME", value),
        None => jcode_base::env::remove_var("JCODE_HOME"),
    }
    match prev_endpoint {
        Some(value) => jcode_base::env::set_var(jcode_provider_antigravity::ENDPOINT_ENV, value),
        None => jcode_base::env::remove_var(jcode_provider_antigravity::ENDPOINT_ENV),
    }

    assert_eq!(tool_names, vec!["read"]);
}
