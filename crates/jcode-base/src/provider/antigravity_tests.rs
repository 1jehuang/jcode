use super::*;
use crate::provider::Provider;
use tokio_stream::StreamExt;

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
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = AntigravityProvider::persisted_catalog_path().expect("catalog path");
    crate::storage::write_json(
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
        crate::env::set_var("JCODE_HOME", previous);
    } else {
        crate::env::remove_var("JCODE_HOME");
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
fn antigravity_compatible_schema_passes_gemini_through_unchanged() {
    // Gemini is the native backend path; it accepts everything jcode emits, so
    // the schema must be byte-identical (combiners and numeric bounds intact).
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

    assert_eq!(
        antigravity_compatible_schema(&schema, "gemini-3-flash"),
        schema,
        "Gemini path must not rewrite the schema"
    );
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
