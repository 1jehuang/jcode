#[test]
fn test_provider_for_model_claude() {
    assert_eq!(provider_for_model("claude-opus-4-6"), Some("claude"));
    assert_eq!(provider_for_model("claude-opus-4-6[1m]"), Some("claude"));
    assert_eq!(provider_for_model("claude-sonnet-4-6"), Some("claude"));
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

#[test]
fn test_provider_for_model_bedrock() {
    assert_eq!(provider_for_model("amazon.nova-pro-v1:0"), Some("bedrock"));
    assert_eq!(
        provider_for_model("us.amazon.nova-micro-v1:0"),
        Some("bedrock")
    );
    assert_eq!(
        provider_for_model(
            "arn:aws:bedrock:us-east-2:302154194530:inference-profile/us.deepseek.r1-v1:0"
        ),
        Some("bedrock")
    );
}

#[test]
fn test_provider_for_model_openrouter() {
    // OpenRouter uses provider/model format
    assert_eq!(
        provider_for_model("anthropic/claude-sonnet-4"),
        Some("openrouter")
    );
    assert_eq!(provider_for_model("openai/gpt-4o"), Some("openrouter"));
    assert_eq!(
        provider_for_model("google/gemini-2.0-flash"),
        Some("openrouter")
    );
    assert_eq!(
        provider_for_model("meta-llama/llama-3.1-405b"),
        Some("openrouter")
    );
}

#[test]
fn test_openrouter_catalog_model_id_normalizes_bare_openai_and_claude_models() {
    assert_eq!(
        openrouter_catalog_model_id("gpt-5.4").as_deref(),
        Some("openai/gpt-5.4")
    );
    assert_eq!(
        openrouter_catalog_model_id("claude-sonnet-4-6").as_deref(),
        Some("anthropic/claude-sonnet-4-6")
    );
    assert_eq!(
        openrouter_catalog_model_id("anthropic/claude-sonnet-4").as_deref(),
        Some("anthropic/claude-sonnet-4")
    );
    assert_eq!(
        openrouter_catalog_model_id(
            "arn:aws:bedrock:us-east-2:302154194530:inference-profile/us.deepseek.r1-v1:0"
        ),
        None
    );
    assert_eq!(openrouter_catalog_model_id("composer-2-fast"), None);
}

#[test]
fn test_available_models_display_uses_route_models_and_filters_placeholder_rows() {
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
        active: RwLock::new(ActiveProvider::OpenAI),
        use_claude_cli: false,
        startup_notices: RwLock::new(Vec::new()),
        forced_provider: None,
    };

    let models = provider.available_models_display();
    assert!(
        models
            .iter()
            .any(|model| known_openai_model_ids().contains(model)),
        "route-backed display models should include OpenAI picker rows: {:?}",
        models
    );
    assert!(
        models
            .iter()
            .any(|model| known_anthropic_model_ids().contains(model)),
        "route-backed display models should include Anthropic picker rows: {:?}",
        models
    );
    assert!(!models.iter().any(|model| model == "openrouter models"));
    assert!(!models.iter().any(|model| model == "copilot models"));
}

#[test]
fn test_cerebras_model_routes_are_profile_scoped_and_unique() {
    with_clean_provider_test_env(|| {
        with_env_var("CEREBRAS_API_KEY", "test-cerebras-key", || {
            crate::provider_catalog::force_apply_openai_compatible_profile_env(
                crate::provider_catalog::openai_compatible_profile_by_id("cerebras"),
            );
            let openrouter = Arc::new(
                openrouter::OpenRouterProvider::new()
                    .expect("Cerebras direct provider should initialize"),
            );
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(None),
                bedrock: RwLock::new(None),
                openrouter: RwLock::new(Some(openrouter)),
                openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                active_openai_compatible_profile: RwLock::new(None),
                active: RwLock::new(ActiveProvider::OpenRouter),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: Some(ActiveProvider::OpenRouter),
            };

            let routes = provider.model_routes();
            let qwen_routes = routes
                .iter()
                .filter(|route| {
                    route.provider == "Cerebras" && route.model == "qwen-3-235b-a22b-instruct-2507"
                })
                .collect::<Vec<_>>();
            assert_eq!(
                qwen_routes.len(),
                1,
                "Cerebras direct route should not appear twice in provider routes: {routes:?}"
            );
            assert_eq!(qwen_routes[0].api_method, "openai-compatible:cerebras");
            assert!(qwen_routes[0].available);
            assert!(
                !routes.iter().any(|route| {
                    route.provider == "Cerebras" && route.api_method == "openai-compatible"
                }),
                "generic Cerebras OpenAI-compatible route should be collapsed into the profile-scoped route: {routes:?}"
            );
        })
    });
}

