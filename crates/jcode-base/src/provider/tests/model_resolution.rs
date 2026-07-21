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
        routes_memo: std::sync::Mutex::new(None),
        post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
            let openrouter =
                test_openrouter_runtime().expect("Cerebras direct provider should initialize");
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
                routes_memo: std::sync::Mutex::new(None),
                post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            };

            let routes = provider.model_routes();
            // Assert against the profile's current static model list so this
            // test tracks catalog updates instead of hardcoding a model that
            // Cerebras may stop serving (the original fixture pinned
            // `qwen-3-235b-a22b-instruct-2507`, which rotted when the static
            // coverage was refreshed).
            let static_models = crate::provider_catalog::openai_compatible_profile_static_models(
                crate::provider_catalog::CEREBRAS_PROFILE,
            );
            let probe_model = static_models
                .first()
                .expect("Cerebras profile should have static models")
                .clone();
            let probe_routes = routes
                .iter()
                .filter(|route| route.provider == "Cerebras" && route.model == probe_model)
                .collect::<Vec<_>>();
            assert_eq!(
                probe_routes.len(),
                1,
                "Cerebras direct route should not appear twice in provider routes: {routes:?}"
            );
            assert_eq!(probe_routes[0].api_method, "openai-compatible:cerebras");
            assert!(probe_routes[0].available);
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
                let openrouter = test_openrouter_runtime()
                    .expect("autodetected Chutes provider should initialize");
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
                    routes_memo: std::sync::Mutex::new(None),
                    post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        let openrouter = test_openrouter_runtime().expect("Cerebras provider should initialize");
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        let openrouter = test_openrouter_runtime().expect("Cerebras provider should initialize");

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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        let openrouter = test_openrouter_runtime().expect("OpenRouter provider should initialize");
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        let openrouter = test_openrouter_runtime().expect("OpenRouter provider should initialize");
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        provider
            .set_model("nvidia-nim:nvidia/llama-3.1-nemotron-ultra-253b-v1")
            .expect("NVIDIA NIM model should be selectable");
        assert_eq!(provider.model(), "nvidia/llama-3.1-nemotron-ultra-253b-v1");
        assert!(Arc::ptr_eq(
            &provider
                .openrouter_provider()
                .expect("real OpenRouter remains"),
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
            let openrouter =
                test_openrouter_runtime().expect("openrouter provider should initialize");
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
                routes_memo: std::sync::Mutex::new(None),
                post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
                        let openrouter = test_openrouter_runtime()
                            .expect("custom compatible provider should initialize");
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
                            openai_compatible_profiles: RwLock::new(
                                std::collections::HashMap::new(),
                            ),
                            active_openai_compatible_profile: RwLock::new(None),
                            active: RwLock::new(ActiveProvider::OpenRouter),
                            use_claude_cli: false,
                            startup_notices: RwLock::new(Vec::new()),
                            forced_provider: Some(ActiveProvider::OpenRouter),
                            routes_memo: std::sync::Mutex::new(None),
                            post_auth_refreshes_pending: Arc::new(
                                std::sync::atomic::AtomicUsize::new(0),
                            ),
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
                        let openrouter = test_openrouter_runtime()
                            .expect("custom compatible provider should initialize");
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
                            openai_compatible_profiles: RwLock::new(
                                std::collections::HashMap::new(),
                            ),
                            active_openai_compatible_profile: RwLock::new(None),
                            active: RwLock::new(ActiveProvider::OpenRouter),
                            use_claude_cli: false,
                            startup_notices: RwLock::new(Vec::new()),
                            forced_provider: Some(ActiveProvider::OpenRouter),
                            routes_memo: std::sync::Mutex::new(None),
                            post_auth_refreshes_pending: Arc::new(
                                std::sync::atomic::AtomicUsize::new(0),
                            ),
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
                        let openrouter = test_openrouter_runtime()
                            .expect("OpenAI-compatible provider should initialize");
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
                            openai_compatible_profiles: RwLock::new(
                                std::collections::HashMap::new(),
                            ),
                            active_openai_compatible_profile: RwLock::new(None),
                            active: RwLock::new(ActiveProvider::OpenRouter),
                            use_claude_cli: false,
                            startup_notices: RwLock::new(Vec::new()),
                            forced_provider: None,
                            routes_memo: std::sync::Mutex::new(None),
                            post_auth_refreshes_pending: Arc::new(
                                std::sync::atomic::AtomicUsize::new(0),
                            ),
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
                        let openrouter = test_openrouter_runtime()
                            .expect("custom compatible provider should initialize");
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
                            openai_compatible_profiles: RwLock::new(
                                std::collections::HashMap::new(),
                            ),
                            active_openai_compatible_profile: RwLock::new(None),
                            active: RwLock::new(ActiveProvider::OpenRouter),
                            use_claude_cli: false,
                            startup_notices: RwLock::new(Vec::new()),
                            forced_provider: Some(ActiveProvider::OpenRouter),
                            routes_memo: std::sync::Mutex::new(None),
                            post_auth_refreshes_pending: Arc::new(
                                std::sync::atomic::AtomicUsize::new(0),
                            ),
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
                    routes_memo: std::sync::Mutex::new(None),
                    post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
fn test_named_config_provider_models_appear_in_picker_and_are_selectable() {
    // Issue #444: models declared under `[[providers.<name>.models]]` in
    // config.toml must appear in the model picker with a route back to that
    // profile, and selecting the emitted `<name>:<model>` spec must bind the
    // named profile runtime.
    with_clean_provider_test_env(|| {
        let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
        std::fs::write(
            std::path::PathBuf::from(jcode_home).join("config.toml"),
            r#"
[provider]
default_provider = "my-gateway"
default_model = "vendor/my-model"

[providers.my-gateway]
type = "openai-compatible"
base_url = "https://example.com/proxy/openai"
auth = "none"
default_model = "vendor/my-model"

[[providers.my-gateway.models]]
id = "vendor/my-model"
context_window = 230000
input = ["text"]

[[providers.my-gateway.models]]
id = "vendor/image-only-model"
input = ["image"]
"#,
        )
        .expect("write test config.toml");
        crate::config::invalidate_config_cache();

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
            forced_provider: None,
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        // The picker must offer the text-capable configured model with a
        // route back to the named profile, and exclude image-only models.
        let routes = provider.model_routes();
        let route = routes
            .iter()
            .find(|route| route.model == "vendor/my-model")
            .unwrap_or_else(|| {
                panic!("configured named-profile model missing from picker: {routes:?}")
            });
        assert_eq!(route.provider, "my-gateway");
        assert_eq!(route.api_method, "openai-compatible:my-gateway");
        assert!(route.available);
        assert!(
            !routes
                .iter()
                .any(|route| route.model == "vendor/image-only-model"),
            "image-only configured models must not be listed"
        );

        // Selecting the picker's spec must bind the named profile runtime.
        provider
            .set_model("my-gateway:vendor/my-model")
            .expect("named profile model spec must be selectable");
        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
        assert_eq!(provider.model(), "vendor/my-model");

        // And the configured default_provider/default_model pair must bind the
        // profile directly (same bug class as issue #448).
        let provider2 = MultiProvider {
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
            forced_provider: None,
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        provider2
            .set_config_default_model("vendor/my-model", Some("my-gateway"))
            .expect("configured named-profile default must bind the profile runtime");
        assert_eq!(provider2.active_provider(), ActiveProvider::OpenRouter);
        assert_eq!(provider2.model(), "vendor/my-model");
    });
}

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
            forced_provider: None,
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

#[test]
fn test_config_default_provider_deepseek_applies_without_openrouter_key() {
    // Issue #448: `default_provider = "deepseek"` + `default_model =
    // "deepseek-v4-pro"` with only DEEPSEEK_API_KEY set must bind the DeepSeek
    // profile runtime. The generic OpenRouter path would try to rebind the
    // slot to a plain OpenRouter API-key runtime, fail (no OPENROUTER_API_KEY),
    // and silently fall back to the auto-detected default provider.
    with_clean_provider_test_env(|| {
        with_env_var("DEEPSEEK_API_KEY", "test-deepseek-key", || {
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(Some(test_anthropic_runtime())),
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
                routes_memo: std::sync::Mutex::new(None),
                post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            };

            provider
                .set_config_default_model("deepseek-v4-pro", Some("deepseek"))
                .expect("configured DeepSeek default must bind the profile runtime");
            assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
            assert_eq!(provider.model(), "deepseek-v4-pro");
            assert_eq!(provider.display_name(), "DeepSeek");
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
                    routes_memo: std::sync::Mutex::new(None),
                    post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                };

                provider
                    .set_model("deepseek:deepseek-v4-pro")
                    .expect("DeepSeek profile-prefixed model should initialize direct provider");
                assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
                assert_eq!(provider.model(), "deepseek-v4-pro");
                // `display_name` resolves through the active execution runtime
                // (registry), which is the production display path since the
                // compat-profile/OpenRouter slot split.
                assert_eq!(provider.display_name(), "DeepSeek");

                provider
                    .set_model("kimi:kimi-for-coding")
                    .expect("Kimi profile-prefixed model should reinitialize direct provider");
                assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
                assert_eq!(provider.model(), "kimi-for-coding");
                assert_eq!(provider.display_name(), "Kimi Code");
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

        let openai = test_openai_runtime();
        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(Some(Arc::clone(&openai) as Arc<dyn Provider>)),
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();

        // Route pinning is MultiProvider's job; per-pin token resolution is
        // covered by jcode-provider-openai-runtime's tests.
        assert_eq!(
            openai.credential_mode(),
            jcode_provider_core::CredentialMode::Auto,
            "default OpenAI credentials stay on the OAuth-first Auto pin"
        );

        provider
            .set_model("openai-api:gpt-5.5")
            .expect("API-key route should select the OpenAI API credentials");
        assert_eq!(
            openai.credential_mode(),
            jcode_provider_core::CredentialMode::ApiKey
        );

        provider
            .set_model("openai-oauth:gpt-5.5")
            .expect("OAuth route should switch back to Codex OAuth credentials");
        assert_eq!(
            openai.credential_mode(),
            jcode_provider_core::CredentialMode::OAuth
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

        let anthropic = test_anthropic_runtime();
        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(Some(Arc::clone(&anthropic) as Arc<dyn Provider>)),
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();

        // Route pinning is MultiProvider's job; the concrete token resolution
        // for each pin is covered by jcode-provider-anthropic-runtime's
        // credential-mode tests.
        assert_eq!(
            anthropic.credential_mode(),
            jcode_provider_core::CredentialMode::Auto,
            "default (Auto) leaves the credential pin unset"
        );

        provider
            .set_model("claude-oauth:claude-opus-4-6")
            .expect("OAuth route should select Claude OAuth credentials");
        assert_eq!(
            anthropic.credential_mode(),
            jcode_provider_core::CredentialMode::OAuth
        );

        provider
            .set_model("claude-api:claude-opus-4-6")
            .expect("API route should select Anthropic API-key credentials");
        assert_eq!(
            anthropic.credential_mode(),
            jcode_provider_core::CredentialMode::ApiKey
        );
    });
}

#[test]
fn test_config_default_provider_anthropic_api_pins_api_credential() {
    use jcode_provider_core::{Provider, ResolvedCredential};
    // A config `default_provider = "anthropic-api"` is a routing decision that
    // also pins the OAuth-vs-API credential. Applying the default at startup
    // must leave the provider on the API-key route so the header auth tag and
    // model picker report "API Key", not the Auto/OAuth fallback.
    for (default_provider, expected, expect_oauth) in [
        ("anthropic-api", ResolvedCredential::ApiKey, false),
        ("claude-api", ResolvedCredential::ApiKey, false),
        ("claude", ResolvedCredential::Oauth, true),
        ("anthropic", ResolvedCredential::Oauth, true),
    ] {
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

            let anthropic = test_anthropic_runtime();
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(Some(Arc::clone(&anthropic) as Arc<dyn Provider>)),
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
                routes_memo: std::sync::Mutex::new(None),
                post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            };
            let rt = enter_test_runtime();
            let _runtime_guard = rt.enter();

            provider
                .set_config_default_model("claude-opus-4-6", Some(default_provider))
                .unwrap_or_else(|e| {
                    panic!("default_provider '{default_provider}' should apply: {e}")
                });

            assert_eq!(
                provider.active_provider(),
                ActiveProvider::Claude,
                "default_provider '{default_provider}' routes to Claude",
            );
            assert_eq!(
                provider.active_explicit_credential(),
                (!expect_oauth).then_some(ResolvedCredential::ApiKey),
                "default_provider '{default_provider}' explicit-pin visibility",
            );
            assert_eq!(
                anthropic.credential_mode(),
                if expect_oauth {
                    // "claude"/"anthropic" leave Auto (OAuth-first) rather than
                    // pinning OAuth explicitly.
                    jcode_provider_core::CredentialMode::Auto
                } else {
                    jcode_provider_core::CredentialMode::ApiKey
                },
                "default_provider '{default_provider}' should resolve {expected:?}",
            );
        });
    }
}

#[test]
fn test_config_default_model_with_credential_prefix_applies_model_and_pin() {
    use jcode_provider_core::{Provider, ResolvedCredential};
    // The model picker saves default_model as a full spec like
    // `claude-api:claude-opus-4-6`. Startup must parse the prefix (routing +
    // credential pin) instead of handing the raw spec to the Anthropic
    // provider, which would reject it and silently keep the fallback default.
    for (spec, expect_oauth) in [
        ("claude-api:claude-opus-4-6", false),
        ("claude-oauth:claude-opus-4-6", true),
        ("claude:claude-opus-4-6", true),
    ] {
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

            let anthropic = test_anthropic_runtime();
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(Some(Arc::clone(&anthropic) as Arc<dyn Provider>)),
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
                routes_memo: std::sync::Mutex::new(None),
                post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            };
            let rt = enter_test_runtime();
            let _runtime_guard = rt.enter();

            provider
                .set_config_default_model(spec, Some("anthropic-api"))
                .unwrap_or_else(|e| panic!("default_model '{spec}' should apply: {e}"));

            assert_eq!(
                provider.active_provider(),
                ActiveProvider::Claude,
                "default_model '{spec}' routes to Claude",
            );
            assert_eq!(
                provider.model(),
                "claude-opus-4-6",
                "default_model '{spec}' should set the bare model id",
            );
            // `claude-api:` must pin the API key; `claude:`/`claude-oauth:`
            // resolve OAuth-first (Auto or explicit OAuth respectively), so the
            // pin must not be ApiKey. Concrete token resolution per pin is
            // covered by jcode-provider-anthropic-runtime's tests.
            if expect_oauth {
                assert_ne!(
                    anthropic.credential_mode(),
                    jcode_provider_core::CredentialMode::ApiKey,
                    "default_model '{spec}' must not pin the API key (expected {:?})",
                    ResolvedCredential::Oauth,
                );
            } else {
                assert_eq!(
                    anthropic.credential_mode(),
                    jcode_provider_core::CredentialMode::ApiKey,
                    "default_model '{spec}' should resolve {:?}",
                    ResolvedCredential::ApiKey,
                );
            }
        });
    }
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
        let openai = test_openai_runtime();
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        let generation_before = crate::provider::pricing::auth_pricing_generation();
        provider
            .set_model("openai-api:gpt-5.5")
            .expect("API-key route should be selectable");
        let api_route_memo_key = provider.routes_memo_key();
        assert_eq!(
            crate::provider::pricing::auth_pricing_generation(),
            generation_before,
            "restoring an in-memory credential route must not invalidate global catalogs"
        );
        assert_eq!(
            provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
            "openai-api:gpt-5.5"
        );
        let _fork = provider.fork();
        assert_eq!(
            crate::provider::pricing::auth_pricing_generation(),
            generation_before,
            "forking a provider must not invalidate global catalogs"
        );
        provider
            .set_model("openai-oauth:gpt-5.5")
            .expect("OAuth route should be selectable");
        assert_ne!(
            provider.routes_memo_key(),
            api_route_memo_key,
            "OAuth and API-key catalogs need distinct shared memo keys"
        );
        assert_eq!(
            crate::provider::pricing::auth_pricing_generation(),
            generation_before
        );
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
        let anthropic = test_anthropic_runtime();
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
            let openrouter =
                test_openrouter_runtime().expect("openrouter provider should initialize");
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
                routes_memo: std::sync::Mutex::new(None),
                post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
                routes_memo: std::sync::Mutex::new(None),
                post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            };

            provider
                .set_model("deepseek:deepseek-v4-pro")
                .expect("DeepSeek profile-prefixed model should initialize direct provider");

            assert_eq!(
                provider.available_efforts(),
                vec![
                    "none",
                    "low",
                    "medium",
                    "high",
                    "max",
                    "swarm",
                    "swarm-deep"
                ]
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
        let copilot = test_copilot_runtime();
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
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
            let openrouter =
                test_openrouter_runtime().expect("openrouter provider should initialize");
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(Some(test_cursor_runtime())),
                bedrock: RwLock::new(None),
                openrouter: RwLock::new(Some(openrouter)),
                openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
                active_openai_compatible_profile: RwLock::new(None),
                active: RwLock::new(ActiveProvider::OpenRouter),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: Some(ActiveProvider::OpenRouter),
                routes_memo: std::sync::Mutex::new(None),
                post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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

// --- Migrated from the OpenRouter runtime tests: these exercise MultiProvider
// --- routing/identity with a real OpenRouter runtime via the registry.

struct OrEnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl OrEnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        crate::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for OrEnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

fn isolate_openrouter_autodetect_env_or() -> Vec<OrEnvVarGuard> {
    let mut guards = vec![
        OrEnvVarGuard::remove("JCODE_OPENROUTER_API_BASE"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_API_KEY_NAME"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_ENV_FILE"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_MODEL"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_ALLOW_NO_AUTH"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_TRANSPORT_STATE"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_PROVIDER_FEATURES"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_MODEL_CATALOG"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_AUTH_HEADER"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_AUTH_HEADER_NAME"),
        OrEnvVarGuard::remove("JCODE_OPENROUTER_STATIC_MODELS"),
        OrEnvVarGuard::remove("JCODE_ACTIVE_PROVIDER"),
        OrEnvVarGuard::remove("JCODE_RUNTIME_PROVIDER"),
        OrEnvVarGuard::remove("JCODE_NAMED_PROVIDER_PROFILE"),
        OrEnvVarGuard::remove("JCODE_PROVIDER_PROFILE_NAME"),
        OrEnvVarGuard::remove("JCODE_PROVIDER_PROFILE_ACTIVE"),
        OrEnvVarGuard::remove("JCODE_OPENAI_COMPAT_API_BASE"),
        OrEnvVarGuard::remove("JCODE_OPENAI_COMPAT_API_KEY_NAME"),
        OrEnvVarGuard::remove("JCODE_OPENAI_COMPAT_ENV_FILE"),
        OrEnvVarGuard::remove("JCODE_OPENAI_COMPAT_SETUP_URL"),
        OrEnvVarGuard::remove("JCODE_OPENAI_COMPAT_DEFAULT_MODEL"),
        OrEnvVarGuard::remove("JCODE_OPENAI_COMPAT_LOCAL_ENABLED"),
        OrEnvVarGuard::remove("OPENROUTER_API_KEY"),
    ];
    guards.extend(
        crate::provider_catalog::openai_compatible_profiles()
            .iter()
            .map(|profile| OrEnvVarGuard::remove(profile.api_key_env)),
    );
    guards
}

fn spawn_single_response_chat_server_or() -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake provider server");
    let addr = listener.local_addr().expect("fake provider addr");
    let (request_tx, request_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fake provider request");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = vec![0u8; 16384];
        let n = stream.read(&mut request).unwrap_or(0);
        let request = String::from_utf8_lossy(&request[..n]).into_owned();
        let _ = request_tx.send(request);

        let body = "data: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write fake provider response");
    });

    (format!("http://{addr}/v1"), request_rx)
}

