fn agent_failover_set_default_model(model: &str) -> anyhow::Result<()> {
    crate::config::Config::set_default_model_only((!model.trim().is_empty()).then_some(model))
}

fn agent_failover_catalog(provider: &MultiProvider, routes: Vec<ModelRoute>) {
    // Read the pricing generation first: it may load the config and advance
    // the reload generation that `routes_memo_key` folds in. Capturing the key
    // afterwards keeps the injected entry fresh for its own lookup.
    let pricing_generation = crate::model_pricing::pricing_generation();
    *provider.routes_memo.lock().unwrap() = Some(RoutesMemoEntry {
        built_at: std::time::Instant::now(),
        scope_key: provider.routes_memo_key(),
        auth_generation: pricing::auth_pricing_generation(),
        catalog_generation: catalog_generation(),
        pricing_generation,
        listable_models: listable_model_names_from_routes(&routes),
        routes,
    });
}

fn agent_failover_fixture(models: &[&str]) -> MultiProvider {
    crate::env::set_var("OPENAI_API_KEY", "sk-test-failover");
    let provider = test_multi_provider_with_openai();
    let source = test_anthropic_runtime();
    source.set_credential_mode(CredentialMode::OAuth).unwrap();
    source.set_oauth_account(Some("claude-otter")).unwrap();
    *provider.anthropic.write().unwrap() = Some(source);
    *provider.active.write().unwrap() = ActiveProvider::Claude;
    crate::auth::account_pool::AccountPool {
        members: vec!["claude:claude-otter".into(), "openai-api".into()],
        excluded: vec![],
    }
    .save()
    .unwrap();
    agent_failover_catalog(
        &provider,
        models
            .iter()
            .map(|model| build_openai_api_key_route(model, true, ""))
            .collect(),
    );
    provider
}

fn agent_failover_target(provider: &MultiProvider, tokens: usize, reason: &str) -> Option<String> {
    // Changing the configured default bumps the config generation, which is
    // part of the memo key. Re-key the fixture catalog so the test keeps
    // measuring its own routes instead of the real built-in catalog.
    let routes = provider
        .routes_memo
        .lock()
        .unwrap()
        .as_ref()
        .map(|entry| entry.routes.clone());
    if let Some(routes) = routes {
        agent_failover_catalog(provider, routes);
    }
    provider
        .cross_provider_pool_prompt(
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            reason.into(),
            tokens.saturating_mul(4),
            tokens,
        )
        .and_then(|prompt| prompt.target_route)
}

#[test]
fn agent_failover_rejects_incident_sora_escape_after_oversized_oauth_failure() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        for models in [["gpt-6-astra", "sora-2-pro"], ["sora-2-pro", "gpt-6-astra"]] {
            let provider = agent_failover_fixture(&models);
            assert_eq!(
                agent_failover_target(
                    &provider,
                    1_376_145,
                    "Claude OAuth refresh token was previously rejected by Anthropic and cannot be refreshed"
                ),
                None,
                "no fitting agent route is safer than an unknown-window video model"
            );
            assert_eq!(
                agent_failover_target(&provider, 1_376_145, "context length exceeded"),
                None
            );
        }
    });
}

#[test]
fn agent_failover_prefers_curated_model_independent_of_catalog_order() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        for models in [
            ["sora-2-pro", "gpt-5.4", "gpt-6-astra"],
            ["gpt-6-astra", "sora-2-pro", "gpt-5.4"],
            ["gpt-5.4", "gpt-6-astra", "sora-2-pro"],
        ] {
            let provider = agent_failover_fixture(&models);
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                Some("openai-api:gpt-6-astra".into())
            );
        }
    });
}

#[test]
fn agent_failover_curated_order_beats_larger_context_window() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        for models in [["gpt-5.4", "gpt-5.5"], ["gpt-5.5", "gpt-5.4"]] {
            let provider = agent_failover_fixture(&models);
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                Some("openai-api:gpt-5.5".into()),
                "272K preferred model outranks a 1M less-preferred model when both fit"
            );
        }
    });
}