#[test]
fn test_direct_chutes_ignores_legacy_openrouter_catalog_cache() {
    with_clean_provider_test_env(|| {
        let temp_home = tempfile::tempdir().expect("temp HOME");
        let home = temp_home.path().to_string_lossy().to_string();
        with_env_var("HOME", &home, || {
            let cache_dir = temp_home.path().join(".jcode").join("cache");
            std::fs::create_dir_all(&cache_dir).expect("create cache dir");
            let cached_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_secs();
            std::fs::write(
                cache_dir.join("chutes_models.json"),
                serde_json::json!({
                    "cached_at": cached_at,
                    "models": [
                        { "id": "openai/gpt-chat-latest" },
                        { "id": "anthropic/claude-sonnet-latest" },
                        { "id": "openrouter/owl-alpha" }
                    ]
                })
                .to_string(),
            )
            .expect("write legacy chutes cache");

            with_env_var("CHUTES_API_KEY", "test-chutes-key", || {
                let openrouter = Arc::new(
                    openrouter::OpenRouterProvider::new()
                        .expect("autodetected Chutes provider should initialize"),
                );
                let direct_route = openrouter
                    .direct_openai_compatible_route_parts()
                    .expect("Chutes should initialize as a direct profile");
                assert_eq!(direct_route.0, "Chutes");
                assert_eq!(direct_route.1, "openai-compatible:chutes");

                let display_models = openrouter.available_models_display();
                assert!(
                    !display_models
                        .iter()
                        .any(|model| model == "openai/gpt-chat-latest"),
                    "legacy source-less Chutes cache must not be trusted as a direct Chutes catalog: {display_models:?}"
                );

                let provider = MultiProvider {
                    claude: RwLock::new(None),
                    anthropic: RwLock::new(None),
                    openai: RwLock::new(None),
                    copilot_api: RwLock::new(None),
                    antigravity: RwLock::new(None),
                    gemini: RwLock::new(None),
                    cursor: RwLock::new(None),
                    bedrock: RwLock::new(None),
                    openrouter: RwLock::new(Some(openrouter)),
                    openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                    active_openai_compatible_profile: RwLock::new(None),
                    active: RwLock::new(ActiveProvider::OpenRouter),
                    use_claude_cli: false,
                    startup_notices: RwLock::new(Vec::new()),
                    forced_provider: Some(ActiveProvider::OpenRouter),
                };

                let routes = provider.model_routes();
                assert!(routes.iter().any(|route| {
                    route.provider == "Chutes"
                        && route.api_method == "openai-compatible:chutes"
                        && route.available
                }));
                assert!(
                    !routes.iter().any(|route| {
                        route.provider == "Chutes" && route.model == "openai/gpt-chat-latest"
                    }),
                    "stale OpenRouter catalog entries must not be relabeled as Chutes routes: {routes:?}"
                );
                assert!(
                    !routes.iter().any(|route| {
                        route.api_method == "openrouter"
                            && matches!(route.provider.as_str(), "OpenAI" | "Anthropic")
                    }),
                    "direct Chutes profiles must not add OpenRouter fallback routes: {routes:?}"
                );
            })
        })
    });
}

#[test]
fn test_auth_changed_preserves_existing_direct_profile_session() {
    with_clean_provider_test_env(|| {
        let cerebras = crate::provider_catalog::openai_compatible_profile_by_id("cerebras")
            .expect("Cerebras profile exists");
        let groq = crate::provider_catalog::openai_compatible_profile_by_id("groq")
            .expect("Groq profile exists");

        crate::env::set_var("CEREBRAS_API_KEY", "test-cerebras-key");
        crate::provider_catalog::force_apply_openai_compatible_profile_env(Some(cerebras));
        let openrouter = Arc::new(
            openrouter::OpenRouterProvider::new().expect("Cerebras provider should initialize"),
        );
        openrouter
            .set_model("qwen-3-235b-a22b-instruct-2507")
            .expect("Cerebras model should be selectable");

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(Some(openrouter)),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenRouter),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::OpenRouter),
        };

        crate::env::set_var("GROQ_API_KEY", "test-groq-key");
        crate::provider_catalog::force_apply_openai_compatible_profile_env(Some(groq));
        provider.on_auth_changed_preserve_current_provider();

        assert_eq!(provider.model(), "qwen-3-235b-a22b-instruct-2507");
        let active_direct_route = provider
            .openrouter_provider()
            .expect("existing direct provider remains installed")
            .direct_openai_compatible_route_parts()
            .expect("existing direct provider remains direct");
        assert_eq!(active_direct_route.0, "Cerebras");
        assert_eq!(active_direct_route.1, "openai-compatible:cerebras");

        let routes = provider.model_routes();
        assert!(routes.iter().any(|route| {
            route.model == "qwen-3-235b-a22b-instruct-2507"
                && route.provider == "Cerebras"
                && route.api_method == "openai-compatible:cerebras"
                && route.available
        }));
        assert!(
            routes.iter().all(|route| {
                !(route.model == "qwen-3-235b-a22b-instruct-2507" && route.provider == "Groq")
            }),
            "Groq auth should not relabel an existing Cerebras session route: {routes:?}"
        );
    });
}

#[test]
fn test_auth_changed_replaces_template_direct_profile_for_new_logins() {
    with_clean_provider_test_env(|| {
        let cerebras = crate::provider_catalog::openai_compatible_profile_by_id("cerebras")
            .expect("Cerebras profile exists");
        let groq = crate::provider_catalog::openai_compatible_profile_by_id("groq")
            .expect("Groq profile exists");

        crate::env::set_var("CEREBRAS_API_KEY", "test-cerebras-key");
        crate::provider_catalog::force_apply_openai_compatible_profile_env(Some(cerebras));
        let openrouter = Arc::new(
            openrouter::OpenRouterProvider::new().expect("Cerebras provider should initialize"),
        );

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(Some(openrouter)),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenRouter),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::OpenRouter),
        };

        crate::env::set_var("GROQ_API_KEY", "test-groq-key");
        crate::provider_catalog::force_apply_openai_compatible_profile_env(Some(groq));
        provider.on_auth_changed();

        let active_direct_route = provider
            .openrouter_provider()
            .expect("template direct provider remains installed")
            .direct_openai_compatible_route_parts()
            .expect("template direct provider remains direct");
        assert_eq!(active_direct_route.0, "Groq");
        assert_eq!(active_direct_route.1, "openai-compatible:groq");
    });
}

