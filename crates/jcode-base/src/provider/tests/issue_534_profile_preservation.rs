#[test]
fn test_named_profile_survives_failed_openrouter_switch_then_transitions_on_success() {
    with_clean_provider_test_env(|| {
        let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
        std::fs::write(
            std::path::PathBuf::from(jcode_home).join("config.toml"),
            r#"
[providers.my-gateway]
type = "openai-compatible"
base_url = "https://example.com/proxy/openai"
auth = "none"
default_model = "vendor/my-model"

[[providers.my-gateway.models]]
id = "vendor/my-model"
input = ["text"]
"#,
        )
        .expect("write test config.toml");
        crate::config::invalidate_config_cache();
        external::register_openrouter_factory(|spec| {
            use external::OpenRouterRuntimeSpec;
            use jcode_provider_openrouter_runtime::OpenRouterProvider;
            let provider: Arc<dyn Provider> = match spec {
                OpenRouterRuntimeSpec::OpenRouterApiKey => {
                    if std::env::var_os("OPENROUTER_API_KEY").is_none() {
                        anyhow::bail!("OPENROUTER_API_KEY is not configured");
                    }
                    Arc::new(StubExternalRuntime::new(
                        "openrouter",
                        "OpenRouter",
                        "openrouter",
                        &["openrouter/owl-alpha"],
                    ))
                }
                OpenRouterRuntimeSpec::Default => Arc::new(OpenRouterProvider::new()?),
                OpenRouterRuntimeSpec::CompatibleProfile(profile) => Arc::new(
                    OpenRouterProvider::new_openai_compatible_profile_runtime(profile)?,
                ),
                OpenRouterRuntimeSpec::NamedProfile { name, config } => Arc::new(
                    OpenRouterProvider::new_named_openai_compatible(&name, &config)?,
                ),
            };
            Ok(provider)
        });

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Claude),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            initial_provider: None,
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        provider
            .set_model("my-gateway:vendor/my-model")
            .expect("named profile model should be selectable");
        assert_eq!(
            ProviderRegistry::new(&provider).active_compatible_profile_id(),
            Some("my-gateway".to_string())
        );

        let err = provider
            .set_model("openrouter:openrouter/owl-alpha")
            .expect_err("OpenRouter switch without credentials should fail");
        assert!(
            err.to_string().contains("OPENROUTER_API_KEY"),
            "unexpected missing-credential error: {err:#}"
        );
        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
        assert_eq!(provider.model(), "vendor/my-model");
        assert_eq!(
            ProviderRegistry::new(&provider).active_compatible_profile_id(),
            Some("my-gateway".to_string()),
            "failed switch must preserve the active named profile"
        );

        crate::env::set_var("OPENROUTER_API_KEY", "test-openrouter-key");
        let err = provider
            .set_model("openrouter:not-in-openrouter-catalog")
            .expect_err("OpenRouter switch with an invalid model should fail");
        assert!(
            err.to_string().contains("Unsupported OpenRouter model"),
            "unexpected model-validation error: {err:#}"
        );
        assert_eq!(provider.model(), "vendor/my-model");
        assert_eq!(
            ProviderRegistry::new(&provider).active_compatible_profile_id(),
            Some("my-gateway".to_string()),
            "failed model validation must preserve the active named profile"
        );
        assert!(
            provider.openrouter_provider().is_none(),
            "failed model validation must not install the candidate OpenRouter runtime"
        );

        provider
            .set_model("openrouter:openrouter/owl-alpha")
            .expect("credentialed OpenRouter switch should succeed");
        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
        assert_eq!(provider.model(), "openrouter/owl-alpha");
        assert_eq!(
            ProviderRegistry::new(&provider).active_compatible_profile_id(),
            None,
            "successful switch must clear the prior named profile"
        );
    });
}
