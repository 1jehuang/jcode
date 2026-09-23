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
fn test_openai_reset_clears_only_pinned_account_cooldown() {
    let _guard = crate::storage::lock_test_env();
    let target = "reset-pinned-target";
    let other = "reset-pinned-target-other";
    crate::auth::codex::set_active_account_override(Some(target.to_string()));
    record_provider_unavailable_for_account("openai", "target quota exhausted");
    record_model_unavailable_for_account("reset-denied-model", "model access denied");

    crate::auth::codex::set_active_account_override(Some(other.to_string()));
    record_provider_unavailable_for_account("openai", "other quota exhausted");
    clear_openai_provider_unavailability_for_account_label(Some(target));
    // Idempotent retries cannot clear a different account, including a label
    // that shares the reset target's prefix.
    clear_openai_provider_unavailability_for_account_label(Some(target));
    assert!(provider_unavailability_detail_for_account("openai").is_some());
    assert_eq!(
        crate::auth::codex::active_account_label().as_deref(),
        Some(other)
    );

    crate::auth::codex::set_active_account_override(Some(target.to_string()));
    assert!(provider_unavailability_detail_for_account("openai").is_none());
    assert!(
        model_unavailability_detail_for_account("reset-denied-model")
            .unwrap_or_default()
            .contains("model access denied")
    );
    clear_model_unavailable_for_account("reset-denied-model");
    clear_openai_provider_unavailability_for_account_label(Some(other));
    crate::auth::codex::set_active_account_override(None);
}

#[test]
fn test_openai_reset_default_scope_does_not_follow_active_account() {
    let _guard = crate::storage::lock_test_env();
    crate::auth::codex::set_active_account_override(Some("default".to_string()));
    record_provider_unavailable_for_account("openai", "default quota exhausted");
    crate::auth::codex::set_active_account_override(Some("reset-active-other".to_string()));
    record_provider_unavailable_for_account("openai", "other quota exhausted");
    clear_openai_provider_unavailability_for_account_label(None);
    assert!(provider_unavailability_detail_for_account("openai").is_some());
    clear_openai_provider_unavailability_for_account_label(Some("reset-active-other"));
    crate::auth::codex::set_active_account_override(Some("default".to_string()));
    assert!(provider_unavailability_detail_for_account("openai").is_none());
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

    assert_eq!(
        models[..2],
        [
            "gpt-5.4-live-only".to_string(),
            jcode_provider_core::CHATGPT_WEB_MODEL.to_string()
        ]
    );
    // The only entries allowed past the live catalog are the platform-API-only
    // GPT Pro models, appended when an OPENAI_API_KEY is configured on the
    // machine running the tests.
    for extra in &models[2..] {
        assert!(
            jcode_provider_core::is_openai_api_only_pro_model(extra),
            "unexpected non-pro extra model '{extra}' in live catalog list"
        );
    }

    crate::auth::codex::set_active_account_override(None);
}