#[test]
fn test_state_space_openrouter_default_survives_switch_to_nvidia_nim() {
    with_clean_provider_test_env(|| {
        let nvidia = crate::provider_catalog::openai_compatible_profile_by_id("nvidia-nim")
            .expect("NVIDIA NIM profile exists");

        save_test_openrouter_model_cache(
            "openrouter",
            "https://openrouter.ai/api/v1",
            &["openrouter/owl-alpha"],
        );

        crate::env::set_var("OPENROUTER_API_KEY", "test-openrouter-key");
        crate::provider_catalog::force_apply_openai_compatible_profile_env(None);
        let openrouter = Arc::new(
            openrouter::OpenRouterProvider::new().expect("OpenRouter provider should initialize"),
        );
        openrouter
            .set_model("openrouter/owl-alpha")
            .expect("OpenRouter default model should be selectable");

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(Some(openrouter)),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenRouter),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: None,
        };

        crate::env::set_var(nvidia.api_key_env, "test-nvidia-key");
        provider
            .set_model("nvidia-nim:nvidia/llama-3.1-nemotron-ultra-253b-v1")
            .expect("NVIDIA NIM model should be selectable after OpenRouter default");
        assert!(
            std::env::var_os("JCODE_OPENROUTER_API_BASE").is_none(),
            "profile route selection should not mutate global OpenRouter API base env"
        );

        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
        assert_eq!(provider.model(), "nvidia/llama-3.1-nemotron-ultra-253b-v1");
        let active_direct_route = provider
            .active_openrouter_execution_provider()
            .expect("NVIDIA direct provider is active")
            .direct_openai_compatible_route_parts()
            .expect("NVIDIA NIM is hosted through OpenAI-compatible runtime");
        assert_eq!(active_direct_route.0, "NVIDIA NIM");
        assert_eq!(active_direct_route.1, "openai-compatible:nvidia-nim");
        assert!(
            provider
                .openrouter_provider()
                .expect("real OpenRouter provider remains installed")
                .direct_openai_compatible_route_parts()
                .is_none(),
            "compatible profile must not replace the real OpenRouter slot"
        );

        let routes = provider.model_routes();
        assert!(
            routes.iter().any(|route| {
                route.model == "nvidia/llama-3.1-nemotron-ultra-253b-v1"
                    && route.provider == "NVIDIA NIM"
                    && route.api_method == "openai-compatible:nvidia-nim"
                    && route.available
            }),
            "NVIDIA route should remain selectable: {routes:?}"
        );
        assert!(
            routes.iter().any(|route| {
                route.model == "openrouter/owl-alpha"
                    && route.api_method == "openrouter"
                    && route.available
            }),
            "OpenRouter route should remain selectable after switching to NVIDIA NIM: {routes:?}"
        );
        assert!(
            routes.iter().all(|route| {
                !(route.model == "openrouter/owl-alpha" && route.provider == "NVIDIA NIM")
            }),
            "OpenRouter model must not be relabeled as NVIDIA NIM: {routes:?}"
        );

        let openrouter_route = routes
            .iter()
            .find(|route| route.model == "openrouter/owl-alpha" && route.api_method == "openrouter")
            .expect("OpenRouter route should be present after switching to NVIDIA NIM");
        let selection = crate::provider::RouteSelection::from_model_route(openrouter_route);
        provider
            .set_route_selection(&selection)
            .expect("OpenRouter route should switch runtime back to OpenRouter");
        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
        assert_eq!(provider.model(), "openrouter/owl-alpha");
        let active_direct_route = provider
            .openrouter_provider()
            .expect("OpenRouter provider remains installed")
            .direct_openai_compatible_route_parts();
        assert!(
            active_direct_route.is_none(),
            "OpenRouter model should not remain bound to NVIDIA NIM runtime: {active_direct_route:?}"
        );
    });
}

#[test]
fn test_session_route_restore_request_matrix_preserves_runtime_identity() {
    let cases = [
        (
            "claude-sonnet-4-6",
            Some("claude"),
            Some("claude-oauth"),
            "claude-oauth:claude-sonnet-4-6",
        ),
        (
            "claude-sonnet-4-6",
            Some("claude"),
            Some("anthropic-api-key"),
            "claude-api:claude-sonnet-4-6",
        ),
        (
            "gpt-5.4",
            Some("openai"),
            Some("openai-oauth"),
            "openai-oauth:gpt-5.4",
        ),
        (
            "gpt-5.4",
            Some("openai"),
            Some("openai-api-key"),
            "openai-api:gpt-5.4",
        ),
        (
            "openrouter/owl-alpha",
            Some("openrouter"),
            Some("openrouter"),
            "openrouter:openrouter/owl-alpha",
        ),
        (
            "nvidia/example",
            Some("openai-compatible:nvidia-nim"),
            Some("openai-compatible:nvidia-nim"),
            "nvidia-nim:nvidia/example",
        ),
        (
            "claude-sonnet-4",
            Some("copilot"),
            Some("copilot"),
            "copilot:claude-sonnet-4",
        ),
        (
            "composer-2.5",
            Some("cursor"),
            Some("cursor"),
            "cursor:composer-2.5",
        ),
        (
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            Some("bedrock"),
            Some("bedrock"),
            "bedrock:anthropic.claude-3-5-sonnet-20241022-v2:0",
        ),
        (
            "default",
            Some("antigravity"),
            Some("antigravity-https"),
            "antigravity:default",
        ),
    ];

    for (model, provider_key, api_method, expected) in cases {
        assert_eq!(
            MultiProvider::model_switch_request_for_session_route(model, provider_key, api_method),
            expected,
            "restore request should preserve route identity for {provider_key:?}/{api_method:?}"
        );
    }
}

