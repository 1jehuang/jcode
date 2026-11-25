use super::*;

#[test]
fn named_openai_compatible_model_context_window_overrides_default() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut config = jcode_base::config::NamedProviderConfig {
        base_url: "https://compat.example.test/v1".to_string(),
        api_key: Some("test".to_string()),
        default_model: Some("custom-long-context".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "custom-long-context".to_string(),
            context_window: Some(512_000),
            reasoning: None,
            reasoning_effort: None,
            input: Vec::new(),
        }],
        ..Default::default()
    };
    config.model_catalog = false;

    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &config).expect("provider");

    assert_eq!(provider.context_window(), 512_000);
}

#[test]
fn vsllm_uses_conservative_context_when_catalog_omits_limits() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", temp.path());
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://vsllm.cc/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        model_catalog: true,
        default_model: Some("vsllm-future-2099".to_string()),
        ..Default::default()
    };
    let mut provider = OpenRouterProvider::new_named_openai_compatible("vsllm-api", &profile)
        .expect("VSLLM profile should initialize");
    provider
        .set_model("vsllm-future-2099")
        .expect("VSLLM accepts catalog model ids");
    assert_eq!(provider.context_window(), 32_768);
    provider
        .static_context_limits
        .insert("vsllm-future-2099".into(), 65_536);
    assert_eq!(provider.context_window(), 65_536);
    provider.static_context_limits.clear();
    provider.models_cache.blocking_write().models.push(
        serde_json::from_value(serde_json::json!({
            "id": "vsllm-future-2099",
            "context_length": 98_304
        }))
        .unwrap(),
    );
    assert_eq!(provider.context_window(), 98_304);
}