#[test]
fn test_anthropic_live_catalog_replaces_static_fallback_list() {
    let _guard = crate::storage::lock_test_env();
    crate::env::remove_var("ANTHROPIC_API_KEY");
    crate::auth::claude::set_active_account_override(Some("work".to_string()));

    // Use a model the static classifier does not recognize so this exercises
    // the generic catalog-driven path (>=1M cached limit => synthesized [1m]
    // alias). The id must carry no parseable version, because any versioned
    // Claude id is now classified statically (>=5.0 => native 1M, which
    // deliberately gets no redundant [1m] alias).
    populate_context_limits(
        [("claude-nebula-preview".to_string(), 1_048_576)]
            .into_iter()
            .collect(),
    );
    populate_anthropic_models(vec!["claude-nebula-preview".to_string()]);
    let models = known_anthropic_model_ids();

    assert_eq!(
        models,
        vec![
            "claude-nebula-preview".to_string(),
            "claude-nebula-preview[1m]".to_string()
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
            reasoning_efforts: [(
                "openai-disk-only-model".to_string(),
                vec!["low".to_string(), "max".to_string()],
            )]
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
        assert_eq!(
            cached_openai_reasoning_efforts()
                .and_then(|efforts| efforts.get("openai-disk-only-model").cloned()),
            Some(vec!["low".to_string(), "max".to_string()])
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
            available_models: vec!["claude-nebula-preview".to_string()],
            context_limits: [("claude-nebula-preview".to_string(), 1_048_576)]
                .into_iter()
                .collect(),
        });

        assert_eq!(
            cached_anthropic_model_ids(),
            Some(vec![
                "claude-nebula-preview".to_string(),
                "claude-nebula-preview[1m]".to_string()
            ])
        );
        assert_eq!(
            context_limit_for_model("claude-nebula-preview"),
            Some(1_048_576)
        );

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

        crate::auth::codex::set_active_account("openai-otter").unwrap();
        let candidates = MultiProvider::same_provider_account_candidates(ActiveProvider::OpenAI);
        assert_eq!(candidates, vec!["openai-fox".to_string()]);
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

    assert!(ensure_model_allowed_for_subscription("claude-opus-4-8").is_ok());
    assert!(ensure_model_allowed_for_subscription("opus 4.8").is_ok());
    assert!(ensure_model_allowed_for_subscription("claude-sonnet-4-6").is_ok());
    assert!(ensure_model_allowed_for_subscription("sonnet 4.6").is_ok());
    assert!(ensure_model_allowed_for_subscription("gpt-5.5").is_ok());
    assert!(ensure_model_allowed_for_subscription("gpt-5.4").is_err());

    crate::subscription_catalog::clear_runtime_env();
}

#[test]
fn test_hosted_model_guard_does_not_gate_models_by_legacy_tier() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::tempdir().expect("temp home");
    crate::env::set_var("JCODE_HOME", temp_home.path().to_string_lossy().to_string());
    crate::env::remove_var(crate::subscription_catalog::JCODE_TIER_ENV);
    crate::subscription_catalog::clear_runtime_env();
    crate::subscription_catalog::apply_runtime_env();

    // Every curated model is available to metered accounts. The router owns
    // spending-limit and model-policy enforcement.
    assert!(ensure_model_allowed_for_subscription("gpt-5.6-sol").is_ok());
    assert!(ensure_model_allowed_for_subscription("claude-fable-5").is_ok());

    // Legacy cached tier metadata cannot change client-side availability.
    crate::env::set_var(crate::subscription_catalog::JCODE_TIER_ENV, "ultra");
    assert!(ensure_model_allowed_for_subscription("claude-fable-5").is_ok());
    assert!(ensure_model_allowed_for_subscription("sol").is_ok());

    crate::env::remove_var(crate::subscription_catalog::JCODE_TIER_ENV);
    crate::env::remove_var("JCODE_HOME");
    crate::subscription_catalog::clear_runtime_env();
}

#[test]
fn test_filtered_display_models_respects_curated_subscription_catalog() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::tempdir().expect("temp home");
    crate::env::set_var("JCODE_HOME", temp_home.path().to_string_lossy().to_string());
    crate::env::remove_var(crate::subscription_catalog::JCODE_TIER_ENV);
    crate::subscription_catalog::clear_runtime_env();
    crate::subscription_catalog::apply_runtime_env();

    let filtered = filtered_display_models(vec![
        "gpt-5.4".to_string(),
        "claude-opus-4-8".to_string(),
        "claude-sonnet-4-6".to_string(),
        "gpt-5.5".to_string(),
        "gpt-5.6-sol".to_string(),
        "claude-fable-5".to_string(),
    ]);

    // Every curated hosted model is shown, while unknown router models remain hidden.
    assert_eq!(
        filtered,
        vec![
            "claude-opus-4-8".to_string(),
            "claude-sonnet-4-6".to_string(),
            "gpt-5.5".to_string(),
            "gpt-5.6-sol".to_string(),
            "claude-fable-5".to_string(),
        ]
    );

    crate::env::set_var(crate::subscription_catalog::JCODE_TIER_ENV, "ultra");
    let filtered = filtered_display_models(vec![
        "claude-fable-5".to_string(),
        "gpt-5.6-sol".to_string(),
        "gpt-5.4".to_string(),
    ]);
    assert_eq!(
        filtered,
        vec!["claude-fable-5".to_string(), "gpt-5.6-sol".to_string()]
    );

    crate::env::remove_var(crate::subscription_catalog::JCODE_TIER_ENV);
    crate::env::remove_var("JCODE_HOME");
    crate::subscription_catalog::clear_runtime_env();
}