#[test]
fn test_openrouter_and_compatible_profile_transition_invariants() {
    with_clean_provider_test_env(|| {
        let nvidia = crate::provider_catalog::openai_compatible_profile_by_id("nvidia-nim")
            .expect("NVIDIA NIM profile exists");

        save_test_openrouter_model_cache(
            "openrouter",
            "https://openrouter.ai/api/v1",
            &["openrouter/owl-alpha"],
        );

        crate::env::set_var("OPENROUTER_API_KEY", "test-openrouter-key");
        crate::env::set_var(nvidia.api_key_env, "test-nvidia-key");
        crate::provider_catalog::force_apply_openai_compatible_profile_env(None);
        let openrouter = Arc::new(
            openrouter::OpenRouterProvider::new().expect("OpenRouter provider should initialize"),
        );
        openrouter
            .set_model("openrouter/owl-alpha")
            .expect("OpenRouter default model should be selectable");

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(Some(openrouter.clone())),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenRouter),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: None,
        };

        provider
            .set_model("nvidia-nim:nvidia/llama-3.1-nemotron-ultra-253b-v1")
            .expect("NVIDIA NIM model should be selectable");
        assert_eq!(provider.model(), "nvidia/llama-3.1-nemotron-ultra-253b-v1");
        assert!(Arc::ptr_eq(
            &provider.openrouter_provider().expect("real OpenRouter remains"),
            &openrouter
        ));
        assert_eq!(
            provider
                .active_openrouter_execution_provider()
                .expect("active compatible runtime")
                .direct_openai_compatible_route_parts()
                .map(|(_provider, api_method, _detail)| api_method),
            Some("openai-compatible:nvidia-nim".to_string())
        );

        provider
            .set_model("openrouter:openrouter/owl-alpha")
            .expect("OpenRouter switch should select real OpenRouter slot");
        assert_eq!(provider.model(), "openrouter/owl-alpha");
        assert!(
            provider
                .active_openrouter_execution_provider()
                .expect("active OpenRouter runtime")
                .direct_openai_compatible_route_parts()
                .is_none(),
            "real OpenRouter route must not inherit compatible profile state"
        );

        provider
            .set_model("nvidia-nim:nvidia/llama-3.1-nemotron-ultra-253b-v1")
            .expect("cached compatible runtime should be selectable again");
        assert_eq!(provider.model(), "nvidia/llama-3.1-nemotron-ultra-253b-v1");
        assert!(
            provider
                .openrouter_provider()
                .expect("real OpenRouter remains")
                .direct_openai_compatible_route_parts()
                .is_none(),
            "compatible profile route must never overwrite the real OpenRouter runtime"
        );
    });
}

#[test]
fn test_set_model_accepts_bare_openai_openrouter_pin_when_openrouter_available() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            let openrouter = Arc::new(
                openrouter::OpenRouterProvider::new()
                    .expect("openrouter provider should initialize"),
            );
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(None),
                bedrock: RwLock::new(None),
                openrouter: RwLock::new(Some(openrouter)),
                openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                active_openai_compatible_profile: RwLock::new(None),
                active: RwLock::new(ActiveProvider::OpenAI),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: None,
            };

            provider
                .set_model("gpt-5.4@OpenAI")
                .expect("bare pinned OpenRouter spec should normalize");

            assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
            assert_eq!(provider.model(), "openai/gpt-5.4");
        })
    });
}

#[test]
fn test_forced_openrouter_treats_claude_like_model_as_provider_local() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            with_env_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0", || {
                with_env_var(
                    "JCODE_OPENROUTER_API_BASE",
                    "https://compat.example.test/v1",
                    || {
                        let openrouter = Arc::new(
                            openrouter::OpenRouterProvider::new()
                                .expect("custom compatible provider should initialize"),
                        );
                        let provider = MultiProvider {
                            claude: RwLock::new(None),
                            anthropic: RwLock::new(None),
                            openai: RwLock::new(None),
                            copilot_api: RwLock::new(None),
                            antigravity: RwLock::new(None),
                            gemini: RwLock::new(None),
                            cursor: RwLock::new(None),
                            bedrock: RwLock::new(None),
                            openrouter: RwLock::new(Some(openrouter)),
                            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                            active_openai_compatible_profile: RwLock::new(None),
                            active: RwLock::new(ActiveProvider::OpenRouter),
                            use_claude_cli: false,
                            startup_notices: RwLock::new(Vec::new()),
                            forced_provider: Some(ActiveProvider::OpenRouter),
                        };

                        provider.set_model("claude-opus4.6-thinking").expect(
                            "forced OpenAI-compatible provider should accept opaque model IDs",
                        );

                        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
                        assert_eq!(provider.model(), "claude-opus4.6-thinking");
                    },
                )
            })
        })
    });
}