#[test]
fn agent_failover_configured_unknown_context_cannot_bypass_capacity_check() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        agent_failover_set_default_model("openai-api:gpt-7-sol").unwrap();
        let provider = agent_failover_fixture(&["gpt-7-sol"]);
        assert!(super::models::is_agent_model_for_failover(
            "gpt-7-sol",
            "openai"
        ));
        assert_eq!(
            super::models::context_limit_for_model_with_provider("gpt-7-sol", Some("openai")),
            None
        );
        assert_eq!(
            agent_failover_target(&provider, 128, "401 unauthorized"),
            None
        );
    });
}

#[test]
fn agent_failover_configured_model_beats_larger_curated_model_when_both_fit() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        agent_failover_set_default_model("openai-api:gpt-5.5").unwrap();
        let provider = agent_failover_fixture(&["gpt-6-astra", "gpt-5.5", "sora-2-pro"]);
        assert_eq!(
            agent_failover_target(&provider, 128, "401 unauthorized"),
            Some("openai-api:gpt-5.5".into())
        );
        assert_eq!(
            agent_failover_target(&provider, 300_000, "401 unauthorized"),
            Some("openai-api:gpt-6-astra".into()),
            "configured model cannot bypass sufficient-context eligibility"
        );
    });
}

#[test]
fn agent_failover_skips_unavailable_configured_model_and_never_prefers_media() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let provider = agent_failover_fixture(&["gpt-6-astra", "gpt-5.5", "sora-2-pro"]);
        for configured in ["openai-api:gpt-5.5", "openai-api:sora-2-pro"] {
            agent_failover_set_default_model(configured).unwrap();
            agent_failover_catalog(
                &provider,
                vec![
                    build_openai_api_key_route("sora-2-pro", true, ""),
                    build_openai_api_key_route("gpt-5.5", false, "model access denied"),
                    build_openai_api_key_route("gpt-6-astra", true, ""),
                ],
            );
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                Some("openai-api:gpt-6-astra".into())
            );
        }
    });
}

#[test]
fn agent_failover_media_only_catalog_and_countdown_targets_are_rejected() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let models = [
            "sora-2-pro",
            "dall-e-3",
            "gpt-image-1",
            "gpt-4o-audio-preview",
            "gpt-4o-realtime-preview",
            "gpt-4o-mini-tts",
            "gpt-4o-transcribe",
            "gpt-4o-search-preview",
            "gpt-3.5-turbo-instruct",
            "text-embedding-3-large",
            "omni-moderation-latest",
            "davinci-002",
            "babbage-002",
            "gpt-5.6-pro[web]",
        ];
        let provider = agent_failover_fixture(&models);
        for model in models {
            agent_failover_set_default_model(&format!("openai-api:{model}")).unwrap();
            assert!(!MultiProvider::is_failover_route_pool_approved(&format!(
                "openai-api:{model}"
            )));
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                None
            );
        }
        assert!(MultiProvider::is_failover_route_pool_approved(
            "openai-api:gpt-6-astra"
        ));
    });
}

#[test]
fn agent_failover_unranked_models_have_no_catalog_or_context_tiebreak_fallback() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        for models in [
            ["gpt-6.1-terra", "gpt-6.1-luna"],
            ["gpt-6.1-luna", "gpt-6.1-terra"],
        ] {
            let provider = agent_failover_fixture(&models);
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                None
            );
            agent_failover_set_default_model("openai-api:gpt-6.1-terra").unwrap();
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                Some("openai-api:gpt-6.1-terra".into()),
                "an explicitly configured eligible new GPT variant is supported"
            );
            agent_failover_set_default_model("").unwrap();
        }
    });
}

#[test]
fn agent_failover_configured_oauth_model_does_not_override_api_identity() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let provider = agent_failover_fixture(&["gpt-6.1-sol", "gpt-6-astra", "gpt-5.5"]);
        for configured in ["openai-oauth:gpt-5.5", "openai-oauth@openai-fox:gpt-5.5"] {
            agent_failover_set_default_model(configured).unwrap();
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                Some("openai-api:gpt-6.1-sol".into())
            );
        }
        for configured in ["openai-api-key:gpt-5.5", "gpt-5.5"] {
            agent_failover_set_default_model(configured).unwrap();
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                Some("openai-api:gpt-5.5".into())
            );
        }
    });
}

