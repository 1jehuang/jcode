use super::*;

fn with_clean_provider_test_env<T>(f: impl FnOnce() -> T) -> T {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_subscription =
        std::env::var_os(crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV);
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::subscription_catalog::clear_runtime_env();
    crate::auth::claude::set_active_account_override(None);
    crate::auth::codex::set_active_account_override(None);

    let result = f();

    crate::auth::claude::set_active_account_override(None);
    crate::auth::codex::set_active_account_override(None);
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    if let Some(prev_subscription) = prev_subscription {
        crate::env::set_var(
            crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV,
            prev_subscription,
        );
    } else {
        crate::env::remove_var(crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV);
    }
    crate::subscription_catalog::clear_runtime_env();
    result
}

fn enter_test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn with_env_var<T>(key: &str, value: &str, f: impl FnOnce() -> T) -> T {
    let prev = std::env::var_os(key);
    crate::env::set_var(key, value);
    let result = f();
    if let Some(prev) = prev {
        crate::env::set_var(key, prev);
    } else {
        crate::env::remove_var(key);
    }
    result
}

fn test_multi_provider_with_cursor() -> MultiProvider {
    MultiProvider {
        claude: RwLock::new(None),
        anthropic: RwLock::new(None),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(Some(Arc::new(cursor::CursorCliProvider::new()))),
        openrouter: RwLock::new(None),
        active: RwLock::new(ActiveProvider::Cursor),
        use_claude_cli: false,
        startup_notices: RwLock::new(Vec::new()),
        forced_provider: None,
    }
}

fn test_multi_provider_with_antigravity() -> MultiProvider {
    MultiProvider {
        claude: RwLock::new(None),
        anthropic: RwLock::new(None),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(Some(Arc::new(antigravity::AntigravityCliProvider::new()))),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        openrouter: RwLock::new(None),
        active: RwLock::new(ActiveProvider::Antigravity),
        use_claude_cli: false,
        startup_notices: RwLock::new(Vec::new()),
        forced_provider: None,
    }
}

#[test]
fn test_on_auth_changed_hot_initializes_openai_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::OpenAI),
        };

        crate::auth::codex::upsert_account_from_tokens(
            "openai-1",
            "test-access-token",
            "test-refresh-token",
            None,
            None,
        )
        .expect("save test OpenAI auth");

        provider.on_auth_changed();

        assert!(provider.openai_provider().is_some());
        assert!(provider.model_routes().iter().any(|route| {
            route.provider == "OpenAI" && route.api_method == "openai-oauth" && route.available
        }));
    });
}

#[test]
fn test_on_auth_changed_refreshes_existing_openai_provider_credentials() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        crate::auth::codex::upsert_account_from_tokens(
            "openai-1",
            "stale-access-token",
            "test-refresh-token",
            None,
            None,
        )
        .expect("save stale test OpenAI auth");

        let existing = Arc::new(openai::OpenAIProvider::new(
            crate::auth::codex::load_credentials().expect("load stale openai credentials"),
        ));

        crate::auth::codex::upsert_account_from_tokens(
            "openai-1",
            "fresh-access-token",
            "test-refresh-token",
            None,
            None,
        )
        .expect("save fresh test OpenAI auth");

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(Some(existing.clone())),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::OpenAI),
        };

        provider.on_auth_changed();

        let openai = provider
            .openai_provider()
            .expect("existing openai provider");
        let loaded = runtime.block_on(async { openai.test_access_token().await });
        assert_eq!(loaded, "fresh-access-token");
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_anthropic_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Claude),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::Claude),
        };

        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "test-access-token".to_string(),
            refresh: "test-refresh-token".to_string(),
            expires: i64::MAX,
            email: None,
            subscription_type: None,
        })
        .expect("save test Claude auth");

        provider.on_auth_changed();

        assert!(provider.anthropic_provider().is_some());
        assert!(provider.model_routes().iter().any(|route| {
            route.provider == "Anthropic" && route.api_method == "claude-oauth" && route.available
        }));
    });
}

