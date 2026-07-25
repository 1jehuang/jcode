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

/// A built-in profile keeps replace-after-fetch semantics: its `static_models`
/// are a pre-catalog fallback, not a user-authored list.
#[test]
fn builtin_profile_is_not_treated_as_user_named() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("CEREBRAS_API_KEY", "test-key");

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://api.cerebras.ai/v1".to_string(),
        api_key_env: Some("CEREBRAS_API_KEY".to_string()),
        model_catalog: true,
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("cerebras", &profile)
        .expect("builtin-shaped profile should initialize");
    assert!(!provider.is_user_named_profile());
    assert!(!provider.should_merge_static_models_with_live_catalog());
}

/// A `[providers.cerebras]` block that shadows a built-in name but points at a
/// different endpoint is still user-declared, so its models must be preserved.
#[test]
fn profile_shadowing_builtin_name_with_other_base_is_user_named() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_SHADOW_KEY", "test-key");

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://proxy.internal.test/v1".to_string(),
        api_key_env: Some("TEST_SHADOW_KEY".to_string()),
        model_catalog: true,
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "shadowed-model".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("cerebras", &profile)
        .expect("shadowing profile should initialize");
    assert!(provider.is_user_named_profile());
    assert!(provider.should_merge_static_models_with_live_catalog());
}