#[test]
fn agent_failover_pool_order_beats_global_model_quality_and_configured_identity() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let provider = agent_failover_fixture(&["gpt-6-astra"]);
        agent_failover_set_default_model("openai-api:gpt-6-astra").unwrap();
        crate::auth::account_pool::AccountPool {
            members: vec![
                "claude:claude-otter".into(),
                "anthropic-api".into(),
                "openai-api".into(),
            ],
            excluded: vec![],
        }
        .save()
        .unwrap();
        agent_failover_catalog(
            &provider,
            vec![
                build_openai_api_key_route("gpt-6-astra", true, ""),
                ModelRoute {
                    model: "claude-sonnet-4-6".into(),
                    provider: "Anthropic".into(),
                    api_method: "anthropic-api-key".into(),
                    available: true,
                    detail: String::new(),
                    usage: None,
                    cheapness: None,
                },
            ],
        );
        assert_eq!(
            agent_failover_target(&provider, 128, "401 unauthorized"),
            Some("claude-api:claude-sonnet-4-6".into())
        );
        let mut pool = crate::auth::account_pool::AccountPool::load();
        pool.excluded.push("anthropic-api".into());
        pool.save().unwrap();
        assert_eq!(
            agent_failover_target(&provider, 128, "401 unauthorized"),
            Some("openai-api:gpt-6-astra".into())
        );
        pool.members.retain(|key| key != "openai-api");
        pool.save().unwrap();
        assert_eq!(
            agent_failover_target(&provider, 128, "401 unauthorized"),
            None
        );
    });
}

#[test]
fn agent_failover_same_model_rotation_precedes_config_and_respects_context() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let (provider, _attempts, _cleanup) = account_routing_fixture(&[]);
        agent_failover_set_default_model("openai-oauth:gpt-6-astra").unwrap();
        let preserved = provider
            .cross_provider_pool_prompt(
                ActiveProvider::OpenAI,
                ActiveProvider::OpenAI,
                "401 unauthorized".into(),
                512,
                128,
            )
            .unwrap();
        assert_eq!(
            preserved.target_route.as_deref(),
            Some("openai-oauth@openai-fox:gpt-5.4")
        );
        assert!(
            provider
                .cross_provider_pool_prompt(
                    ActiveProvider::OpenAI,
                    ActiveProvider::OpenAI,
                    "401 unauthorized".into(),
                    5_504_580,
                    1_376_145,
                )
                .is_none(),
            "synthetic preserved-model candidates must also fit the input"
        );
    });
}

#[test]
fn agent_failover_configured_account_pin_matches_only_exact_target_login() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let provider = agent_failover_fixture(&[]);
        auth::codex::upsert_account_from_tokens("openai-fox", "fresh", "refresh", None, None)
            .unwrap();
        let mut pool = crate::auth::account_pool::AccountPool::load();
        pool.members.insert(1, "openai:openai-fox".into());
        pool.save().unwrap();
        for (configured, expected) in [
            ("openai-oauth@openai-panda:gpt-5.5", "gpt-6-astra"),
            ("openai-api:gpt-5.5", "gpt-6-astra"),
            ("openai-oauth@openai-fox:gpt-5.5", "gpt-5.5"),
            ("openai-oauth:gpt-5.5", "gpt-5.5"),
        ] {
            agent_failover_set_default_model(configured).unwrap();
            agent_failover_catalog(
                &provider,
                ["gpt-6-astra", "gpt-5.5"]
                    .into_iter()
                    .map(|model| ModelRoute {
                        api_method: "openai-oauth@openai-fox".into(),
                        ..build_openai_oauth_route(model, true, "")
                    })
                    .collect(),
            );
            assert_eq!(
                agent_failover_target(&provider, 128, "401 unauthorized"),
                Some(format!("openai-oauth@openai-fox:{expected}"))
            );
        }
    });
}

#[test]
fn agent_failover_bedrock_without_tools_is_rejected_at_countdown_recheck() {
    with_clean_provider_test_env(|| {
        crate::auth::account_pool::AccountPool {
            members: vec!["bedrock".into()],
            excluded: vec![],
        }
        .save()
        .unwrap();
        for model in ["meta.llama3-1-405b-instruct", "mistral.mistral-large"] {
            assert!(!MultiProvider::is_failover_route_pool_approved(&format!(
                "bedrock:{model}"
            )));
        }
        assert!(MultiProvider::is_failover_route_pool_approved(
            "bedrock:us.anthropic.claude-sonnet-4-6"
        ));
    });
}