#[test]
fn test_anthropic_model_routes_keep_plain_4_6_available_without_extra_usage() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Claude),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::Claude),
        };

        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "test-access-token".to_string(),
            refresh: "test-refresh-token".to_string(),
            expires: i64::MAX,
            email: None,
            subscription_type: None,
        })
        .expect("save test Claude auth");

        provider.on_auth_changed();

        let routes = provider.model_routes();
        let plain_opus = routes
            .iter()
            .find(|route| {
                route.provider == "Anthropic"
                    && route.api_method == "claude-oauth"
                    && route.model == "claude-opus-4-6"
            })
            .expect("plain opus route");
        assert!(plain_opus.available);
        assert!(plain_opus.detail.is_empty());

        let opus_1m = routes
            .iter()
            .find(|route| {
                route.provider == "Anthropic"
                    && route.api_method == "claude-oauth"
                    && route.model == "claude-opus-4-6[1m]"
            })
            .expect("1m opus route");
        assert!(!opus_1m.available);
        assert_eq!(opus_1m.detail, "requires extra usage");
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_openrouter_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            with_env_var("JCODE_OPENROUTER_MODEL_CATALOG", "0", || {
                let runtime = enter_test_runtime();
                let _enter = runtime.enter();

                let provider = MultiProvider {
                    claude: RwLock::new(None),
                    anthropic: RwLock::new(None),
                    openai: RwLock::new(None),
                    copilot_api: RwLock::new(None),
                    antigravity: RwLock::new(None),
                    gemini: RwLock::new(None),
                    cursor: RwLock::new(None),
                    openrouter: RwLock::new(None),
                    active: RwLock::new(ActiveProvider::OpenRouter),
                    use_claude_cli: false,
                    startup_notices: RwLock::new(Vec::new()),
                    forced_provider: Some(ActiveProvider::OpenRouter),
                };

                provider.on_auth_changed();

                assert!(provider.openrouter.read().unwrap().is_some());
                assert!(
                    provider
                        .model_routes()
                        .iter()
                        .any(|route| { route.api_method == "openrouter" && route.available })
                );
            })
        })
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_copilot_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        with_env_var("GITHUB_TOKEN", "gho_test_token", || {
            crate::auth::AuthStatus::invalidate_cache();
            let runtime = enter_test_runtime();
            let _enter = runtime.enter();

            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(None),
                openrouter: RwLock::new(None),
                active: RwLock::new(ActiveProvider::Copilot),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: Some(ActiveProvider::Copilot),
            };

            provider.on_auth_changed();

            assert!(provider.copilot_api.read().unwrap().is_some());
            assert!(provider.model_routes().iter().any(|route| {
                route.provider == "Copilot" && route.api_method == "copilot" && route.available
            }));
        })
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_gemini_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        crate::auth::gemini::save_tokens(&crate::auth::gemini::GeminiTokens {
            access_token: "test-access-token".to_string(),
            refresh_token: "test-refresh-token".to_string(),
            expires_at: i64::MAX,
            email: None,
        })
        .expect("save test Gemini auth");

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Gemini),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::Gemini),
        };

        provider.on_auth_changed();

        assert!(provider.gemini_provider().is_some());
        assert!(provider.model_routes().iter().any(|route| {
            route.provider == "Gemini" && route.api_method == "code-assist-oauth" && route.available
        }));
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_cursor_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        with_env_var("CURSOR_API_KEY", "cursor-test-key", || {
            crate::auth::AuthStatus::invalidate_cache();
            let runtime = enter_test_runtime();
            let _enter = runtime.enter();

            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(None),
                openrouter: RwLock::new(None),
                active: RwLock::new(ActiveProvider::Cursor),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: Some(ActiveProvider::Cursor),
            };

            provider.on_auth_changed();

            assert!(provider.cursor.read().unwrap().is_some());
            assert!(provider.model_routes().iter().any(|route| {
                route.provider == "Cursor" && route.api_method == "cursor" && route.available
            }));
        })
    });
}

