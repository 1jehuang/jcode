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
        openrouter: RwLock::new(None),
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
                openrouter: RwLock::new(Some(openrouter)),
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
                with_env_var("JCODE_OPENROUTER_API_BASE", "https://compat.example.test/v1", || {
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
                        openrouter: RwLock::new(Some(openrouter)),
                        active: RwLock::new(ActiveProvider::OpenRouter),
                        use_claude_cli: false,
                        startup_notices: RwLock::new(Vec::new()),
                        forced_provider: Some(ActiveProvider::OpenRouter),
                    };

                    provider
                        .set_model("claude-opus4.6-thinking")
                        .expect("forced OpenAI-compatible provider should accept opaque model IDs");

                    assert_eq!(provider.active_provider(), ActiveProvider::OpenRouter);
                    assert_eq!(provider.model(), "claude-opus4.6-thinking");
                })
            })
        })
    });
}

#[test]
fn test_forced_openrouter_preserves_custom_at_sign_model_ids() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            with_env_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0", || {
                with_env_var("JCODE_OPENROUTER_API_BASE", "https://compat.example.test/v1", || {
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
                        openrouter: RwLock::new(Some(openrouter)),
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
                })
            })
        })
    });
}

#[test]
fn test_custom_compatible_model_routes_do_not_request_openrouter_rewrite() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            with_env_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0", || {
                with_env_var("JCODE_OPENROUTER_API_BASE", "https://compat.example.test/v1", || {
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
                        openrouter: RwLock::new(Some(openrouter)),
                        active: RwLock::new(ActiveProvider::OpenRouter),
                        use_claude_cli: false,
                        startup_notices: RwLock::new(Vec::new()),
                        forced_provider: Some(ActiveProvider::OpenRouter),
                    };

                    provider
                        .set_model("claude-opus4.6-thinking")
                        .expect("forced OpenAI-compatible provider should accept opaque model IDs");

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
                })
            })
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
            openrouter: RwLock::new(None),
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
                openrouter::OpenRouterProvider::new().expect("openrouter provider should initialize"),
            );
            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(Some(Arc::new(cursor::CursorCliProvider::new()))),
                openrouter: RwLock::new(Some(openrouter)),
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
