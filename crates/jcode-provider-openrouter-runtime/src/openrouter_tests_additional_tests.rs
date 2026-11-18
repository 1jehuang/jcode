use super::*;

#[test]
fn cache_breakpoint_preserves_shared_payload_when_already_marked() {
    let original = Message {
        role: Role::User,
        content: vec![
            ContentBlock::ToolResult {
                tool_use_id: "large-output".into(),
                content: "x".repeat(1024 * 1024),
                is_error: None,
            },
            ContentBlock::Text {
                text: "continue".into(),
                cache_control: Some(CacheControl::ephemeral(Some("1h".into()))),
            },
        ]
        .into(),
        timestamp: None,
        tool_duration_ms: None,
    };
    let mut request = vec![original.clone()];
    let before = serde_json::to_vec(&request).unwrap();
    assert!(add_cache_breakpoint(&mut request));
    assert!(request[0].content.shares_storage_with(&original.content));
    assert_eq!(serde_json::to_vec(&request).unwrap(), before);
}

#[test]
fn cache_breakpoint_detaches_only_when_a_marker_is_missing() {
    let original = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "continue".into(),
            cache_control: None,
        }]
        .into(),
        timestamp: None,
        tool_duration_ms: None,
    };
    let mut request = vec![original.clone()];
    assert!(add_cache_breakpoint(&mut request));
    assert!(!request[0].content.shares_storage_with(&original.content));
    assert!(matches!(
        &original.content[0],
        ContentBlock::Text {
            cache_control: None,
            ..
        }
    ));
    let marked = request[0].clone();
    assert!(add_cache_breakpoint(&mut request));
    assert!(request[0].content.shares_storage_with(&marked.content));
    assert!(!add_cache_breakpoint(&mut [Message {
        role: Role::Assistant,
        ..original
    }]));
}

#[test]
fn test_has_credentials() {
    let _has_creds = OpenRouterProvider::has_credentials();
}

#[test]
fn named_openai_compatible_provider_sets_catalog_cache_namespace() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_COMPAT_KEY", "test-key");

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key_env: Some("TEST_NAMED_COMPAT_KEY".to_string()),
        model_catalog: true,
        default_model: Some("example-model".to_string()),
        ..Default::default()
    };

    let _provider = OpenRouterProvider::new_named_openai_compatible("example-compat", &profile)
        .expect("named profile should initialize");

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE").as_deref(),
        Ok("example-compat")
    );
}

#[test]
fn named_openai_compatible_provider_exposes_static_models_as_routes() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_COMPAT_KEY", "test-key");

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key_env: Some("TEST_NAMED_COMPAT_KEY".to_string()),
        model_catalog: true,
        default_model: Some("glm-51-nvfp4".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "glm-51-nvfp4".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("comtegra-test", &profile)
        .expect("named profile should initialize");
    let routes = provider.model_routes();

    assert!(routes.iter().any(|route| {
        route.model == "glm-51-nvfp4"
            && route.api_method == "openai-compatible:comtegra-test"
            && route.available
    }));
}
