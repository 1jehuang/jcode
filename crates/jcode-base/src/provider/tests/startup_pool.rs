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
fn startup_pool_non_account_config_default_survives_pool_head() {
    with_clean_provider_test_env(|| {
        let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
        std::fs::write(
            std::path::PathBuf::from(jcode_home).join("config.toml"),
            r#"
[providers.llmg-coding]
type = "openai-compatible"
base_url = "https://example.com/proxy/openai"
auth = "none"
default_model = "together_ai/revolut-ltd/large-1"

[[providers.llmg-coding.models]]
id = "together_ai/revolut-ltd/large-1"
input = ["text"]
"#,
        )
        .expect("write test config.toml");
        crate::config::invalidate_config_cache();
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        startup_pool_fixture();
        crate::config::Config::set_default_model(
            Some("together_ai/revolut-ltd/large-1"),
            Some("llmg-coding"),
        )
        .unwrap();
        let config_before = crate::config::Config::load().provider;
        let router = MultiProvider::new_fast();
        // The OAuth pool head (openai-otter on the newest OpenAI flagship) is
        // ready, but the configured gateway default must stay bound: it is not
        // a billing identity the pool may rotate.
        assert_eq!(
            ProviderRegistry::new(&router).active_compatible_profile_id(),
            Some("llmg-coding".to_string())
        );
        assert_eq!(router.model(), "together_ai/revolut-ltd/large-1");
        assert_eq!(router.oauth_account_label(), None);
        let fresh = router.fork_for_new_session();
        assert_eq!(fresh.model(), "together_ai/revolut-ltd/large-1");
        assert_eq!(fresh.oauth_account_label(), None);
        let config_after = crate::config::Config::load().provider;
        assert_eq!(config_after.default_model, config_before.default_model);
        assert_eq!(
            config_after.default_provider,
            config_before.default_provider
        );
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

#[test]
fn startup_pool_bare_default_still_binds_pool_head() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let mut pool = startup_pool_fixture();
        // Bare model, no provider hint: a saved default is a preference within
        // the selected account, so the ready priority-0 head must still win over
        // the last-used priority-1 login.
        pool.members = vec!["openai:openai-fox".into(), "openai:openai-otter".into()];
        pool.set_priority("openai:openai-fox", 1);
        pool.set_priority("openai:openai-otter", 0);
        crate::config::Config::set_default_model(Some("gpt-5.5"), None).unwrap();
        pool.save().unwrap();
        let router = MultiProvider::new_fast();
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        assert_eq!(router.model(), "gpt-5.5");
        assert_eq!(
            router
                .fork_for_new_session()
                .oauth_account_label()
                .as_deref(),
            Some("openai-otter")
        );
    });
}

#[test]
fn startup_pool_explicit_api_prefix_survives_generic_provider_hint() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        startup_pool_fixture();
        // An explicit credential prefix pins the route; the generic provider
        // hint must not let the pool swap the API-key identity for OAuth.
        crate::config::Config::set_default_model(Some("openai-api:gpt-5.5"), Some("openai"))
            .unwrap();
        let config_before = crate::config::Config::load().provider;
        let router = MultiProvider::new_fast();
        assert_eq!(router.credential_mode(), CredentialMode::ApiKey);
        assert_eq!(router.oauth_account_label(), None);
        assert_eq!(router.model(), "gpt-5.5");
        let fresh = router.fork_for_new_session();
        assert_eq!(fresh.credential_mode(), CredentialMode::ApiKey);
        assert_eq!(fresh.oauth_account_label(), None);
        let config_after = crate::config::Config::load().provider;
        assert_eq!(config_after.default_model, config_before.default_model);
        assert_eq!(
            config_after.default_provider,
            config_before.default_provider
        );
    });
}

