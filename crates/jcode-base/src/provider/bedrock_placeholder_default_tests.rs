use super::*;

fn route(model: &str, available: bool) -> ModelRoute {
    ModelRoute {
        catalog_metadata: None,
        model: model.to_string(),
        provider: "AWS Bedrock".to_string(),
        api_method: "bedrock".to_string(),
        available,
        detail: String::new(),
        usage: None,
        cheapness: None,
    }
}

#[test]
fn bedrock_placeholder_promotes_to_newest_usable_claude() {
    let routes = vec![
        route(bedrock::BedrockProvider::PLACEHOLDER_DEFAULT_MODEL, true),
        route("anthropic.claude-sonnet-4-20250514-v1:0", true),
        route("us.anthropic.claude-opus-6-v1", true),
        route("amazon.nova-pro-v1:0", true),
    ];
    assert_eq!(
        bedrock_promoted_default(bedrock::BedrockProvider::PLACEHOLDER_DEFAULT_MODEL, &routes)
            .as_deref(),
        Some("us.anthropic.claude-opus-6-v1")
    );
}

#[test]
fn bedrock_placeholder_skips_unusable_routes() {
    // A newer Opus that needs a missing inference profile is unavailable
    // and must not become the default.
    let routes = vec![
        route("anthropic.claude-opus-5-20260101-v1:0", false),
        route("anthropic.claude-sonnet-4-20250514-v1:0", true),
    ];
    assert_eq!(
        bedrock_promoted_default(bedrock::BedrockProvider::PLACEHOLDER_DEFAULT_MODEL, &routes)
            .as_deref(),
        Some("anthropic.claude-sonnet-4-20250514-v1:0")
    );
}

/// End to end through the real construction path: a cached Bedrock catalog
/// on disk must make a freshly built provider start on the newest Claude
/// flagship instead of the 2024 placeholder; with no catalog it stays put.
#[test]
fn new_bedrock_provider_starts_on_newest_cached_flagship() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let keys = [
        "JCODE_HOME",
        "JCODE_BEDROCK_MODEL",
        "JCODE_BEDROCK_REGION",
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
    ];
    let saved: Vec<_> = keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
    for key in &keys[1..] {
        crate::env::remove_var(key);
    }
    crate::env::set_var("JCODE_HOME", temp.path());

    let fresh = new_bedrock_provider();
    assert_eq!(
        fresh.model(),
        bedrock::BedrockProvider::PLACEHOLDER_DEFAULT_MODEL,
        "no catalog: keep placeholder rather than guess from the known list"
    );

    let cache = crate::storage::app_config_dir()
        .expect("config dir")
        .join("bedrock_models_cache.json");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(
        &cache,
        serde_json::json!({
            "models": [
                "anthropic.claude-3-5-sonnet-20241022-v2:0",
                "anthropic.claude-sonnet-4-20250514-v1:0",
                "anthropic.claude-opus-6-v1:0",
            ],
            "inference_profiles": [],
            "region": null,
            "fetched_at_rfc3339": "2026-10-01T00:00:00Z",
        })
        .to_string(),
    )
    .unwrap();
    let promoted = new_bedrock_provider().model();

    crate::env::set_var(
        "JCODE_BEDROCK_MODEL",
        "anthropic.claude-3-5-haiku-20241022-v1:0",
    );
    let pinned = new_bedrock_provider().model();

    for (key, value) in saved {
        match value {
            Some(value) => crate::env::set_var(key, value),
            None => crate::env::remove_var(key),
        }
    }
    assert_eq!(promoted, "anthropic.claude-opus-6-v1:0");
    assert_eq!(pinned, "anthropic.claude-3-5-haiku-20241022-v1:0");
}

#[test]
fn bedrock_explicit_model_is_never_replaced() {
    let routes = vec![route("us.anthropic.claude-opus-6-v1", true)];
    assert_eq!(
        bedrock_promoted_default("anthropic.claude-sonnet-4-20250514-v1:0", &routes),
        None
    );
}
