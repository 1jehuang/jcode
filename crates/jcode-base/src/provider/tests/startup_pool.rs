use super::*;

#[test]
fn startup_pool_tiers_override_last_account_then_fall_back_when_exhausted() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let mut pool = startup_pool_fixture();
        // Last-used work login is first in the old saved order, but lower tier.
        pool.members = vec!["openai:openai-fox".into(), "openai:openai-otter".into()];
        pool.set_priority("openai:openai-fox", 1);
        pool.set_priority("openai:openai-otter", 0);
        crate::config::Config::set_default_model(
            Some("openai-oauth@openai-fox:gpt-5.5"),
            Some("openai-oauth"),
        )
        .unwrap();
        pool.save().unwrap();
        let router = MultiProvider::new_fast();
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        assert_eq!(router.model(), "gpt-5.5");
        models::record_provider_unavailable_for_account_label(
            "openai",
            Some("openai-otter"),
            "quota exhausted",
        );
        assert_eq!(
            router
                .fork_for_new_session()
                .oauth_account_label()
                .as_deref(),
            Some("openai-fox")
        );
        models::clear_openai_provider_unavailability_for_account_label(Some("openai-otter"));
        assert_eq!(
            router
                .fork_for_new_session()
                .oauth_account_label()
                .as_deref(),
            Some("openai-otter")
        );
        assert_eq!(
            auth::codex::active_account_label().as_deref(),
            Some("openai-fox")
        );
        assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
    });
}

#[test]
fn startup_pool_ready_priority_head_wins_over_saved_provider_default() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let mut pool = startup_pool_fixture();
        let label = auth::claude::upsert_account(auth::claude::AnthropicAccount {
            label: "claude-fox".into(),
            access: "access".into(),
            refresh: "refresh".into(),
            expires: chrono::Utc::now().timestamp_millis() + 86_400_000,
            email: None,
            subscription_type: Some("max".into()),
            scopes: vec![],
        })
        .unwrap();
        pool.members.insert(0, format!("claude:{label}"));
        pool.save().unwrap();
        crate::config::Config::set_default_model(
            Some("openai-oauth:gpt-5.5"),
            Some("openai-oauth"),
        )
        .unwrap();
        pool.save().unwrap();
        models::clear_provider_unavailable_for_account("claude");
        let router = MultiProvider::new_fast();
        assert_eq!(router.active_provider(), ActiveProvider::Claude);
        assert!(router.model().starts_with("claude"));
        assert_eq!(router.credential_mode(), CredentialMode::OAuth);
        assert_eq!(router.fork().model(), router.model());
        let fresh = router.fork_for_new_session();
        assert_eq!(fresh.oauth_account_label().as_deref(), Some(label.as_str()));
        assert!(fresh.model().starts_with("claude"));
        assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
    });
}

#[test]
fn startup_pool_saved_account_does_not_pin_new_sessions() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let pool = startup_pool_fixture();
        crate::config::Config::set_default_model(
            Some("openai-oauth@openai-fox:gpt-5.5"),
            Some("openai-oauth"),
        )
        .unwrap();
        // Keep the configured account behind the pool head intentionally.
        pool.save().unwrap();
        let config_before = crate::config::Config::load().provider;
        let router = MultiProvider::new_fast();
        for provider in [router.fork(), router.fork_for_new_session()] {
            assert_eq!(provider.model(), "gpt-5.5");
            assert_eq!(
                provider.oauth_account_label().as_deref(),
                Some("openai-otter")
            );
            assert_eq!(provider.credential_mode(), CredentialMode::OAuth);
        }
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
        let config_after = crate::config::Config::load().provider;
        assert_eq!(config_after.default_model, config_before.default_model);
        assert_eq!(
            config_after.default_provider,
            config_before.default_provider
        );
    });
}

#[test]
fn startup_pool_head_binds_when_no_config_default_exists() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let mut pool = startup_pool_fixture();
        crate::config::Config::set_default_model(None, None).unwrap();
        let label = auth::claude::upsert_account(auth::claude::AnthropicAccount {
            label: "claude-fox".into(),
            access: "access".into(),
            refresh: "refresh".into(),
            expires: chrono::Utc::now().timestamp_millis() + 86_400_000,
            email: None,
            subscription_type: Some("max".into()),
            scopes: vec![],
        })
        .unwrap();
        pool.members.insert(0, format!("claude:{label}"));
        pool.save().unwrap();
        models::clear_provider_unavailable_for_account("claude");
        let router = MultiProvider::new_fast();
        assert_eq!(router.active_provider(), ActiveProvider::Claude);
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some(label.as_str())
        );
        assert!(router.model().starts_with("claude"));
        models::record_provider_unavailable_for_account_label(
            "claude",
            Some(&label),
            "quota exhausted",
        );
        let fresh = router.fork_for_new_session();
        assert_eq!(fresh.oauth_account_label().as_deref(), Some("openai-otter"));
        auth::claude::set_active_account_override(Some(label.clone()));
        models::clear_provider_unavailable_for_account("claude");
        let reset = router.fork_for_new_session();
        assert_eq!(reset.oauth_account_label().as_deref(), Some(label.as_str()));
        assert!(reset.model().starts_with("claude"));
        assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
    });
}