#[test]
fn test_provider_for_model_claude() {
    assert_eq!(provider_for_model("claude-opus-4-6"), Some("claude"));
    assert_eq!(provider_for_model("claude-opus-4-6[1m]"), Some("claude"));
    assert_eq!(provider_for_model("claude-sonnet-4-6"), Some("claude"));
}

#[test]
fn test_provider_for_model_openai() {
    assert_eq!(provider_for_model("gpt-5.2-codex"), Some("openai"));
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
fn test_resolve_model_capabilities_respects_antigravity_provider_hint() {
    let caps = resolve_model_capabilities("gpt-oss-120b-medium", Some("antigravity"));
    assert_eq!(caps.provider.as_deref(), Some("antigravity"));
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

#[test]
fn test_fallback_sequence_includes_all_providers() {
    assert_eq!(
        MultiProvider::fallback_sequence(ActiveProvider::Claude),
        vec![
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Copilot,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
            ActiveProvider::OpenRouter,
        ]
    );
    assert_eq!(
        MultiProvider::fallback_sequence(ActiveProvider::OpenAI),
        vec![
            ActiveProvider::OpenAI,
            ActiveProvider::Claude,
            ActiveProvider::Copilot,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
            ActiveProvider::OpenRouter,
        ]
    );
    assert_eq!(
        MultiProvider::fallback_sequence(ActiveProvider::Copilot),
        vec![
            ActiveProvider::Copilot,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
            ActiveProvider::OpenRouter,
        ]
    );
    assert_eq!(
        MultiProvider::fallback_sequence(ActiveProvider::Gemini),
        vec![
            ActiveProvider::Gemini,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Copilot,
            ActiveProvider::Cursor,
            ActiveProvider::OpenRouter,
        ]
    );
    assert_eq!(
        MultiProvider::fallback_sequence(ActiveProvider::OpenRouter),
        vec![
            ActiveProvider::OpenRouter,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Copilot,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
        ]
    );
}

#[test]
fn test_parse_provider_hint_supports_known_values() {
    assert_eq!(
        MultiProvider::parse_provider_hint("claude"),
        Some(ActiveProvider::Claude)
    );
    assert_eq!(
        MultiProvider::parse_provider_hint("Anthropic"),
        Some(ActiveProvider::Claude)
    );
    assert_eq!(
        MultiProvider::parse_provider_hint("openai"),
        Some(ActiveProvider::OpenAI)
    );
    assert_eq!(
        MultiProvider::parse_provider_hint("copilot"),
        Some(ActiveProvider::Copilot)
    );
    assert_eq!(
        MultiProvider::parse_provider_hint("gemini"),
        Some(ActiveProvider::Gemini)
    );
    assert_eq!(
        MultiProvider::parse_provider_hint("openrouter"),
        Some(ActiveProvider::OpenRouter)
    );
    assert_eq!(
        MultiProvider::parse_provider_hint("cursor"),
        Some(ActiveProvider::Cursor)
    );
}

#[test]
fn test_cursor_models_are_included_in_available_models_display_when_configured() {
    with_clean_provider_test_env(|| {
        let provider = test_multi_provider_with_cursor();
        let models = provider.available_models_display();
        assert!(models.iter().any(|model| model == "composer-2-fast"));
        assert!(models.iter().any(|model| model == "composer-2"));
    });
}

#[test]
fn test_cursor_models_are_included_in_model_routes_when_configured() {
    with_clean_provider_test_env(|| {
        let provider = test_multi_provider_with_cursor();
        let routes = provider.model_routes();
        assert!(routes.iter().any(|route| {
            route.model == "composer-2-fast"
                && route.provider == "Cursor"
                && route.api_method == "cursor"
                && route.available
        }));
    });
}

#[test]
fn test_set_model_switches_to_cursor_for_cursor_models() {
    with_clean_provider_test_env(|| {
        let provider = test_multi_provider_with_cursor();
        *provider.active.write().unwrap() = ActiveProvider::Claude;

        provider
            .set_model("composer-2-fast")
            .expect("cursor model should route to Cursor");

        assert_eq!(provider.active_provider(), ActiveProvider::Cursor);
        assert_eq!(provider.model(), "composer-2-fast");
    });
}

#[test]
fn test_set_model_supports_explicit_cursor_prefix() {
    with_clean_provider_test_env(|| {
        let provider = test_multi_provider_with_cursor();
        *provider.active.write().unwrap() = ActiveProvider::OpenAI;

        provider
            .set_model("cursor:gpt-5")
            .expect("explicit cursor prefix should force Cursor route");

        assert_eq!(provider.active_provider(), ActiveProvider::Cursor);
        assert_eq!(provider.model(), "gpt-5");
    });
}

#[test]
fn test_antigravity_models_are_included_in_model_routes_when_configured() {
    with_clean_provider_test_env(|| {
        let cache_path = crate::storage::app_config_dir()
            .expect("app config dir")
            .join("antigravity_models_cache.json");
        crate::storage::write_json(
            &cache_path,
            &serde_json::json!({
                "models": [
                    {
                        "id": "claude-sonnet-4-6",
                        "display_name": "Claude Sonnet 4.6",
                        "recommended": true,
                        "available": true,
                        "remaining_fraction_milli": 1000
                    }
                ],
                "fetched_at_rfc3339": "2026-04-17T20:53:26Z"
            }),
        )
        .expect("write antigravity cache");

        let provider = test_multi_provider_with_antigravity();
        let routes = provider.model_routes();
        assert!(routes.iter().any(|route| {
            route.model == "claude-sonnet-4-6"
                && route.provider == "Antigravity"
                && route.api_method == "cli"
                && route.available
        }));
    });
}

#[test]
fn test_set_model_supports_explicit_antigravity_prefix() {
    with_clean_provider_test_env(|| {
        let provider = test_multi_provider_with_antigravity();
        *provider.active.write().unwrap() = ActiveProvider::OpenAI;

        provider
            .set_model("antigravity:claude-sonnet-4-6")
            .expect("explicit antigravity prefix should force Antigravity route");

        assert_eq!(provider.active_provider(), ActiveProvider::Antigravity);
        assert_eq!(provider.model(), "claude-sonnet-4-6");
    });
}

#[test]
fn test_forced_provider_disables_cross_provider_fallback_sequence() {
    assert_eq!(
        MultiProvider::fallback_sequence_for(ActiveProvider::Claude, Some(ActiveProvider::OpenAI)),
        vec![ActiveProvider::Claude, ActiveProvider::OpenAI]
    );
    assert_eq!(
        MultiProvider::fallback_sequence_for(ActiveProvider::OpenAI, Some(ActiveProvider::OpenAI)),
        vec![ActiveProvider::OpenAI]
    );
    assert_eq!(
        MultiProvider::fallback_sequence_for(ActiveProvider::Claude, None),
        MultiProvider::fallback_sequence(ActiveProvider::Claude)
    );
}

#[test]
fn test_set_model_rejects_cross_provider_without_creds() {
    let _guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
    crate::env::remove_var("JCODE_FORCE_PROVIDER");

    let provider = MultiProvider {
        claude: RwLock::new(None),
        anthropic: RwLock::new(None),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        openrouter: RwLock::new(None),
        active: RwLock::new(ActiveProvider::OpenAI),
        use_claude_cli: false,
        startup_notices: RwLock::new(Vec::new()),
        forced_provider: Some(ActiveProvider::OpenAI),
    };

    let err = provider
        .set_model("claude-sonnet-4-6")
        .expect_err("model routing should reject when target provider has no creds");
    assert!(
        err.to_string().contains("credentials are not configured"),
        "expected credentials error, got: {}",
        err
    );
}

#[test]
fn test_auto_default_prefers_openai_over_claude_when_both_available() {
    let active =
        MultiProvider::auto_default_provider(true, true, false, false, false, false, false, false);
    assert_eq!(active, ActiveProvider::OpenAI);
}

#[test]
fn test_auto_default_prefers_copilot_when_zero_premium_mode_enabled() {
    let active = MultiProvider::auto_default_provider(true, true, true, true, true, true, true, true);
    assert_eq!(active, ActiveProvider::Copilot);
}

#[test]
fn test_should_failover_on_403_forbidden() {
    let err = anyhow::anyhow!(
        "Copilot token exchange failed (HTTP 403 Forbidden): not accessible by integration"
    );
    assert!(MultiProvider::classify_failover_error(&err).should_failover());
}

#[test]
fn test_should_failover_on_token_exchange_failed() {
    let msg = r#"Copilot token exchange failed (HTTP 403 Forbidden): {"error_details":{"title":"Contact Support"}}"#;
    let err = anyhow::anyhow!("{}", msg);
    assert!(MultiProvider::classify_failover_error(&err).should_failover());
}

#[test]
fn test_should_failover_on_access_denied() {
    let err = anyhow::anyhow!("Access denied: account suspended");
    assert!(MultiProvider::classify_failover_error(&err).should_failover());
}

#[test]
fn test_should_failover_when_status_code_starts_message() {
    let err = anyhow::anyhow!("401 unauthorized");
    assert!(MultiProvider::classify_failover_error(&err).should_failover());
    assert_eq!(
        MultiProvider::classify_failover_error(&err),
        FailoverDecision::RetryAndMarkUnavailable
    );
}

#[test]
fn test_should_not_failover_on_non_standalone_status_digits() {
    let err = anyhow::anyhow!("backend returned code 14290");
    assert!(!MultiProvider::classify_failover_error(&err).should_failover());
}

#[test]
fn test_context_limit_error_fails_over_without_marking_provider_unavailable() {
    let err = anyhow::anyhow!("Context length exceeded maximum context window");
    assert!(MultiProvider::classify_failover_error(&err).should_failover());
    assert_eq!(
        MultiProvider::classify_failover_error(&err),
        FailoverDecision::RetryNextProvider
    );
}

#[test]
fn test_should_not_failover_on_generic_error() {
    let err = anyhow::anyhow!("Connection timed out");
    assert!(!MultiProvider::classify_failover_error(&err).should_failover());
}

#[test]
fn test_no_provider_error_mentions_tokens_and_details() {
    let provider = MultiProvider {
        claude: RwLock::new(None),
        anthropic: RwLock::new(None),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        openrouter: RwLock::new(None),
        active: RwLock::new(ActiveProvider::OpenAI),
        use_claude_cli: false,
        startup_notices: RwLock::new(Vec::new()),
        forced_provider: None,
    };
    let err = provider.no_provider_available_error(&[
        "OpenAI: rate limited".to_string(),
        "GitHub Copilot: not configured".to_string(),
    ]);
    let text = err.to_string();
    assert!(text.contains("No tokens/providers left"));
    assert!(text.contains("OpenAI: rate limited"));
    assert!(text.contains("GitHub Copilot: not configured"));
}

#[test]
fn test_openai_provider_unavailability_is_scoped_per_account() {
    let _guard = crate::storage::lock_test_env();

    crate::auth::codex::set_active_account_override(Some("work".to_string()));
    clear_all_provider_unavailability_for_account();
    record_provider_unavailable_for_account("openai", "work rate limit");
    assert!(
        provider_unavailability_detail_for_account("openai")
            .unwrap_or_default()
            .contains("work rate limit")
    );

    crate::auth::codex::set_active_account_override(Some("personal".to_string()));
    clear_all_provider_unavailability_for_account();
    assert!(provider_unavailability_detail_for_account("openai").is_none());

    crate::auth::codex::set_active_account_override(Some("work".to_string()));
    assert!(
        provider_unavailability_detail_for_account("openai")
            .unwrap_or_default()
            .contains("work rate limit")
    );

    clear_all_provider_unavailability_for_account();
    crate::auth::codex::set_active_account_override(None);
}

#[test]
fn test_openai_model_catalog_is_scoped_per_account() {
    let _guard = crate::storage::lock_test_env();
    let work_model = "scoped-work-model-123";
    let personal_model = "scoped-personal-model-456";

    crate::auth::codex::set_active_account_override(Some("work".to_string()));
    populate_account_models(vec![work_model.to_string()]);
    assert!(known_openai_model_ids().contains(&work_model.to_string()));
    assert!(!known_openai_model_ids().contains(&personal_model.to_string()));

    crate::auth::codex::set_active_account_override(Some("personal".to_string()));
    assert!(!known_openai_model_ids().contains(&work_model.to_string()));
    populate_account_models(vec![personal_model.to_string()]);
    assert!(known_openai_model_ids().contains(&personal_model.to_string()));
    assert!(!known_openai_model_ids().contains(&work_model.to_string()));

    crate::auth::codex::set_active_account_override(Some("work".to_string()));
    assert!(known_openai_model_ids().contains(&work_model.to_string()));
    assert!(!known_openai_model_ids().contains(&personal_model.to_string()));

    crate::auth::codex::set_active_account_override(None);
}

#[test]
fn test_openai_live_catalog_replaces_static_fallback_list() {
    let _guard = crate::storage::lock_test_env();
    crate::auth::codex::set_active_account_override(Some("work".to_string()));

    populate_account_models(vec!["gpt-5.4-live-only".to_string()]);
    let models = known_openai_model_ids();

    assert_eq!(models, vec!["gpt-5.4-live-only".to_string()]);

    crate::auth::codex::set_active_account_override(None);
}

#[test]
fn test_anthropic_live_catalog_replaces_static_fallback_list() {
    let _guard = crate::storage::lock_test_env();
    crate::env::remove_var("ANTHROPIC_API_KEY");
    crate::auth::claude::set_active_account_override(Some("work".to_string()));

    populate_context_limits(
        [("claude-opus-4-7".to_string(), 1_048_576)]
            .into_iter()
            .collect(),
    );
    populate_anthropic_models(vec!["claude-opus-4-7".to_string()]);
    let models = known_anthropic_model_ids();

    assert_eq!(
        models,
        vec![
            "claude-opus-4-7".to_string(),
            "claude-opus-4-7[1m]".to_string()
        ]
    );

    crate::auth::claude::set_active_account_override(None);
}

#[test]
fn test_openai_model_catalog_hydrates_from_disk_cache() {
    with_clean_provider_test_env(|| {
        crate::auth::codex::set_active_account_override(Some("disk-openai".to_string()));
        persist_openai_model_catalog(&OpenAIModelCatalog {
            available_models: vec!["openai-disk-only-model".to_string()],
            context_limits: [("openai-disk-only-model".to_string(), 424_242)]
                .into_iter()
                .collect(),
        });

        assert_eq!(
            cached_openai_model_ids(),
            Some(vec!["openai-disk-only-model".to_string()])
        );
        assert_eq!(
            context_limit_for_model("openai-disk-only-model"),
            Some(424_242)
        );

        crate::auth::codex::set_active_account_override(None);
    });
}

#[test]
fn test_anthropic_model_catalog_hydrates_from_disk_cache() {
    with_clean_provider_test_env(|| {
        crate::env::remove_var("ANTHROPIC_API_KEY");
        crate::auth::claude::set_active_account_override(Some("disk-claude".to_string()));
        persist_anthropic_model_catalog(&AnthropicModelCatalog {
            available_models: vec!["claude-opus-4-7".to_string()],
            context_limits: [("claude-opus-4-7".to_string(), 1_048_576)]
                .into_iter()
                .collect(),
        });

        assert_eq!(
            cached_anthropic_model_ids(),
            Some(vec![
                "claude-opus-4-7".to_string(),
                "claude-opus-4-7[1m]".to_string()
            ])
        );
        assert_eq!(context_limit_for_model("claude-opus-4-7"), Some(1_048_576));

        crate::auth::claude::set_active_account_override(None);
    });
}

#[test]
fn test_same_provider_account_candidates_include_other_openai_accounts() {
    with_clean_provider_test_env(|| {
        let now_ms = chrono::Utc::now().timestamp_millis() + 60_000;
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "seed-a".to_string(),
            access_token: "acc-a".to_string(),
            refresh_token: "ref-a".to_string(),
            id_token: None,
            account_id: Some("acct-a".to_string()),
            expires_at: Some(now_ms),
            email: Some("a@example.com".to_string()),
        })
        .unwrap();
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "seed-b".to_string(),
            access_token: "acc-b".to_string(),
            refresh_token: "ref-b".to_string(),
            id_token: None,
            account_id: Some("acct-b".to_string()),
            expires_at: Some(now_ms),
            email: Some("b@example.com".to_string()),
        })
        .unwrap();

        crate::auth::codex::set_active_account("openai-1").unwrap();
        let candidates = MultiProvider::same_provider_account_candidates(ActiveProvider::OpenAI);
        assert_eq!(candidates, vec!["openai-2".to_string()]);
    });
}