#[test]
fn test_remote_jcode_subscription_fallback_keeps_managed_route_identity() {
    let models = vec![
        "claude-opus-4-8".to_string(),
        "claude-sonnet-4-6".to_string(),
        "gpt-5.5".to_string(),
        "gpt-5.6-sol".to_string(),
    ];
    let routes = remote_model_routes_fallback(
        Some(crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME),
        &models,
    );

    assert_eq!(
        routes
            .iter()
            .map(|route| route.model.as_str())
            .collect::<Vec<_>>(),
        vec![
            "claude-opus-4-8",
            "claude-sonnet-4-6",
            "gpt-5.5",
            "gpt-5.6-sol",
        ]
    );
    assert!(routes.iter().all(|route| {
        route.provider == crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME
            && route.api_method == crate::subscription_catalog::JCODE_ROUTE_API_METHOD
            && route.available
    }));
}

#[test]
fn test_subscription_filters_do_not_activate_from_saved_credentials_alone() {
    let _guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::env::set_var(crate::subscription_catalog::JCODE_API_KEY_ENV, "test-key");

    assert!(ensure_model_allowed_for_subscription("gpt-5.4").is_ok());
    assert_eq!(
        filtered_display_models(vec!["gpt-5.4".to_string(), "claude-opus-4-8".to_string(),]),
        vec!["gpt-5.4".to_string(), "claude-opus-4-8".to_string()]
    );

    crate::env::remove_var(crate::subscription_catalog::JCODE_API_KEY_ENV);
    crate::subscription_catalog::clear_runtime_env();
}

#[test]
fn test_anthropic_catalog_scopes_isolate_routes_accounts_and_api_keys() {
    with_clean_provider_test_env(|| {
        crate::env::set_var("ANTHROPIC_API_KEY", "catalog-key-a");
        crate::auth::claude::set_active_account_override(Some("catalog-oauth-a".into()));
        let api = anthropic_catalog_scope_for_route(false);
        let oauth = anthropic_catalog_scope_for_route(true);
        assert_ne!(api, oauth);
        assert!(!api.contains("catalog-key-a"));
        populate_anthropic_models_for_scope(&api, vec!["claude-api-exclusive".into()]);
        populate_anthropic_models_for_scope(&oauth, vec!["claude-oauth-exclusive".into()]);
        assert_eq!(
            cached_anthropic_model_ids_for_scope(&api).unwrap(),
            vec!["claude-api-exclusive"]
        );
        assert_eq!(
            cached_anthropic_model_ids_for_scope(&oauth).unwrap(),
            vec!["claude-oauth-exclusive"]
        );
        assert!(!anthropic_oauth_route_availability("claude-api-exclusive").0);
        assert!(!anthropic_api_key_route_availability("claude-oauth-exclusive").0);
        assert!(anthropic_api_key_route_availability("claude-api-exclusive").0);
        assert!(anthropic_oauth_route_availability("claude-oauth-exclusive").0);

        crate::env::set_var("ANTHROPIC_API_KEY", "catalog-key-b");
        assert_ne!(anthropic_catalog_scope_for_route(false), api);
        assert!(
            cached_anthropic_model_ids_for_scope(&anthropic_catalog_scope_for_route(false))
                .is_none()
        );
        assert_eq!(anthropic_catalog_scope_for_route(true), oauth);
        crate::auth::claude::set_active_account_override(Some("catalog-oauth-b".into()));
        assert_ne!(anthropic_catalog_scope_for_route(true), oauth);
        crate::env::set_var("ANTHROPIC_AUTH_TOKEN", "catalog-auth-token");
        let token_scope = anthropic_catalog_scope_for_route(false);
        crate::env::set_var("ANTHROPIC_API_KEY", "catalog-key-c");
        assert_eq!(anthropic_catalog_scope_for_route(false), token_scope);
    });
}