#[test]
fn test_forced_openrouter_preserves_custom_at_sign_model_ids() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            with_env_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0", || {
                with_env_var(
                    "JCODE_OPENROUTER_API_BASE",
                    "https://compat.example.test/v1",
                    || {
                        let openrouter = Arc::new(
                            openrouter::OpenRouterProvider::new()
                                .expect("custom compatible provider should initialize"),
                        );
                        let provider = MultiProvider {
                            claude: RwLock::new(None),
                            anthropic: RwLock::new(None),
                            openai: RwLock::new(None),
                            copilot_api: RwLock::new(None),
                            antigravity: RwLock::new(None),
                            gemini: RwLock::new(None),
                            cursor: RwLock::new(None),
                            bedrock: RwLock::new(None),
                            openrouter: RwLock::new(Some(openrouter)),
                            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                            active_openai_compatible_profile: RwLock::new(None),
                            active: RwLock::new(ActiveProvider::OpenRouter),
                            use_claude_cli: false,
                            startup_notices: RwLock::new(Vec::new()),
                            forced_provider: Some(ActiveProvider::OpenRouter),
                        };

                        provider
                            .set_model("gpt-5.4@OpenAI")
                            .expect("custom compatible provider should preserve @ in model IDs");

                        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
                        assert_eq!(provider.model(), "gpt-5.4@OpenAI");
                    },
                )
            })
        })
    });
}

#[test]
fn test_config_default_provider_openai_compatible_keeps_gpt_model_provider_local() {
    with_clean_provider_test_env(|| {
        with_env_var(
            "JCODE_OPENAI_COMPAT_API_BASE",
            "https://compat.example.test/v1",
            || {
                with_env_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "OPENAI_API_KEY", || {
                    with_env_var("OPENAI_API_KEY", "test-compatible-key", || {
                        crate::provider_catalog::force_apply_openai_compatible_profile_env(Some(
                            crate::provider_catalog::OPENAI_COMPAT_PROFILE,
                        ));
                        let openrouter = Arc::new(
                            openrouter::OpenRouterProvider::new()
                                .expect("OpenAI-compatible provider should initialize"),
                        );
                        let provider = MultiProvider {
                            claude: RwLock::new(None),
                            anthropic: RwLock::new(None),
                            openai: RwLock::new(None),
                            copilot_api: RwLock::new(None),
                            antigravity: RwLock::new(None),
                            gemini: RwLock::new(None),
                            cursor: RwLock::new(None),
                            bedrock: RwLock::new(None),
                            openrouter: RwLock::new(Some(openrouter)),
                            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                            active_openai_compatible_profile: RwLock::new(None),
                            active: RwLock::new(ActiveProvider::OpenRouter),
                            use_claude_cli: false,
                            startup_notices: RwLock::new(Vec::new()),
                            forced_provider: None,
                        };

                        provider
                            .set_config_default_model("gpt-5.5", Some("openai-compatible"))
                            .expect(
                                "configured OpenAI-compatible default model should apply locally",
                            );

                        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
                        assert_eq!(provider.model(), "gpt-5.5");
                        assert_eq!(
                            crate::provider_catalog::runtime_provider_display_name(provider.name()),
                            "OpenAI-compatible"
                        );
                    })
                })
            },
        )
    });
}

#[test]
fn test_custom_compatible_model_routes_do_not_request_openrouter_rewrite() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            with_env_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0", || {
                with_env_var(
                    "JCODE_OPENROUTER_API_BASE",
                    "https://compat.example.test/v1",
                    || {
                        let openrouter = Arc::new(
                            openrouter::OpenRouterProvider::new()
                                .expect("custom compatible provider should initialize"),
                        );
                        let provider = MultiProvider {
                            claude: RwLock::new(None),
                            anthropic: RwLock::new(None),
                            openai: RwLock::new(None),
                            copilot_api: RwLock::new(None),
                            antigravity: RwLock::new(None),
                            gemini: RwLock::new(None),
                            cursor: RwLock::new(None),
                            bedrock: RwLock::new(None),
                            openrouter: RwLock::new(Some(openrouter)),
                            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                            active_openai_compatible_profile: RwLock::new(None),
                            active: RwLock::new(ActiveProvider::OpenRouter),
                            use_claude_cli: false,
                            startup_notices: RwLock::new(Vec::new()),
                            forced_provider: Some(ActiveProvider::OpenRouter),
                        };

                        provider.set_model("claude-opus4.6-thinking").expect(
                            "forced OpenAI-compatible provider should accept opaque model IDs",
                        );

                        let routes = provider.model_routes();
                        assert!(routes.iter().any(|route| {
                            route.model == "claude-opus4.6-thinking"
                                && route.provider == "OpenAI-compatible"
                                && route.api_method == "openai-compatible"
                        }));
                        assert!(!routes.iter().any(|route| {
                            route.model == "claude-opus4.6-thinking"
                                && route.provider == "auto"
                                && route.api_method == "openrouter"
                        }));
                    },
                )
            })
        })
    });
}

#[test]
fn test_configured_direct_compatible_profiles_are_listed_without_openrouter_key() {
    with_clean_provider_test_env(|| {
        with_env_var("DEEPSEEK_API_KEY", "test-deepseek-key", || {
            with_env_var("KIMI_API_KEY", "test-kimi-key", || {
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
                    active: RwLock::new(ActiveProvider::OpenAI),
                    use_claude_cli: false,
                    startup_notices: RwLock::new(Vec::new()),
                    forced_provider: None,
                };

                let routes = provider.model_routes();
                assert!(routes.iter().any(|route| {
                    route.model == "deepseek-v4-flash"
                        && route.provider == "DeepSeek"
                        && route.api_method == "openai-compatible:deepseek"
                        && route.available
                }));
                assert!(routes.iter().any(|route| {
                    route.model == "deepseek-v4-pro"
                        && route.provider == "DeepSeek"
                        && route.api_method == "openai-compatible:deepseek"
                        && route.available
                }));
                assert!(routes.iter().any(|route| {
                    route.model == "kimi-for-coding"
                        && route.provider == "Kimi Code"
                        && route.api_method == "openai-compatible:kimi"
                        && route.available
                }));
                assert!(
                    !routes
                        .iter()
                        .any(|route| route.model == "openrouter models")
                );
            })
        })
    });
}

