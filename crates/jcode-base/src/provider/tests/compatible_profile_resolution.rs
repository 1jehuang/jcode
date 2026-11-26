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

/// A bare model id from an OpenAI-compatible catalog must route to the profile
/// that serves it, not to whichever provider happens to be active.
///
/// Regression: `/model celeris-1` while Anthropic was active failed with
/// "Model celeris-1 not supported by Anthropic provider", because bare ids
/// match none of the built-in model-name heuristics and fell through to the
/// active provider.
#[test]
fn bare_openai_compatible_model_ids_route_to_their_profile_not_the_active_provider() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        crate::env::set_var("CELERIS_API_KEY", "test-celeris-key");
        crate::env::set_var("META_MUSE_API_KEY", "test-meta-key");
        let provider = MultiProvider {
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            cursor_acp: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            session_id: RwLock::new(None),
            working_dir: RwLock::new(None),
            // The failing case: a Claude-family provider is active, so the old
            // fallthrough handed the bare id to Anthropic.
            active: RwLock::new(ActiveProvider::Claude),
            active_external: RwLock::new(None),
            startup_notices: RwLock::new(Vec::new()),
            initial_provider: None,
            catalog_scope: next_catalog_scope(),
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        provider
            .set_model("muse-spark-1.2")
            .expect("bare Muse model id should resolve to the Meta Model API profile");
        assert_eq!(provider.model(), "muse-spark-1.2");
        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
        assert_eq!(
            provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
            "meta-muse:muse-spark-1.2"
        );

        provider.set_active_provider(ActiveProvider::Claude);

        provider
            .set_model("celeris-1")
            .expect("bare Celeris model id should resolve to the Celeris profile");
        assert_eq!(provider.model(), "celeris-1");
        assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
        assert_eq!(
            provider.fork_model_switch_request(provider.active_provider(), &provider.model()),
            "celeris:celeris-1"
        );
    });
}