#[test]
fn test_normalize_copilot_model_name_claude() {
    assert_eq!(
        normalize_copilot_model_name("claude-opus-4.6"),
        Some("claude-opus-4-6")
    );
    assert_eq!(
        normalize_copilot_model_name("claude-sonnet-4.6"),
        Some("claude-sonnet-4-6")
    );
    assert_eq!(
        normalize_copilot_model_name("claude-sonnet-4.5"),
        Some("claude-sonnet-4-5")
    );
    assert_eq!(
        normalize_copilot_model_name("claude-haiku-4.5"),
        Some("claude-haiku-4-5")
    );
}

#[test]
fn test_normalize_copilot_model_name_already_canonical() {
    assert_eq!(normalize_copilot_model_name("claude-opus-4-6"), None);
    assert_eq!(normalize_copilot_model_name("claude-sonnet-4-6"), None);
    assert_eq!(normalize_copilot_model_name("gpt-5.3-codex"), None);
}

#[test]
fn test_normalize_copilot_model_name_unknown() {
    assert_eq!(normalize_copilot_model_name("gemini-3-pro-preview"), None);
    assert_eq!(normalize_copilot_model_name("grok-code-fast-1"), None);
}

#[test]
fn test_provider_for_model_copilot_dot_notation() {
    assert_eq!(provider_for_model("claude-opus-4.6"), Some("claude"));
    assert_eq!(provider_for_model("claude-sonnet-4.6"), Some("claude"));
    assert_eq!(provider_for_model("claude-haiku-4.5"), Some("claude"));
    assert_eq!(provider_for_model("gpt-4.1"), Some("openai"));
}