#[test]
fn test_profile_prefixed_model_switch_reinitializes_direct_compatible_runtime() {
    with_clean_provider_test_env(|| {
        with_env_var("DEEPSEEK_API_KEY", "test-deepseek-key", || {
            with_env_var("KIMI_API_KEY", "test-kimi-key", || {
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
                    active: RwLock::new(ActiveProvider::OpenAI),
                    use_claude_cli: false,
                    startup_notices: RwLock::new(Vec::new()),
                    forced_provider: None,
                };

                provider
                    .set_model("deepseek:deepseek-v4-pro")
                    .expect("DeepSeek profile-prefixed model should initialize direct provider");
                assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
                assert_eq!(provider.model(), "deepseek-v4-pro");
                assert_eq!(
                    crate::provider_catalog::runtime_provider_display_name(provider.name()),
                    "DeepSeek"
                );

                provider
                    .set_model("kimi:kimi-for-coding")
                    .expect("Kimi profile-prefixed model should reinitialize direct provider");
                assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
                assert_eq!(provider.model(), "kimi-for-coding");
                assert_eq!(
                    crate::provider_catalog::runtime_provider_display_name(provider.name()),
                    "Kimi Code"
                );
            })
        })
    });
}

#[test]
fn test_openai_auth_mode_prefixed_model_switch_changes_credentials() {
    with_clean_provider_test_env(|| {
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_PROVIDER");
        crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
        crate::env::set_var("OPENAI_API_KEY", "sk-test-openai-api-key");
        crate::auth::codex::upsert_account_from_tokens(
            "openai-1",
            "oauth-access-token",
            "oauth-refresh-token",
            None,
            None,
        )
        .expect("save OAuth account");

        let openai = Arc::new(openai::OpenAIProvider::new(
            crate::auth::codex::load_credentials().expect("load OpenAI credentials"),
        ));
        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(Some(Arc::clone(&openai))),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: None,
        };
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();

        assert_eq!(
            rt.block_on(openai.test_access_token()),
            "oauth-access-token",
            "default OpenAI credentials should still prefer OAuth for backwards compatibility"
        );

        provider
            .set_model("openai-api:gpt-5.5")
            .expect("API-key route should select the OpenAI API credentials");
        assert_eq!(
            rt.block_on(openai.test_access_token()),
            "sk-test-openai-api-key"
        );

        provider
            .set_model("openai-oauth:gpt-5.5")
            .expect("OAuth route should switch back to Codex OAuth credentials");
        assert_eq!(
            rt.block_on(openai.test_access_token()),
            "oauth-access-token"
        );

        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_PROVIDER", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
        }
    });
}

#[test]
fn test_anthropic_auth_mode_prefixed_model_switch_changes_credentials() {
    with_clean_provider_test_env(|| {
        crate::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-api-key");
        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "oauth-access-token".to_string(),
            refresh: "oauth-refresh-token".to_string(),
            expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: None,
            subscription_type: Some("max".to_string()),
            scopes: vec!["user:inference".to_string()],
        })
        .expect("save Claude OAuth account");

        let anthropic = Arc::new(anthropic::AnthropicProvider::new());
        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(Some(Arc::clone(&anthropic))),
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
            forced_provider: None,
        };
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();

        assert_eq!(
            rt.block_on(anthropic.test_access_token_and_oauth_mode())
                .expect("default token"),
            ("oauth-access-token".to_string(), true),
            "default (Auto) Anthropic credentials prefer OAuth/subscription when an \
             OAuth account is available, matching the canonical OAuth-first Auto \
             behavior shared with the OpenAI provider and resolve_dual_credential_auth"
        );

        provider
            .set_model("claude-oauth:claude-opus-4-6")
            .expect("OAuth route should select Claude OAuth credentials");
        assert_eq!(
            rt.block_on(anthropic.test_access_token_and_oauth_mode())
                .expect("oauth token"),
            ("oauth-access-token".to_string(), true)
        );

        provider
            .set_model("claude-api:claude-opus-4-6")
            .expect("API route should select Anthropic API-key credentials");
        assert_eq!(
            rt.block_on(anthropic.test_access_token_and_oauth_mode())
                .expect("api token"),
            ("sk-ant-test-api-key".to_string(), false)
        );
    });
}

