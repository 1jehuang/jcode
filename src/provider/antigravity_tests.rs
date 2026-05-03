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
            "gpt-oss-120b-medium": {}
        }
    }))
    .expect("parse response");

    let parsed = parse_fetch_available_models_response(&response);
    assert_eq!(parsed[0].id, "claude-opus-4-6-thinking");
    assert_eq!(parsed[1].id, "gemini-3.1-pro-high");
    assert_eq!(parsed[2].id, "gpt-oss-120b-medium");
    assert_eq!(
        parsed[0].display_name.as_deref(),
        Some("Claude Opus 4.6 (Thinking)")
    );
    assert_eq!(parsed[1].remaining_fraction_milli, Some(250));
}

#[test]
fn available_models_display_includes_dynamic_cache_and_current_override() {
    let provider = AntigravityCliProvider::new();
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

    let path = AntigravityCliProvider::persisted_catalog_path().expect("catalog path");
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
        },
    )
    .expect("write persisted catalog");

    let provider = AntigravityCliProvider::new();
    assert!(
        provider
            .available_models_display()
            .contains(&"claude-opus-4-6-thinking".to_string())
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

#[tokio::test]
async fn complete_uses_native_https_transport_not_cli_subprocess() {
    let provider = AntigravityCliProvider::new();
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