#[test]
fn test_subscription_model_guard_allows_only_curated_models_when_enabled() {
    let _guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::subscription_catalog::apply_runtime_env();

    assert!(ensure_model_allowed_for_subscription("moonshotai/kimi-k2.5").is_ok());
    assert!(ensure_model_allowed_for_subscription("kimi/k2.5").is_ok());
    assert!(ensure_model_allowed_for_subscription("gpt-5.4").is_err());

    crate::subscription_catalog::clear_runtime_env();
}

#[test]
fn test_filtered_display_models_respects_curated_subscription_catalog() {
    let _guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::subscription_catalog::apply_runtime_env();

    let filtered = filtered_display_models(vec![
        "gpt-5.4".to_string(),
        "moonshotai/kimi-k2.5".to_string(),
        "openrouter/healer-alpha".to_string(),
    ]);

    assert_eq!(
        filtered,
        vec![
            "moonshotai/kimi-k2.5".to_string(),
            "openrouter/healer-alpha".to_string()
        ]
    );

    crate::subscription_catalog::clear_runtime_env();
}

#[test]
fn test_subscription_filters_do_not_activate_from_saved_credentials_alone() {
    let _guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::env::set_var(crate::subscription_catalog::JCODE_API_KEY_ENV, "test-key");

    assert!(ensure_model_allowed_for_subscription("gpt-5.4").is_ok());
    assert_eq!(
        filtered_display_models(vec![
            "gpt-5.4".to_string(),
            "moonshotai/kimi-k2.5".to_string(),
        ]),
        vec!["gpt-5.4".to_string(), "moonshotai/kimi-k2.5".to_string()]
    );

    crate::env::remove_var(crate::subscription_catalog::JCODE_API_KEY_ENV);
    crate::subscription_catalog::clear_runtime_env();
}