#[test]
fn startup_pool_generic_compat_hint_with_pool_member_still_binds_pool_head() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let mut pool = startup_pool_fixture();
        // Reproduces the regression: `default_provider = "openai-compatible"`
        // is the generic saved-login bucket. When the pool owns that member,
        // pool selection must keep deciding the route instead of pinning the
        // static profile (api.openai.com + OPENAI_COMPAT_API_KEY), which 401s
        // for a gateway-saved login.
        pool.members = vec!["openai-compatible".into(), "openai:openai-otter".into()];
        pool.save().unwrap();
        crate::env::set_var("OPENAI_COMPAT_API_KEY", "sk-compat-gateway");
        crate::config::Config::set_default_model(
            Some("together_ai/revolut-ltd/large-1"),
            Some("openai-compatible"),
        )
        .unwrap();
        let config_before = crate::config::Config::load().provider;
        let router = MultiProvider::new_fast();
        // Pool selection ran: the ready OAuth head is bound, not the compat
        // static profile.
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-otter")
        );
        let fresh = router.fork_for_new_session();
        assert_eq!(fresh.oauth_account_label().as_deref(), Some("openai-otter"));
        let config_after = crate::config::Config::load().provider;
        assert_eq!(config_after.default_model, config_before.default_model);
        assert_eq!(
            config_after.default_provider,
            config_before.default_provider
        );
    });
}

#[test]
fn startup_pool_classifier_accepts_route_aliases_and_case_variants() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        startup_pool_fixture();
        // The hint must classify the same way config routing resolves it:
        // accepted aliases (`anthropic-oauth`) and mixed-case (`OpenAI`) bind
        // the pool's OAuth logins, while explicit API-key hints stay pinned.
        let router = MultiProvider::new_fast();
        for hint in ["anthropic-oauth", "OpenAI", " openai "] {
            assert!(
                MultiProvider::config_default_binds_oauth_account(&router, "gpt-5.5", Some(hint)),
                "hint '{hint}' should classify as pool-owned"
            );
        }
        for hint in [
            "openai-api",
            "anthropic-api",
            "claude-api",
            "openrouter",
            "bedrock",
            "gemini",
        ] {
            assert!(
                !MultiProvider::config_default_binds_oauth_account(&router, "gpt-5.5", Some(hint)),
                "hint '{hint}' should stay a deliberate routing choice"
            );
        }
        // Generic bucket without pool membership stays a deliberate choice.
        assert!(!MultiProvider::config_default_binds_oauth_account(
            &router,
            "gpt-5.5",
            Some("nvidia-nim")
        ));
    });
}

#[test]
fn startup_pool_classifier_resolves_compatible_aliases_through_config_routing() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let mut pool = startup_pool_fixture();
        // The generic saved-login bucket is reachable through every spelling
        // config routing accepts (`openai-compatible`, `compat`, `custom`,
        // `openai_compatible`); each must classify identically. When the pool
        // owns the member, all of them keep pool selection active instead of
        // pinning the static profile.
        pool.members = vec!["openai-compatible".into(), "openai:openai-otter".into()];
        pool.save().unwrap();
        let router = MultiProvider::new_fast();
        for hint in [
            "openai-compatible",
            "compat",
            "custom",
            "openai_compatible",
            " Compat ",
        ] {
            assert!(
                MultiProvider::config_default_binds_oauth_account(&router, "gpt-5.5", Some(hint)),
                "compatible alias '{hint}' should classify as pool-owned when the pool has the member"
            );
        }
        // An excluded compat member stays a deliberate choice: pool ownership
        // is the default for the saved-login bucket, so exclusion is the
        // negative case.
        pool.excluded = vec!["openai-compatible".into()];
        pool.save().unwrap();
        for hint in ["openai-compatible", "compat", "custom"] {
            assert!(
                !MultiProvider::config_default_binds_oauth_account(&router, "gpt-5.5", Some(hint)),
                "compatible alias '{hint}' excluded from the pool should stay a deliberate choice"
            );
        }
    });
}