#[test]
fn test_multi_provider_fork_switch_request_preserves_route_identity_state_space() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        crate::env::set_var("OPENAI_API_KEY", "sk-test-openai-api-key");
        crate::auth::codex::upsert_account_from_tokens(
            "openai-1",
            "oauth-access-token",
            "oauth-refresh-token",
            None,
            None,
        )
        .expect("save OpenAI OAuth account");
        let openai = Arc::new(openai::OpenAIProvider::new(
            crate::auth::codex::load_credentials().expect("load OpenAI credentials"),
        ));
        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(Some(openai)),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: None,
        };

        provider
            .set_model("openai-api:gpt-5.5")
            .expect("API-key route should be selectable");
        assert_eq!(
            provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
            "openai-api:gpt-5.5"
        );
        provider
            .set_model("openai-oauth:gpt-5.5")
            .expect("OAuth route should be selectable");
        assert_eq!(
            provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
            "openai-oauth:gpt-5.5"
        );
    });

    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        crate::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-api-key");
        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "oauth-access-token".to_string(),
            refresh: "oauth-refresh-token".to_string(),
            expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: None,
            subscription_type: Some("max".to_string()),
            scopes: vec!["user:inference".to_string()],
        })
        .expect("save Claude OAuth account");
        let anthropic = Arc::new(anthropic::AnthropicProvider::new());
        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(Some(anthropic)),
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
            forced_provider: None,
        };

        provider
            .set_model("claude-oauth:claude-opus-4-6")
            .expect("OAuth route should be selectable");
        assert_eq!(
            provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
            "claude-oauth:claude-opus-4-6"
        );
        provider
            .set_model("claude-api:claude-opus-4-6")
            .expect("API-key route should be selectable");
        assert_eq!(
            provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
            "claude-api:claude-opus-4-6"
        );
    });

    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        crate::env::set_var("CEREBRAS_API_KEY", "test-cerebras-key");
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
            active: RwLock::new(ActiveProvider::OpenAI),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: None,
        };
        provider
            .set_model("cerebras:qwen-3-235b-a22b-instruct-2507")
            .expect("profile-prefixed Cerebras route should be selectable");
        assert_eq!(
            provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
            "cerebras:qwen-3-235b-a22b-instruct-2507"
        );
    });

    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            let openrouter = Arc::new(
                openrouter::OpenRouterProvider::new()
                    .expect("openrouter provider should initialize"),
            );
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(None),
                bedrock: RwLock::new(None),
                openrouter: RwLock::new(Some(openrouter)),
                openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                active_openai_compatible_profile: RwLock::new(None),
                active: RwLock::new(ActiveProvider::OpenRouter),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: None,
            };

            provider
                .set_model("openrouter:openai/gpt-5.4@OpenAI")
                .expect("OpenRouter provider-pinned route should be selectable");
            assert_eq!(
                provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
                "openrouter:openai/gpt-5.4@OpenAI"
            );
        })
    });
}

#[test]
fn test_deepseek_direct_profile_supports_reasoning_effort_via_multi_provider() {
    with_clean_provider_test_env(|| {
        with_env_var("DEEPSEEK_API_KEY", "test-deepseek-key", || {
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
                active: RwLock::new(ActiveProvider::OpenAI),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: None,
            };

            provider
                .set_model("deepseek:deepseek-v4-pro")
                .expect("DeepSeek profile-prefixed model should initialize direct provider");

            assert_eq!(
                provider.available_efforts(),
                vec!["none", "low", "medium", "high", "max"]
            );
            provider
                .set_reasoning_effort("max")
                .expect("/effort max should work for direct DeepSeek profile");
            assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));
        })
    });
}

#[test]
fn test_forced_copilot_treats_claude_like_model_as_provider_local() {
    with_clean_provider_test_env(|| {
        let copilot = Arc::new(copilot::CopilotApiProvider::new_with_token(
            "test-token".to_string(),
        ));
        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(Some(copilot)),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Copilot),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::Copilot),
        };

        provider
            .set_model("claude-opus-4.6")
            .expect("forced Copilot should accept Copilot's dotted Claude model ID");

        assert_eq!(provider.active_provider(), ActiveProvider::Copilot);
        assert_eq!(provider.model(), "claude-opus-4.6");
    });
}

#[test]
fn test_provider_specific_model_prefix_cannot_bypass_provider_lock() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            let openrouter = Arc::new(
                openrouter::OpenRouterProvider::new()
                    .expect("openrouter provider should initialize"),
            );
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(Some(Arc::new(cursor::CursorCliProvider::new()))),
                bedrock: RwLock::new(None),
                openrouter: RwLock::new(Some(openrouter)),
                openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                active_openai_compatible_profile: RwLock::new(None),
                active: RwLock::new(ActiveProvider::OpenRouter),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: Some(ActiveProvider::OpenRouter),
            };

            let err = provider
                .set_model("cursor:gpt-5")
                .expect_err("explicit cursor prefix should not bypass an OpenRouter lock");

            assert!(
                err.to_string().contains("--provider is locked"),
                "expected provider lock error, got: {}",
                err
            );
            assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
        })
    });
}

#[test]
fn test_provider_for_model_unknown() {
    assert_eq!(provider_for_model("unknown-model"), None);
}

#[test]
fn test_provider_for_model_cursor() {
    assert_eq!(provider_for_model("composer-2-fast"), Some("cursor"));
    assert_eq!(provider_for_model("composer-2"), Some("cursor"));
    assert_eq!(provider_for_model("sonnet-4.6"), Some("cursor"));
    assert_eq!(provider_for_model("gpt-5"), Some("openai"));
}

#[test]
fn test_context_limit_spark_vs_codex() {
    assert_eq!(
        context_limit_for_model("gpt-5.3-codex-spark"),
        Some(128_000)
    );
    assert_eq!(context_limit_for_model("gpt-5.5"), Some(272_000));
    assert_eq!(context_limit_for_model("gpt-5.3-codex"), Some(272_000));
    assert_eq!(context_limit_for_model("gpt-5.2-codex"), Some(272_000));
    assert_eq!(context_limit_for_model("gpt-5-codex"), Some(272_000));
}