#[test]
fn test_anthropic_catalog_refresh_scopes_retry_and_empty_retention() {
    with_clean_provider_test_env(|| {
        let api = "api-key::refresh-test";
        let oauth = "oauth::refresh-test";
        assert!(begin_anthropic_model_catalog_refresh_for_scope(api));
        assert!(!begin_anthropic_model_catalog_refresh_for_scope(api));
        assert!(begin_anthropic_model_catalog_refresh_for_scope(oauth));
        finish_anthropic_model_catalog_refresh_for_scope(api);
        // A failed fetch releases in-flight but remains retry-throttled.
        assert!(!begin_anthropic_model_catalog_refresh_for_scope(api));
        populate_anthropic_models_for_scope(oauth, vec!["claude-cached-success".into()]);
        finish_anthropic_model_catalog_refresh_for_scope(oauth);
        assert!(!should_refresh_anthropic_model_catalog_for_scope(oauth));
        // Empty or failed discovery must not erase a previously useful snapshot.
        populate_anthropic_models_for_scope(oauth, Vec::new());
        assert_eq!(
            cached_anthropic_model_ids_for_scope(oauth).unwrap(),
            vec!["claude-cached-success"]
        );
        assert!(begin_anthropic_model_catalog_refresh_for_scope(
            "oauth::different-account"
        ));
    });
}

#[test]
fn test_anthropic_catalog_scoped_persistence_ttl_and_models_updated() {
    with_clean_provider_test_env(|| {
        let api = "api-key::disk-scoped-test";
        let oauth = "oauth::disk-scoped-test";
        let catalog = |model: &str| AnthropicModelCatalog {
            available_models: vec![model.into()],
            context_limits: Default::default(),
        };
        persist_anthropic_model_catalog_for_scope(api, &catalog("claude-disk-api"));
        persist_anthropic_model_catalog_for_scope(oauth, &catalog("claude-disk-oauth"));
        assert_eq!(
            cached_anthropic_model_ids_for_scope(api).unwrap(),
            vec!["claude-disk-api"]
        );
        assert_eq!(
            cached_anthropic_model_ids_for_scope(oauth).unwrap(),
            vec!["claude-disk-oauth"]
        );
        assert!(!begin_anthropic_model_catalog_refresh_for_scope(api));
        assert!(!begin_anthropic_model_catalog_refresh_for_scope(oauth));

        // Backdate only API's persisted observation beyond the 30-minute TTL.
        let path = crate::storage::app_config_dir()
            .unwrap()
            .join("anthropic_model_catalog_cache.json");
        let mut store: serde_json::Value = crate::storage::read_json(&path).unwrap();
        store["scopes"][api]["observed_at_unix_secs"] = serde_json::json!(1);
        crate::storage::write_json(&path, &store).unwrap();
        models::reset_model_catalog_services_for_tests();
        assert!(begin_anthropic_model_catalog_refresh_for_scope(api));
        assert!(!begin_anthropic_model_catalog_refresh_for_scope(oauth));
        assert_eq!(
            cached_anthropic_model_ids_for_scope(api).unwrap(),
            vec!["claude-disk-api"]
        );
        finish_anthropic_model_catalog_refresh_for_scope(api);

        crate::bus::reset_models_updated_publish_state_for_tests();
        let mut events = crate::bus::Bus::global().subscribe();
        populate_anthropic_models_for_scope(api, vec!["claude-newly-discovered".into()]);
        assert!(matches!(
            events.try_recv(),
            Ok(crate::bus::BusEvent::ModelsUpdated)
        ));
        assert_eq!(
            cached_anthropic_model_ids_for_scope(oauth).unwrap(),
            vec!["claude-disk-oauth"]
        );
    });
}

