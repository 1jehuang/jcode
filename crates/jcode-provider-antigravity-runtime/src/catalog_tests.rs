use super::*;
use chrono::Utc;
use jcode_provider_antigravity::{
    FetchAvailableModelsResponse, antigravity_version, parse_fetch_available_models_response,
};
use jcode_provider_core::Provider;

fn test_catalog_entry(id: &str, available: bool) -> CatalogModel {
    CatalogModel {
        id: id.to_string(),
        display_name: None,
        reset_time: None,
        tag_title: None,
        model_provider: None,
        max_tokens: None,
        max_output_tokens: None,
        recommended: true,
        available,
        remaining_fraction_milli: Some(if available { 1000 } else { 0 }),
    }
}

#[test]
fn load_persisted_catalog_invalidates_unstamped_mismatched_and_legacy_non_cli_caches() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let previous = std::env::var_os("JCODE_HOME");
    jcode_base::env::set_var("JCODE_HOME", temp.path());

    let path = jcode_base::provider::antigravity::persisted_catalog_path().expect("catalog path");
    let active_version = antigravity_version();

    // 1. Unstamped legacy cache (`cli_version: None`) must be invalidated.
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec![test_catalog_entry("gemini-4-argon-low", true)],
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            default_model_id: Some("gemini-4-argon-low".to_string()),
            endpoint: None,
            cli_version: None,
        },
    )
    .expect("write unstamped catalog");
    assert!(jcode_base::provider::antigravity::load_persisted_catalog().is_none());

    // 2. Version-mismatched cache (`cli_version: Some("1.3.5")`) must be invalidated
    // so upgrading `agy` triggers a fresh catalog fetch for newly unlocked models.
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec![test_catalog_entry("gemini-4-argon-low", true)],
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            default_model_id: Some("gemini-4-argon-low".to_string()),
            endpoint: None,
            cli_version: Some("1.3.5".to_string()),
        },
    )
    .expect("write mismatched-version catalog");
    assert!(jcode_base::provider::antigravity::load_persisted_catalog().is_none());

    // 3. Legacy non-CLI catalog (`gemini-3.8-flash-tiered` without Carbon/Argon)
    // must be rejected even if stamped with the current CLI version.
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec![test_catalog_entry("gemini-3.8-flash-tiered", true)],
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            default_model_id: None,
            endpoint: None,
            cli_version: Some(active_version.clone()),
        },
    )
    .expect("write legacy tiered catalog");
    assert!(jcode_base::provider::antigravity::load_persisted_catalog().is_none());

    // 4. `persist_catalog` stamps `cli_version` automatically and loads cleanly.
    jcode_base::provider::antigravity::persist_catalog(&CatalogSnapshot {
        models: vec![
            test_catalog_entry("gemini-4-carbon-low", true),
            test_catalog_entry("gemini-4-argon-low", true),
        ],
        default_model_id: Some("gemini-4-carbon-low".to_string()),
    });
    let loaded = jcode_base::provider::antigravity::load_persisted_catalog()
        .expect("current CLI catalog must load");
    assert_eq!(loaded.cli_version.as_deref(), Some(active_version.as_str()));
    assert_eq!(
        loaded.default_model_id.as_deref(),
        Some("gemini-4-carbon-low")
    );

    match previous {
        Some(prev) => jcode_base::env::set_var("JCODE_HOME", prev),
        None => jcode_base::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn future_catalog_model_absent_from_static_fallbacks_is_discovered_in_switching_and_routes() {
    assert!(
        !AVAILABLE_MODELS.contains(&"gemini-5-xenon-ultra"),
        "test fixture model must be absent from static AVAILABLE_MODELS"
    );

    let response: FetchAvailableModelsResponse = serde_json::from_value(serde_json::json!({
        "defaultAgentModelId": "gemini-5-xenon-ultra",
        "models": {
            "gemini-5-xenon-ultra": {
                "displayName": "Gemini 5 Xenon Ultra",
                "quotaInfo": { "remainingFraction": 0.9, "resetTime": "2026-12-01T00:00:00Z" },
                "recommended": true,
                "maxTokens": 2_000_000,
                "maxOutputTokens": 65_536,
                "modelProvider": "MODEL_PROVIDER_GOOGLE"
            }
        }
    }))
    .expect("parse future catalog response");

    let snapshot = parse_fetch_available_models_response(&response);
    let provider = AntigravityProvider {
        client: reqwest::Client::new(),
        model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        fetched_catalog: Arc::new(RwLock::new(snapshot.models)),
        backend_default_model: Arc::new(RwLock::new(snapshot.default_model_id)),
    };

    assert!(
        provider
            .available_models_for_switching()
            .contains(&"gemini-5-xenon-ultra".to_string())
    );
    assert!(
        provider
            .available_models_display()
            .contains(&"gemini-5-xenon-ultra".to_string())
    );
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-5-xenon-ultra"
    );
    assert_eq!(provider.context_window(), 2_000_000);

    let routes = provider.model_routes();
    let future_route = routes
        .iter()
        .find(|route| route.model == "gemini-5-xenon-ultra")
        .expect("future catalog model route");
    assert!(future_route.available);
    assert!(future_route.detail.contains("Gemini 5 Xenon Ultra"));
}