#[test]
fn startup_pool_head_binds_when_config_default_fails_to_apply() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let pool = startup_pool_fixture();
        crate::config::Config::set_default_model(
            Some("no-such-provider:no-such-model"),
            Some("no-such-provider"),
        )
        .unwrap();
        let router = MultiProvider::new_fast();
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        assert_eq!(router.credential_mode(), CredentialMode::OAuth);
        assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
    });
}

fn startup_pool_fixture() -> crate::auth::account_pool::AccountPool {
    crate::env::set_var("OPENAI_API_KEY", "sk-openai-panda");
    for label in ["openai-otter", "openai-fox", "openai-panda"] {
        auth::codex::upsert_account_from_tokens(label, "access", "refresh", None, None).unwrap();
        models::clear_openai_provider_unavailability_for_account_label(Some(label));
    }
    auth::codex::set_active_account("openai-fox").unwrap();
    crate::config::Config::set_default_model(Some("gpt-5.5"), Some("openai-api")).unwrap();
    let pool = crate::auth::account_pool::AccountPool {
        members: vec!["openai:openai-otter".into(), "openai:openai-fox".into()],
        excluded: vec!["openai:openai-panda".into()],
        ..Default::default()
    };
    pool.save().unwrap();
    pool
}

#[test]
fn startup_pool_all_exhausted_never_falls_through_to_manual_api_default() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let pool = startup_pool_fixture();
        crate::config::Config::set_default_model(None, None).unwrap();
        for label in ["openai-otter", "openai-fox"] {
            models::record_provider_unavailable_for_account_label(
                "openai",
                Some(label),
                "quota exhausted",
            );
        }
        let router = MultiProvider::new_fast();
        assert_eq!(router.credential_mode(), CredentialMode::OAuth);
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
        for label in ["openai-otter", "openai-fox"] {
            models::clear_openai_provider_unavailability_for_account_label(Some(label));
        }
    });
}

#[test]
fn startup_pool_priority_skips_cooldown_and_returns_after_reset_without_saving_fallback() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let pool = startup_pool_fixture();
        crate::config::Config::set_default_model(None, None).unwrap();
        let config_before = crate::config::Config::load().provider;
        let router = MultiProvider::new_fast();
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        router.set_session_id("existing-session");
        models::record_provider_unavailable_for_account_label(
            "openai",
            Some("openai-otter"),
            "quota exhausted",
        );
        let fresh = router.fork_for_new_session();
        assert_eq!(fresh.oauth_account_label().as_deref(), Some("openai-fox"));
        assert_eq!(
            router.fork().oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        models::clear_openai_provider_unavailability_for_account_label(Some("openai-otter"));
        assert_eq!(
            router
                .fork_for_new_session()
                .oauth_account_label()
                .as_deref(),
            Some("openai-otter")
        );
        assert_eq!(
            auth::codex::active_account_label().as_deref(),
            Some("openai-fox")
        );
        assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
        let after = crate::config::Config::load().provider;
        assert_eq!(after.default_provider, config_before.default_provider);
        assert_eq!(after.default_model, config_before.default_model);
    });
}

#[test]
fn startup_pool_explicit_initial_provider_and_model_remain_authoritative() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        startup_pool_fixture();
        crate::env::set_var("JCODE_INITIAL_PROVIDER_EXPLICIT", "1");
        crate::env::set_var("JCODE_ACTIVE_PROVIDER", "openai");
        let router = MultiProvider::new_fast();
        router.set_model("openai-api:gpt-5.4").unwrap();
        let fresh = router.fork_for_new_session();
        assert_eq!(fresh.model(), "gpt-5.4");
        assert_eq!(fresh.credential_mode(), CredentialMode::ApiKey);
        assert_eq!(fresh.oauth_account_label(), None);
    });
}

#[test]
fn startup_pool_never_uses_excluded_or_unenrolled_metered_keys() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let mut pool = startup_pool_fixture();
        crate::config::Config::set_default_model(None, None).unwrap();
        // Conflicting persisted entries still must not authorize excluded billing.
        pool.members.insert(0, "openai:openai-panda".into());
        pool.save().unwrap();
        let router = MultiProvider::new_fast();
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        assert_eq!(router.credential_mode(), CredentialMode::OAuth);
    });
}
