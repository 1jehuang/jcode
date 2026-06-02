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