#[test]
fn model_routes_marks_omitted_static_fallbacks_unverified_in_sparse_catalog_and_preserves_false() {
    let provider = AntigravityProvider::new();
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        CatalogModel {
            id: "gemini-3-flash".to_string(),
            display_name: Some("Gemini 3 Flash".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: Some("MODEL_PROVIDER_GOOGLE".to_string()),
            max_tokens: None,
            max_output_tokens: None,
            recommended: true,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
        CatalogModel {
            id: "claude-sonnet-4-6".to_string(),
            display_name: Some("Claude Sonnet 4.6".to_string()),
            reset_time: Some("2026-10-15T00:00:00Z".to_string()),
            tag_title: None,
            model_provider: Some("MODEL_PROVIDER_ANTHROPIC".to_string()),
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: false,
            remaining_fraction_milli: Some(0),
        },
        // Duplicate entry in fetched_catalog must be deduplicated, keeping first entry.
        CatalogModel {
            id: "claude-sonnet-4-6".to_string(),
            display_name: Some("Duplicate Claude Sonnet".to_string()),
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

    let routes = provider.model_routes();

    let flash_routes: Vec<_> = routes
        .iter()
        .filter(|route| route.model == "gemini-3-flash")
        .collect();
    assert_eq!(flash_routes.len(), 1, "gemini-3-flash must not duplicate");
    assert!(flash_routes[0].available);
    assert!(flash_routes[0].detail.contains("Gemini 3 Flash"));

    let claude_routes: Vec<_> = routes
        .iter()
        .filter(|route| route.model == "claude-sonnet-4-6")
        .collect();
    assert_eq!(
        claude_routes.len(),
        1,
        "claude-sonnet-4-6 must be deduplicated across catalog and fallback list"
    );
    assert!(
        !claude_routes[0].available,
        "explicit available:false in fetched catalog must be preserved"
    );
    assert!(claude_routes[0].detail.contains("quota 0.0%"));

    // Static fallback models absent from the sparse fetched catalog must be
    // present as unverified fallbacks (available: false), not marked verified.
    for omitted in [
        "gemini-4-carbon-high",
        "gemini-4-carbon-medium",
        "gemini-4-carbon-low",
        "gemini-4-argon-high",
        "gemini-4-argon-medium",
        "gemini-4-argon-low",
        "gemini-3.8-flash-medium",
    ] {
        let matching: Vec<_> = routes
            .iter()
            .filter(|route| route.model == omitted)
            .collect();
        assert_eq!(matching.len(), 1, "{omitted} should appear once in routes");
        assert!(
            !matching[0].available,
            "omitted static fallback {omitted} must not be marked verified available when catalog is non-empty"
        );
        assert_eq!(matching[0].detail, "fallback catalog (unverified)");
    }
}

#[test]
fn model_routes_reconciles_alias_availability_by_canonical_request_id() {
    let provider = AntigravityProvider::new();
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        CatalogModel {
            id: "gemini-4-carbon".to_string(),
            display_name: Some("Gemini 4 Carbon".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: false,
            remaining_fraction_milli: Some(0),
        },
        CatalogModel {
            id: "gemini-4-carbon-medium".to_string(),
            display_name: Some("Gemini 4 Carbon Medium".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
        test_catalog_entry("gemini-3-flash", true),
    ];

    let routes = provider.model_routes();
    let bare_carbon = routes
        .iter()
        .find(|route| route.model == "gemini-4-carbon")
        .expect("bare carbon route");
    let medium_carbon = routes
        .iter()
        .find(|route| route.model == "gemini-4-carbon-medium")
        .expect("medium carbon route");

    assert!(!bare_carbon.available);
    assert!(
        !medium_carbon.available,
        "canonical target must not be marked available when its alias is unavailable"
    );
    assert!(
        medium_carbon
            .detail
            .contains("canonical target 'gemini-4-carbon-medium' unavailable"),
        "expected canonical veto explanation in detail, got: {}",
        medium_carbon.detail
    );

    // Exact-ID duplicate policy is keep-first: a trailing `available: false`
    // duplicate of the exact same ID must not veto the first retained `true` row.
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        test_catalog_entry("gemini-3-flash", true),
        test_catalog_entry(" gemini-3-flash ", false),
    ];
    let deduped_routes = provider.model_routes();
    let flash = deduped_routes
        .iter()
        .find(|route| route.model == "gemini-3-flash")
        .expect("gemini-3-flash route");
    assert!(
        flash.available,
        "exact-ID deduplication is keep-first and must retain the first row's availability"
    );
}

#[test]
fn antigravity_context_window_resolves_carbon_and_argon_defaults_and_catalog_fallback() {
    let provider = AntigravityProvider {
        client: reqwest::Client::new(),
        model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        fetched_catalog: Arc::new(RwLock::new(vec![CatalogModel {
            id: "chat_20706".to_string(),
            display_name: None,
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: Some(16_384),
            max_output_tokens: None,
            recommended: false,
            available: true,
            remaining_fraction_milli: Some(1000),
        }])),
        backend_default_model: Arc::new(RwLock::new(Some("gemini-4-carbon".to_string()))),
    };

    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-4-carbon-medium"
    );
    assert_eq!(provider.context_window(), 1_000_000);
    for model in [
        "gemini-4-carbon",
        "carbon-low",
        "gemini-4-carbon-high",
        "gemini-4-argon",
        "argon-low",
        "gemini-4-argon-high",
    ] {
        provider.set_model(model).unwrap();
        assert_eq!(provider.context_window(), 1_000_000, "model={model}");
    }
    provider.set_model("chat_20706").unwrap();
    assert_eq!(provider.context_window(), 16_384);
}

#[test]
fn resolve_model_for_request_skips_alias_when_canonical_target_is_unavailable() {
    // Gemini-preferred fallback branch: available `gemini-4-carbon` alias whose
    // canonical `gemini-4-carbon-medium` target is unavailable must be skipped
    // in favor of available `gemini-3-flash`.
    let provider = AntigravityProvider {
        client: reqwest::Client::new(),
        model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        fetched_catalog: Arc::new(RwLock::new(vec![
            test_catalog_entry("gemini-4-carbon", true),
            test_catalog_entry("gemini-4-carbon-medium", false),
            test_catalog_entry("gemini-3-flash", true),
        ])),
        backend_default_model: Arc::new(RwLock::new(Some("gemini-4-carbon".to_string()))),
    };
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-3-flash"
    );
    assert_eq!(provider.resolve_model_for_request(""), "gemini-3-flash");

    // General non-Gemini catalog fallback branch: when the only Gemini entries
    // map to an unavailable canonical target, fallback continues to the next
    // available non-Gemini catalog entry.
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = None;
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        test_catalog_entry("gemini-4-argon", true),
        test_catalog_entry("gemini-4-argon-medium", false),
        test_catalog_entry("gpt-oss-120b-medium", true),
    ];
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gpt-oss-120b-medium"
    );
    assert_eq!(
        provider.resolve_model_for_request(""),
        "gpt-oss-120b-medium"
    );
}
