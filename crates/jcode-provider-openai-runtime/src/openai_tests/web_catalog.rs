// Offline runtime regressions. Never invoke a browser bridge.

fn seed_browser_catalog(home: &std::path::Path, model: &str, label: &str) {
    jcode_base::storage::write_json(
        &home.join("chatgpt_web_model_catalog_cache.json"),
        &serde_json::json!({
            "version": 1,
            "observed_at": 1,
            "models": [{"model": model, "label": label, "available": true,
                "detail": "fixture", "expected_upstream_models": []}],
            "reasoning": {model: [
                {"value":"medium", "label":"Medium", "available":true},
                {"value":"high", "label":"High", "available":true},
                {"value":"max", "label":"Pro", "available":false}
            ]}
        }),
    )
    .unwrap();
}

#[test]
fn chatgpt_web_refresh_failure_does_not_block_api_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");
    let provider = OpenAIProvider::new(api_key_creds("sk-refresh-error"));
    assert!(
        !provider
            .apply_browser_catalog_refresh(Err(anyhow::anyhow!("offline")))
            .unwrap()
    );
}

#[test]
fn chatgpt_web_refresh_failure_is_returned_for_browser_transport() {
    let _guard = jcode_base::storage::lock_test_env();
    let provider = OpenAIProvider::new_browser_only();
    assert_eq!(
        provider
            .apply_browser_catalog_refresh(Err(anyhow::anyhow!("offline")))
            .unwrap_err()
            .to_string(),
        "offline"
    );
}

#[test]
fn chatgpt_web_refresh_replaces_only_an_unset_default_with_an_observed_model() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");
    let provider = OpenAIProvider::new_browser_only();
    seed_browser_catalog(temp.path(), "gpt-6[web]", "GPT-6");
    let snapshot: serde_json::Value = jcode_base::storage::read_json(&temp.path().join("chatgpt_web_model_catalog_cache.json")).unwrap();
    let models = serde_json::from_value(snapshot["models"].clone()).unwrap();
    assert!(provider.apply_browser_catalog_refresh(Ok(models)).unwrap());
    assert_eq!(provider.model(), "gpt-6[web]");
}

#[test]
fn chatgpt_web_empty_refresh_does_not_invent_an_available_default() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");
    let provider = OpenAIProvider::new_browser_only();
    assert!(
        provider
            .apply_browser_catalog_refresh(Ok(Vec::new()))
            .unwrap_err()
            .to_string()
            .contains("No selectable")
    );
}

#[test]
fn chatgpt_web_dynamic_model_selection_uses_discovered_power() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");
    seed_browser_catalog(temp.path(), "gpt-6[web]", "GPT-6");
    let provider = OpenAIProvider::new_browser_only();

    assert_eq!(provider.model(), "gpt-6[web]");
    assert_eq!(provider.available_models_for_switching(), ["gpt-6[web]"]);
    assert_eq!(provider.transport().as_deref(), Some("browser"));
    assert_eq!(provider.reasoning_effort().as_deref(), Some("medium"));
    assert_eq!(
        provider.available_efforts(),
        ["medium", "high", "swarm", "swarm-deep"]
    );
    provider.set_reasoning_effort("high").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));
    assert!(provider.set_reasoning_effort("max").is_err());
    assert!(provider.set_reasoning_effort("low").is_err());
    assert!(!provider.supports_image_input());
}

#[test]
fn chatgpt_web_saved_model_override_survives_missing_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _model = EnvVarGuard::set("JCODE_OPENAI_MODEL", "  o4-mini[web]  ");
    let provider = OpenAIProvider::new_browser_only();
    assert_eq!(provider.model(), "o4-mini[web]");
    assert!(provider.available_models_for_switching().is_empty());
    assert_eq!(provider.reasoning_effort(), None);
}

#[test]
fn chatgpt_web_api_catalog_keeps_discovered_browser_models_without_static_web_ids() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");
    let _key = EnvVarGuard::remove("OPENAI_API_KEY");
    seed_browser_catalog(temp.path(), "gpt-6[web]", "GPT-6");
    let provider = OpenAIProvider::new(api_key_creds("sk-browser-catalog-fixture"));
    jcode_base::provider::populate_account_models_for_scope(
        &provider.catalog_scope(),
        vec!["gpt-5.4".into(), CHATGPT_WEB_MODEL.into()],
    );
    let models = provider.available_models_for_switching();
    assert_eq!(
        models.iter().filter(|model| *model == "gpt-6[web]").count(),
        1
    );
    assert!(models.contains(&"gpt-5.4".to_string()));
    assert!(!models.contains(&CHATGPT_WEB_MODEL.to_string()));
    provider.set_model("gpt-6[web]").unwrap();
    assert_eq!(provider.transport().as_deref(), Some("browser"));
}