#[test]
fn default_named_openai_compatible_provider_uses_direct_compatible_request_path() {
    let _lock = crate::storage::lock_test_env();
    // These tests construct MultiProvider directly (not via
    // with_clean_provider_test_env), so make sure the external runtime
    // factories are registered before startup paths need them.
    register_test_external_runtimes();
    let temp = tempfile::TempDir::new().expect("create temp home");
    let jcode_home = temp.path().join("jcode-home");
    let _jcode_home = OrEnvVarGuard::set("JCODE_HOME", &jcode_home);
    let _home = OrEnvVarGuard::set("HOME", temp.path());
    let _appdata = OrEnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env_or();
    let _key = OrEnvVarGuard::set("TEST_DEFAULT_COMPAT_KEY", "test-key");
    let (api_base, request_rx) = spawn_single_response_chat_server_or();

    std::fs::create_dir_all(&jcode_home).expect("create test config dir");
    std::fs::write(
        jcode_home.join("config.toml"),
        format!(
            r#"
[provider]
default_provider = "my-gateway"

[providers.my-gateway]
type = "openai-compatible"
base_url = "{api_base}"
api_key_env = "TEST_DEFAULT_COMPAT_KEY"
default_model = "opaque/model@id"
model_catalog = false
"#
        ),
    )
    .expect("write test config");
    crate::config::invalidate_config_cache();

    let provider = MultiProvider::new_with_auth_status(crate::auth::AuthStatus::default());
    assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
    let openrouter = provider
        .openrouter_provider()
        .expect("openrouter execution slot");
    assert!(
        !openrouter.supports_provider_routing_features(),
        "named openai-compatible defaults must not use OpenRouter provider routing features"
    );
    assert_eq!(
        openrouter
            .direct_openai_compatible_route_parts()
            .as_ref()
            .map(|parts| parts.1.as_str()),
        Some("openai-compatible:my-gateway")
    );

    let messages = vec![crate::message::Message {
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = openrouter
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "unexpected chat request: {request}"
    );
    assert!(
        request.contains(r#""model":"opaque/model@id""#),
        "request should use named profile default model: {request}"
    );
    assert!(
        !request.contains(r#""provider":"#),
        "direct OpenAI-compatible request must not include OpenRouter provider routing object: {request}"
    );
    assert!(
        !request.contains("HTTP-Referer") && !request.contains("X-Title"),
        "direct OpenAI-compatible request must not include OpenRouter-only headers: {request}"
    );
}

/// Regression for issue #304: a `default_provider` pointing at an
/// `openai-compatible` profile must build requests with the direct
/// OpenAI-compatible request shape, NOT the OpenRouter request builder, even
/// when `model_catalog` is left enabled (the default). Using the OpenRouter
/// builder leaks the `provider` routing object / OpenRouter-only headers and
/// causes strict third-party gateways to reject the request with
/// 400 "Unrecognized chat message".
#[test]
fn default_named_openai_compatible_with_catalog_uses_direct_compatible_request_path() {
    let _lock = crate::storage::lock_test_env();
    // These tests construct MultiProvider directly (not via
    // with_clean_provider_test_env), so make sure the external runtime
    // factories are registered before startup paths need them.
    register_test_external_runtimes();
    let temp = tempfile::TempDir::new().expect("create temp home");
    let jcode_home = temp.path().join("jcode-home");
    let _jcode_home = OrEnvVarGuard::set("JCODE_HOME", &jcode_home);
    let _home = OrEnvVarGuard::set("HOME", temp.path());
    let _appdata = OrEnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env_or();
    let _key = OrEnvVarGuard::set("TEST_DEFAULT_COMPAT_KEY", "test-key");
    let (api_base, request_rx) = spawn_single_response_chat_server_or();

    std::fs::create_dir_all(&jcode_home).expect("create test config dir");
    std::fs::write(
        jcode_home.join("config.toml"),
        format!(
            r#"
[provider]
default_provider = "my-gateway"

[providers.my-gateway]
type = "openai-compatible"
base_url = "{api_base}"
api_key_env = "TEST_DEFAULT_COMPAT_KEY"
default_model = "opaque/model@id"
"#
        ),
    )
    .expect("write test config");
    crate::config::invalidate_config_cache();

    let provider = MultiProvider::new_with_auth_status(crate::auth::AuthStatus::default());
    assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
    let openrouter = provider
        .openrouter_provider()
        .expect("openrouter execution slot");
    assert!(
        !openrouter.supports_provider_routing_features(),
        "named openai-compatible defaults must not use OpenRouter provider routing features even with catalog enabled"
    );

    let messages = vec![crate::message::Message {
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = openrouter
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "unexpected chat request: {request}"
    );
    assert!(
        !request.contains(r#""provider":"#),
        "direct OpenAI-compatible request must not include OpenRouter provider routing object: {request}"
    );
    assert!(
        !request.contains("HTTP-Referer") && !request.contains("X-Title"),
        "direct OpenAI-compatible request must not include OpenRouter-only headers: {request}"
    );
}

#[test]
fn runtime_display_name_tracks_active_openai_compatible_profile() {
    // Regression for issue #329: switching to a direct OpenAI-compatible
    // profile (NVIDIA NIM) at runtime must surface that profile's display
    // name, not the fixed "OpenRouter" aggregator label. The machine-facing
    // `name()` stays "openrouter" because billing/routing logic keys off it.
    let _lock = crate::storage::lock_test_env();
    // These tests construct MultiProvider directly (not via
    // with_clean_provider_test_env), so make sure the external runtime
    // factories are registered before startup paths need them.
    register_test_external_runtimes();
    let temp = tempfile::TempDir::new().expect("create temp home");
    let jcode_home = temp.path().join("jcode-home");
    let _jcode_home = OrEnvVarGuard::set("JCODE_HOME", &jcode_home);
    let _home = OrEnvVarGuard::set("HOME", temp.path());
    let _appdata = OrEnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _env = isolate_openrouter_autodetect_env_or();

    // Configure both the OpenRouter aggregator and NVIDIA NIM credentials so
    // the slot can host either runtime. Set after the isolate guard, which
    // clears every profile api-key env var.
    let _or_key = OrEnvVarGuard::set("OPENROUTER_API_KEY", "or-test-key");
    let _nim_key = OrEnvVarGuard::set("NVIDIA_API_KEY", "nim-test-key");
    crate::config::invalidate_config_cache();

    let provider = MultiProvider::new_with_auth_status(crate::auth::AuthStatus::default());

    // Switch to a NVIDIA NIM model via the profile-prefixed model request.
    provider
        .set_model("nvidia-nim:nvidia/llama-3.1-nemotron-ultra-253b-v1")
        .expect("switch to nvidia-nim profile");

    assert_eq!(
        Provider::name(&provider),
        "OpenRouter",
        "machine-facing name must stay stable for billing/routing"
    );
    assert_eq!(
        Provider::display_name(&provider),
        "NVIDIA NIM",
        "header/UI display name must reflect the active runtime profile"
    );

    // Switching back to the plain OpenRouter aggregator restores the label.
    provider
        .set_model("anthropic/claude-sonnet-4")
        .expect("switch back to openrouter aggregator");
    assert_eq!(Provider::display_name(&provider), "OpenRouter");
}