#[test]
fn test_anthropic_simplified_routes_are_api_first_and_scope_aware() {
    with_clean_provider_test_env(|| {
        let mut auth = crate::auth::AuthStatus::default();
        let mut routes = Vec::new();
        append_simplified_anthropic_model_routes(&mut routes, "claude-future", &auth);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].api_method, "claude-api");
        assert_eq!(routes[0].detail, "no API key");
        assert_eq!(routes[1].detail, "no Claude login");
        assert!(routes.iter().all(|route| !route.available));
        auth.anthropic.has_api_key = true;
        auth.anthropic.has_oauth = true;
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(false),
            vec!["claude-future".into()],
        );
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(true),
            vec!["claude-other".into()],
        );
        routes.clear();
        append_simplified_anthropic_model_routes(&mut routes, "claude-future", &auth);
        assert!(routes[0].available);
        assert!(!routes[1].available);
        assert_eq!(routes[1].detail, "not in OAuth model catalog");
    });
}

#[test]
fn test_anthropic_api_long_context_not_gated_by_oauth_extra_usage() {
    with_clean_provider_test_env(|| {
        assert_eq!(
            anthropic_api_key_route_availability("claude-opus-4-6[1m]"),
            (true, String::new())
        );
    });
}

#[test]
fn test_anthropic_full_routes_use_each_routes_own_catalog() {
    with_clean_provider_test_env(|| {
        let provider = test_multi_provider_with_cursor();
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(false),
            vec!["claude-api-only".into()],
        );
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(true),
            vec!["claude-oauth-only".into()],
        );
        let mut routes = Vec::new();
        catalog_routes::append_anthropic_routes(&provider, &mut routes, true, true);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].model, "claude-api-only");
        assert_eq!(routes[0].api_method, "claude-api");
        assert_eq!(routes[1].model, "claude-oauth-only");
        assert_eq!(routes[1].api_method, "claude-oauth");
        assert!(routes.iter().all(|route| route.available));
    });
}

#[test]
fn test_anthropic_api_discovery_does_not_advertise_unverified_oauth_model() {
    with_clean_provider_test_env(|| {
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(false),
            vec!["claude-future-api-only".into()],
        );
        assert!(anthropic_api_key_route_availability("claude-future-api-only").0);
        // No OAuth snapshot at all is not permission to borrow API IDs.
        assert!(!anthropic_oauth_route_availability("claude-future-api-only").0);
    });
}

fn configure_catalog_pro_account() {
    let label = "claude-otter";
    let mut auth = crate::auth::claude::JcodeAuthFile::default();
    auth.anthropic_accounts = vec![crate::auth::claude::AnthropicAccount {
        label: label.into(),
        access: "catalog-test-access".into(),
        refresh: "catalog-test-refresh".into(),
        expires: 4_102_444_800_000,
        email: None,
        subscription_type: Some("pro".into()),
        scopes: vec![],
    }];
    auth.active_anthropic_account = Some(label.into());
    crate::auth::claude::save_auth_file(&auth).unwrap();
    crate::auth::claude::set_active_account_override(Some(label.into()));
    assert!(!crate::auth::claude::is_max_subscription());
}