#[test]
fn test_context_limit_gpt_5_4() {
    assert_eq!(context_limit_for_model("gpt-5.4"), Some(1_000_000));
    assert_eq!(context_limit_for_model("gpt-5.4-pro"), Some(1_000_000));
    assert_eq!(context_limit_for_model("gpt-5.4[1m]"), Some(1_000_000));
}

#[test]
fn test_context_limit_respects_provider_hint() {
    assert_eq!(
        context_limit_for_model_with_provider("gpt-5.4", Some("openai")),
        Some(1_000_000)
    );
    assert_eq!(
        context_limit_for_model_with_provider("gpt-5.4", Some("copilot")),
        Some(128_000)
    );
    assert_eq!(
        context_limit_for_model_with_provider("claude-sonnet-4-6[1m]", Some("claude")),
        Some(1_048_576)
    );
}

#[test]
fn test_resolve_model_capabilities_uses_provider_hint() {
    let openai = resolve_model_capabilities("gpt-5.4", Some("openai"));
    assert_eq!(openai.provider.as_deref(), Some("openai"));
    assert_eq!(openai.context_window, Some(1_000_000));

    let copilot = resolve_model_capabilities("gpt-5.4", Some("copilot"));
    assert_eq!(copilot.provider.as_deref(), Some("copilot"));
    assert_eq!(copilot.context_window, Some(128_000));

    let gemini = resolve_model_capabilities("gemini-2.5-pro", Some("gemini"));
    assert_eq!(gemini.provider.as_deref(), Some("gemini"));
    assert_eq!(gemini.context_window, Some(1_000_000));
}

#[test]
fn test_normalize_model_id_strips_1m_suffix() {
    assert_eq!(models::normalize_model_id("gpt-5.4[1m]"), "gpt-5.4");
    assert_eq!(models::normalize_model_id(" GPT-5.4[1M] "), "gpt-5.4");
}

#[test]
fn test_merge_openai_model_ids_appends_dynamic_oauth_models() {
    let models = models::merge_openai_model_ids(vec![
        "gpt-5.4".to_string(),
        "gpt-5.4-fast-preview".to_string(),
        "gpt-5.4-fast-preview".to_string(),
        " gpt-5.5-experimental ".to_string(),
    ]);

    assert!(models.iter().any(|model| model == "gpt-5.4"));
    assert!(models.iter().any(|model| model == "gpt-5.4-fast-preview"));
    assert!(models.iter().any(|model| model == "gpt-5.5-experimental"));
    assert_eq!(
        models
            .iter()
            .filter(|model| model.as_str() == "gpt-5.4-fast-preview")
            .count(),
        1
    );
}

#[test]
fn test_merge_anthropic_model_ids_appends_dynamic_models() {
    let models = models::merge_anthropic_model_ids(vec![
        "claude-opus-4-6".to_string(),
        "claude-sonnet-5-preview".to_string(),
        "claude-sonnet-5-preview".to_string(),
        " claude-haiku-5-beta ".to_string(),
    ]);

    assert!(models.iter().any(|model| model == "claude-opus-4-6"));
    assert!(models.iter().any(|model| model == "claude-opus-4-6[1m]"));
    assert!(
        models
            .iter()
            .any(|model| model == "claude-sonnet-5-preview")
    );
    assert!(models.iter().any(|model| model == "claude-haiku-5-beta"));
    assert_eq!(
        models
            .iter()
            .filter(|model| model.as_str() == "claude-sonnet-5-preview")
            .count(),
        1
    );
}

#[test]
fn test_parse_anthropic_model_catalog_reads_context_limits() {
    let data = serde_json::json!({
        "data": [
            {
                "id": "claude-opus-4-6",
                "max_input_tokens": 1_048_576
            },
            {
                "id": "claude-sonnet-5-preview",
                "max_input_tokens": 333_000
            }
        ]
    });

    let catalog = models::parse_anthropic_model_catalog(&data);
    assert!(
        catalog
            .available_models
            .contains(&"claude-opus-4-6".to_string())
    );
    assert!(
        catalog
            .available_models
            .contains(&"claude-sonnet-5-preview".to_string())
    );
    assert_eq!(
        catalog.context_limits.get("claude-opus-4-6"),
        Some(&1_048_576)
    );
    assert_eq!(
        catalog.context_limits.get("claude-sonnet-5-preview"),
        Some(&333_000)
    );
}

#[test]
fn test_context_limit_claude() {
    with_clean_provider_test_env(|| {
        assert_eq!(context_limit_for_model("claude-opus-4-6"), Some(200_000));
        assert_eq!(context_limit_for_model("claude-sonnet-4-6"), Some(200_000));
        assert_eq!(
            context_limit_for_model("claude-opus-4-6[1m]"),
            Some(1_048_576)
        );
        assert_eq!(
            context_limit_for_model("claude-sonnet-4-6[1m]"),
            Some(1_048_576)
        );
        // Opus 4.8 / 4.7 expose a 1M window natively (no `[1m]` opt-in needed),
        // matching the live Anthropic catalog's `max_input_tokens: 1000000`.
        assert_eq!(context_limit_for_model("claude-opus-4-8"), Some(1_000_000));
        assert_eq!(
            context_limit_for_model("claude-opus-4-8[1m]"),
            Some(1_000_000)
        );
        assert_eq!(context_limit_for_model("claude-opus-4-7"), Some(1_000_000));
    });
}

#[test]
fn test_context_limit_dynamic_cache() {
    populate_context_limits(
        [("test-model-xyz".to_string(), 64_000)]
            .into_iter()
            .collect(),
    );
    assert_eq!(context_limit_for_model("test-model-xyz"), Some(64_000));
}