#[test]
fn startup_pool_named_anthropic_gateway_default_survives_pool_head() {
    with_clean_provider_test_env(|| {
        let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
        std::fs::write(
            std::path::PathBuf::from(jcode_home).join("config.toml"),
            r#"
[providers.gateway]
type = "anthropic-compatible"
base_url = "https://gateway.example.com/anthropic/v1"
auth = "bearer"
api_key_env = "GATEWAY_CLAUDE_TOKEN"
default_model = "claude-fable-5"

[[providers.gateway.models]]
id = "claude-fable-5"
context_window = 200000
"#,
        )
        .expect("write test config.toml");
        crate::config::invalidate_config_cache();
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        startup_pool_fixture();
        // Named profile spec with no provider hint: the config default names a
        // user-defined gateway, not a billing identity, so the ready OAuth pool
        // head must not replace it on startup or on fresh-session forks.
        crate::config::Config::set_default_model(Some("gateway:claude-fable-5"), None).unwrap();
        let config_before = crate::config::Config::load().provider;
        let router = MultiProvider::new_fast();
        assert_eq!(
            std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
                .ok()
                .as_deref(),
            Some("gateway")
        );
        assert_eq!(router.active_provider(), ActiveProvider::Claude);
        assert_eq!(router.model(), "claude-fable-5");
        assert_eq!(router.oauth_account_label(), None);
        let fresh = router.fork_for_new_session();
        assert_eq!(
            std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
                .ok()
                .as_deref(),
            Some("gateway")
        );
        assert_eq!(fresh.model(), "claude-fable-5");
        assert_eq!(fresh.oauth_account_label(), None);
        let config_after = crate::config::Config::load().provider;
        assert_eq!(config_after.default_model, config_before.default_model);
        assert_eq!(
            config_after.default_provider,
            config_before.default_provider
        );
    });
}

#[test]
fn startup_pool_classifier_treats_named_profile_spec_as_explicit() {
    with_clean_provider_test_env(|| {
        let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
        std::fs::write(
            std::path::PathBuf::from(jcode_home).join("config.toml"),
            r#"
[providers.gateway]
type = "anthropic-compatible"
base_url = "https://gateway.example.com/anthropic/v1"
auth = "bearer"
api_key_env = "GATEWAY_CLAUDE_TOKEN"
default_model = "claude-fable-5"
"#,
        )
        .expect("write test config.toml");
        crate::config::invalidate_config_cache();
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        startup_pool_fixture();
        let router = MultiProvider::new_fast();
        // The named-profile spec is explicit even with no provider hint; a
        // generic `claude` hint must not reclassify it as pool-owned either.
        assert!(!MultiProvider::config_default_binds_oauth_account(
            &router,
            "gateway:claude-fable-5",
            None
        ));
        assert!(!MultiProvider::config_default_binds_oauth_account(
            &router,
            "gateway:claude-fable-5",
            Some("claude")
        ));
        // Unknown prefix without a matching config profile keeps the hint
        // semantics (bare-model classification via the hint).
        assert!(MultiProvider::config_default_binds_oauth_account(
            &router,
            "gpt-5.5",
            Some("claude")
        ));
    });
}

#[test]
fn startup_pool_classifier_keeps_named_profile_priority_over_builtin_names() {
    with_clean_provider_test_env(|| {
        let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
        let config_path = std::path::PathBuf::from(&jcode_home).join("config.toml");
        // A profile colliding with a built-in provider name (`openai`) must
        // keep the same precedence config routing uses: the named gateway is
        // a deliberate routing choice, not the pool-owned built-in OAuth
        // slot, so pool selection must not replace it (review finding).
        std::fs::write(
            &config_path,
            r#"
[providers.openai]
type = "openai-compatible"
base_url = "https://gateway.example.com/v1"
auth = "bearer"
api_key_env = "GATEWAY_OPENAI_TOKEN"
default_model = "openai-otter-pro"

[[providers.openai.models]]
id = "openai-otter-pro"
context_window = 200000
"#,
        )
        .expect("write test config.toml");
        crate::config::invalidate_config_cache();
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        startup_pool_fixture();
        let router = MultiProvider::new_fast();
        // The colliding `openai` hint resolves to the named profile, a
        // deliberate routing choice the pool must not replace. A `claude`
        // hint without a colliding profile keeps its built-in meaning.
        assert!(!MultiProvider::config_default_binds_oauth_account(
            &router,
            "gpt-5.5",
            Some("openai")
        ));
        assert!(MultiProvider::config_default_binds_oauth_account(
            &router,
            "gpt-5.5",
            Some("claude")
        ));
        // Without a colliding profile the `openai` hint is the built-in
        // pool-owned OAuth slot again.
        std::fs::write(&config_path, "").expect("reset test config.toml");
        crate::config::invalidate_config_cache();
        assert!(MultiProvider::config_default_binds_oauth_account(
            &router,
            "gpt-5.5",
            Some("openai")
        ));
    });
}
