//! Regression tests for static-model / live-catalog merge behavior
//! across built-in and user-declared OpenAI-compatible provider profiles.

use crate::tests::{ENV_LOCK, EnvVarGuard};
use crate::*;

#[test]
fn named_profile_static_models_survive_live_catalog_refresh() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_MERGE_KEY", "test-key");

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://llm.example.test/v1".to_string(),
        api_key_env: Some("TEST_NAMED_MERGE_KEY".to_string()),
        model_catalog: true,
        default_model: Some("my-custom-model".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "my-custom-model".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("user-compat", &profile)
        .expect("named profile should initialize");
    assert!(provider.is_user_named_profile());
    assert!(provider.should_merge_static_models_with_live_catalog());

    // Simulate a completed background `/models` catalog refresh that does not
    // include the user's config-declared model.
    {
        let mut cache = provider.models_cache.blocking_write();
        cache.models = vec![jcode_provider_openrouter::ModelInfo {
            id: "vendor-live-model".to_string(),
            name: "vendor live model".to_string(),
            context_length: Some(128_000),
            pricing: Default::default(),
            created: None,
        }];
        cache.fetched = true;
        cache.cached_at = Some(1);
    }

    let models = provider.available_models_display();
    assert!(
        models.iter().any(|m| m == "my-custom-model"),
        "config-declared model should survive live catalog refresh: {models:?}"
    );
    assert!(models.iter().any(|m| m == "vendor-live-model"));
}