#[test]
fn test_anthropic_pro_explicit_oauth_catalog_wins_in_both_pickers() {
    with_clean_provider_test_env(|| {
        configure_catalog_pro_account();
        let model = "claude-opus-5-5";
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(true),
            vec![model.into()],
        );
        let mut auth = crate::auth::AuthStatus::default();
        auth.anthropic.has_oauth = true;
        let mut simplified = Vec::new();
        append_simplified_anthropic_model_routes(&mut simplified, model, &auth);
        let oauth = simplified
            .iter()
            .find(|r| r.api_method == "claude-oauth")
            .unwrap();
        assert!(oauth.available);
        assert!(oauth.detail.is_empty());
        let provider = test_multi_provider_with_cursor();
        let mut full = Vec::new();
        catalog_routes::append_anthropic_routes(&provider, &mut full, true, false);
        assert!(
            full.iter()
                .any(|r| r.model == model && r.api_method == "claude-oauth" && r.available)
        );
    });
}

#[test]
fn test_anthropic_pro_api_catalog_cannot_override_oauth_absence() {
    with_clean_provider_test_env(|| {
        configure_catalog_pro_account();
        let model = "claude-opus-5-5";
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(false),
            vec![model.into()],
        );
        // API discovery must not bypass the no-OAuth-cache legacy heuristic.
        assert_eq!(
            anthropic_oauth_route_availability(model),
            (false, "requires Max subscription".into())
        );
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(true),
            vec!["claude-sonnet-4-6".into()],
        );
        assert_eq!(
            anthropic_oauth_route_availability(model),
            (false, "not in OAuth model catalog".into())
        );
        assert!(anthropic_api_key_route_availability(model).0);
        let mut auth = crate::auth::AuthStatus::default();
        auth.anthropic.has_api_key = true;
        auth.anthropic.has_oauth = true;
        let mut routes = Vec::new();
        append_simplified_anthropic_model_routes(&mut routes, model, &auth);
        assert!(
            routes
                .iter()
                .any(|r| r.api_method == "claude-api" && r.available)
        );
        assert!(
            routes
                .iter()
                .any(|r| r.api_method == "claude-oauth" && !r.available)
        );
        let provider = test_multi_provider_with_cursor();
        routes.clear();
        catalog_routes::append_anthropic_routes(&provider, &mut routes, true, true);
        assert!(
            !routes
                .iter()
                .any(|r| r.model == model && r.api_method == "claude-oauth")
        );
    });
}

#[test]
fn test_anthropic_discovered_oauth_model_still_requires_login_in_both_pickers() {
    with_clean_provider_test_env(|| {
        let model = "claude-opus-5-5";
        populate_anthropic_models_for_scope(
            &anthropic_catalog_scope_for_route(true),
            vec![model.into()],
        );
        let mut routes = Vec::new();
        append_simplified_anthropic_model_routes(
            &mut routes,
            model,
            &crate::auth::AuthStatus::default(),
        );
        let oauth = routes
            .iter()
            .find(|r| r.api_method == "claude-oauth")
            .unwrap();
        assert!(!oauth.available);
        assert_eq!(oauth.detail, "no Claude login");
        let provider = test_multi_provider_with_cursor();
        routes.clear();
        catalog_routes::append_anthropic_routes(&provider, &mut routes, false, false);
        let oauth = routes
            .iter()
            .find(|r| r.model == model && r.api_method == "claude-oauth")
            .unwrap();
        assert!(!oauth.available);
        assert_eq!(oauth.detail, "no Claude login");
    });
}

#[test]
fn test_anthropic_pro_no_catalog_and_explicit_long_context_keep_legacy_gates() {
    with_clean_provider_test_env(|| {
        configure_catalog_pro_account();
        let scope = anthropic_catalog_scope_for_route(true);
        assert!(cached_anthropic_model_ids_for_scope(&scope).is_none());
        assert_eq!(
            anthropic_oauth_route_availability("claude-opus-5-5"),
            (false, "requires Max subscription".into())
        );
        let model = "claude-opus-4-6[1m]";
        populate_anthropic_models_for_scope(&scope, vec![model.into()]);
        let expected = if crate::usage::has_extra_usage() {
            (true, String::new())
        } else {
            (false, "requires extra usage".into())
        };
        assert_eq!(anthropic_oauth_route_availability(model), expected);
    });
}
