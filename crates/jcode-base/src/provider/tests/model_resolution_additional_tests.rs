use super::*;

#[test]
fn test_provider_for_model_claude() {
    assert_eq!(provider_for_model("claude-opus-4-6"), Some("claude"));
    assert_eq!(provider_for_model("claude-opus-4-6[1m]"), Some("claude"));
    assert_eq!(provider_for_model("claude-sonnet-4-6"), Some("claude"));
}

#[test]
fn session_route_catalog_alias_selects_final_runtime_from_openrouter() {
    with_clean_provider_test_env(|| {
        crate::config::invalidate_config_cache();
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        crate::env::set_var("OPENROUTER_API_KEY", "test-openrouter-key");
        crate::env::set_var("ZHIPU_API_KEY", "test-zai-key");
        let provider = test_multi_provider_with_openai();
        provider.set_model("openrouter:z-ai/glm-4.7").unwrap();

        let request = MultiProvider::model_switch_request_for_session_route(
            "z-ai:glm-4.7",
            Some("openrouter"),
            Some("openrouter"),
        );
        provider.set_model(&request).unwrap();

        assert_eq!(
            (
                ProviderRegistry::new(&provider).active_compatible_profile_id(),
                provider
                    .active_openrouter_execution_provider()
                    .unwrap()
                    .direct_openai_compatible_route_parts()
                    .unwrap()
                    .1,
                provider.model()
            ),
            (
                Some("zai".to_string()),
                "openai-compatible:zai".to_string(),
                "glm-4.7".to_string()
            ),
        );
    });
}

#[test]
fn session_route_catalog_alias_replaces_existing_direct_runtime() {
    with_clean_provider_test_env(|| {
        crate::config::invalidate_config_cache();
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        crate::env::set_var("DEEPSEEK_API_KEY", "test-deepseek-key");
        crate::env::set_var("ZHIPU_API_KEY", "test-zai-key");
        let provider = test_multi_provider_with_openai();
        provider.set_model("deepseek:deepseek-chat").unwrap();

        let request = MultiProvider::model_switch_request_for_session_route(
            "z-ai:glm-4.7",
            Some("deepseek"),
            Some("openai-compatible:deepseek"),
        );
        provider.set_model(&request).unwrap();

        assert_eq!(
            (
                ProviderRegistry::new(&provider).active_compatible_profile_id(),
                provider
                    .active_openrouter_execution_provider()
                    .unwrap()
                    .direct_openai_compatible_route_parts()
                    .unwrap()
                    .1,
                provider.model()
            ),
            (
                Some("zai".to_string()),
                "openai-compatible:zai".to_string(),
                "glm-4.7".to_string()
            ),
        );
    });
}

#[test]
fn session_route_named_profile_keeps_catalog_alias_precedence() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let home = std::env::var_os("JCODE_HOME").unwrap();
        std::fs::write(
            std::path::PathBuf::from(home).join("config.toml"),
            r#"
[providers.z-ai]
type = "openai-compatible"
base_url = "https://named-provider.invalid/v1"
auth = "none"
default_model = "glm-4.7"
"#,
        )
        .unwrap();
        crate::config::invalidate_config_cache();
        let provider = test_multi_provider_with_openai();

        let request = MultiProvider::model_switch_request_for_session_route(
            "z-ai:glm-4.7",
            Some("openrouter"),
            Some("openrouter"),
        );
        provider.set_model(&request).unwrap();

        assert_eq!(
            (
                ProviderRegistry::new(&provider).active_compatible_profile_id(),
                provider
                    .active_openrouter_execution_provider()
                    .unwrap()
                    .direct_openai_compatible_route_parts()
                    .unwrap()
                    .1,
                provider.model()
            ),
            (
                Some("z-ai".to_string()),
                "openai-compatible:z-ai".to_string(),
                "glm-4.7".to_string()
            ),
        );
    });
}

#[test]
fn session_model_catalog_alias_restores_canonical_runtime_identity() {
    with_clean_provider_test_env(|| {
        crate::config::invalidate_config_cache();
        assert_eq!(
            MultiProvider::model_switch_request_for_session_model(
                "z-ai:glm-4.7",
                Some("openrouter")
            ),
            "zai:glm-4.7",
        );
    });
}

#[test]
fn session_provider_catalog_alias_restores_canonical_runtime_identity() {
    with_clean_provider_test_env(|| {
        crate::config::invalidate_config_cache();
        assert_eq!(
            MultiProvider::model_switch_request_for_session_model("glm-4.7", Some("z-ai")),
            "zai:glm-4.7",
        );
    });
}

#[test]
fn session_route_catalog_alias_variants_use_canonical_prefix() {
    with_clean_provider_test_env(|| {
        crate::config::invalidate_config_cache();
        for alias in ["z-ai", "z.ai", "zhipu", "zai-coding", "Z-AI"] {
            assert_eq!(
                MultiProvider::model_switch_request_for_session_route(
                    &format!("{alias}:glm-4.7"),
                    Some("openrouter"),
                    Some("openrouter"),
                ),
                "zai:glm-4.7",
                "{alias}",
            );
        }
    });
}

#[test]
fn session_model_named_profile_keeps_catalog_alias_precedence() {
    with_clean_provider_test_env(|| {
        let home = std::env::var_os("JCODE_HOME").unwrap();
        std::fs::write(
            std::path::PathBuf::from(home).join("config.toml"),
            r#"
[providers.z-ai]
type = "openai-compatible"
base_url = "https://named-provider.invalid/v1"
auth = "none"
"#,
        )
        .unwrap();
        crate::config::invalidate_config_cache();
        assert_eq!(
            MultiProvider::model_switch_request_for_session_model(
                "z-ai:glm-4.7",
                Some("openrouter")
            ),
            "z-ai:glm-4.7",
        );
    });
}

#[test]
fn session_provider_named_profile_keeps_catalog_alias_precedence() {
    with_clean_provider_test_env(|| {
        let home = std::env::var_os("JCODE_HOME").unwrap();
        std::fs::write(
            std::path::PathBuf::from(home).join("config.toml"),
            r#"
[providers.z-ai]
type = "openai-compatible"
base_url = "https://named-provider.invalid/v1"
auth = "none"
"#,
        )
        .unwrap();
        crate::config::invalidate_config_cache();
        assert_eq!(
            MultiProvider::model_switch_request_for_session_model("glm-4.7", Some("z-ai")),
            "z-ai:glm-4.7",
        );
    });
}

#[test]
fn test_provider_for_model_openai() {
    assert_eq!(provider_for_model("gpt-5.2-codex"), Some("openai"));
    assert_eq!(provider_for_model("gpt-5.5"), Some("openai"));
    assert_eq!(provider_for_model("gpt-5.4"), Some("openai"));
    assert_eq!(provider_for_model("gpt-5.4[1m]"), Some("openai"));
    assert_eq!(provider_for_model("gpt-5.4-pro"), Some("openai"));
}

#[test]
fn test_provider_for_model_gemini() {
    assert_eq!(provider_for_model("gemini-2.5-pro"), Some("gemini"));
    assert_eq!(provider_for_model("gemini-2.5-flash"), Some("gemini"));
    assert_eq!(provider_for_model("gemini-3-pro-preview"), Some("gemini"));
}
