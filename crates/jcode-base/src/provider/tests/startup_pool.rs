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

#[test]
fn startup_pool_api_default_spellings_preserve_credential_pins() {
    for (model, hint) in [
        ("openai-api:gpt-5.5", Some("openai")),
        ("gpt-5.5", Some("openai-api")),
        ("gpt-5.5", Some("openai-api-key")),
        ("claude-api:claude-opus-4-6", Some("claude")),
        ("claude-opus-4-6", Some("anthropic-api-key")),
    ] {
        with_clean_provider_test_env(|| {
            let runtime = enter_test_runtime();
            let _guard = runtime.enter();
            let pool = startup_pool_fixture();
            crate::env::set_var("ANTHROPIC_API_KEY", "sk-anthropic-test");
            crate::config::Config::set_default_model(Some(model), hint).unwrap();
            pool.save().unwrap();
            let before = crate::config::Config::load().provider;
            let router = MultiProvider::new_fast();
            for provider in [router.fork(), router.fork_for_new_session()] {
                assert_eq!(
                    provider.credential_mode(),
                    CredentialMode::ApiKey,
                    "{model} {hint:?}"
                );
                assert_eq!(provider.oauth_account_label(), None);
                assert_eq!(provider.model(), router.model());
            }
            assert_eq!(router.credential_mode(), CredentialMode::ApiKey);
            assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
            let after = crate::config::Config::load().provider;
            assert_eq!(after.default_model, before.default_model);
            assert_eq!(after.default_provider, before.default_provider);
        });
    }
}

#[test]
fn startup_pool_named_defaults_are_preserved_but_unused_profiles_do_not_shadow_native() {
    for (name, kind, model, hint) in [
        ("gateway", "openai-compatible", "gateway:gpt-5.5", None),
        ("openai", "openai-compatible", "gpt-5.5", Some("openai")),
        (
            "gateway",
            "anthropic-compatible",
            "gateway:claude-opus-4-6",
            None,
        ),
        (
            "claude",
            "anthropic-compatible",
            "claude-opus-4-6",
            Some("claude"),
        ),
        ("openai", "openai-compatible", "gpt-5.5", None),
    ] {
        with_clean_provider_test_env(|| {
            let runtime = enter_test_runtime();
            let _guard = runtime.enter();
            let pool = startup_pool_fixture();
            let path = std::path::PathBuf::from(std::env::var_os("JCODE_HOME").unwrap())
                .join("config.toml");
            std::fs::write(
                &path,
                format!(
                    r#"
[providers.{name}]
type = "{kind}"
base_url = "https://gateway.example.com/v1"
auth = "none"
[[providers.{name}.models]]
id = "gpt-5.5"
[[providers.{name}.models]]
id = "claude-opus-4-6"
"#
                ),
            )
            .unwrap();
            crate::config::invalidate_config_cache();
            crate::config::Config::set_default_model(Some(model), hint).unwrap();
            // Restore pool order after saving a deliberate default.
            pool.save().unwrap();
            let router = MultiProvider::new_fast();
            let native = name == "openai" && hint.is_none();
            for provider in [router.fork(), router.fork_for_new_session()] {
                assert_eq!(provider.model(), router.model());
                assert_eq!(
                    provider.oauth_account_label().as_deref(),
                    native.then_some("openai-otter"),
                    "{name} {hint:?}"
                );
            }
            if native {
                assert_eq!(router.active_provider(), ActiveProvider::OpenAI);
                assert_eq!(router.credential_mode(), CredentialMode::OAuth);
            } else if kind == "openai-compatible" {
                assert_eq!(
                    ProviderRegistry::new(&router)
                        .active_compatible_profile_id()
                        .as_deref(),
                    Some(name)
                );
            } else {
                assert_eq!(router.active_provider(), ActiveProvider::Claude);
                assert_eq!(std::env::var("JCODE_NAMED_PROVIDER_PROFILE").unwrap(), name);
            }
            assert_eq!(crate::auth::account_pool::AccountPool::load(), pool);
        });
    }
}

#[test]
fn startup_pool_bare_default_rotates_ready_accounts_without_changing_saved_login() {
    for hint in [None, Some("OpenAI"), Some(" openai "), Some("openai-oauth")] {
        with_clean_provider_test_env(|| {
            let runtime = enter_test_runtime();
            let _guard = runtime.enter();
            let pool = startup_pool_fixture();
            crate::config::Config::set_default_model(Some("gpt-5.5"), hint).unwrap();
            pool.save().unwrap();
            let before = crate::config::Config::load().provider;
            let router = MultiProvider::new_fast();
            assert_eq!(
                router.oauth_account_label().as_deref(),
                Some("openai-otter")
            );
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
            let after = crate::config::Config::load().provider;
            assert_eq!(after.default_model, before.default_model);
            assert_eq!(after.default_provider, before.default_provider);
        });
    }
}

#[test]
fn startup_pool_excluded_oauth_default_is_not_a_pool_identity() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        let pool = startup_pool_fixture();
        crate::config::Config::set_default_model(
            Some("openai-oauth@openai-panda:gpt-5.5"),
            Some("openai-oauth"),
        )
        .unwrap();
        pool.save().unwrap();
        let router = MultiProvider::new_fast();
        assert_eq!(
            router.oauth_account_label().as_deref(),
            Some("openai-panda")
        );
        assert_eq!(
            router
                .fork_for_new_session()
                .oauth_account_label()
                .as_deref(),
            Some("openai-panda")
        );
    });
}

#[test]
fn startup_pool_claude_default_distinguishes_api_only_auto_from_oauth_alias() {
    for oauth in [false, true] {
        with_clean_provider_test_env(|| {
            let runtime = enter_test_runtime();
            let _guard = runtime.enter();
            let mut pool = startup_pool_fixture();
            crate::env::set_var("ANTHROPIC_API_KEY", "sk-anthropic-test");
            if oauth {
                let label = auth::claude::upsert_account(auth::claude::AnthropicAccount {
                    label: "claude-test".into(),
                    access: "access".into(),
                    refresh: "refresh".into(),
                    expires: chrono::Utc::now().timestamp_millis() + 86_400_000,
                    email: None,
                    subscription_type: Some("max".into()),
                    scopes: vec![],
                })
                .unwrap();
                pool.members.push(format!("claude:{label}"));
                pool.set_priority(&format!("claude:{label}"), 1);
            }
            pool.set_priority("openai:openai-otter", 0);
            crate::config::Config::set_default_model(
                Some("claude-opus-4-6"),
                Some(if oauth { "anthropic-oauth" } else { "claude" }),
            )
            .unwrap();
            pool.save().unwrap();
            let router = MultiProvider::new_fast();
            for provider in [router.fork(), router.fork_for_new_session()] {
                if oauth {
                    assert_eq!(
                        provider.oauth_account_label().as_deref(),
                        Some("openai-otter")
                    );
                } else {
                    assert_eq!(provider.name(), "Claude");
                    assert_eq!(provider.model(), "claude-opus-4-6");
                    assert_eq!(
                        provider.active_resolved_credential(),
                        Some(jcode_provider_core::ResolvedCredential::ApiKey)
                    );
                }
            }
        });
    }
}

#[test]
fn startup_pool_compatible_aliases_rotate_only_enrolled_saved_logins() {
    for hint in [
        "openai-compatible",
        "compat",
        "custom",
        "openai_compatible",
        " Compat ",
    ] {
        for enrolled in [true, false] {
            with_clean_provider_test_env(|| {
                let runtime = enter_test_runtime();
                let _guard = runtime.enter();
                let mut pool = startup_pool_fixture();
                crate::env::set_var("OPENAI_COMPAT_API_KEY", "sk-compatible-test");
                crate::config::Config::set_default_model(Some("gpt-5.5"), Some(hint)).unwrap();
                if enrolled {
                    pool.members.push("openai-compatible".into());
                } else {
                    pool.excluded.push("openai-compatible".into());
                }
                pool.save().unwrap();
                let router = MultiProvider::new_fast();
                assert_eq!(
                    router.oauth_account_label().as_deref(),
                    enrolled.then_some("openai-otter"),
                    "{hint}"
                );
                assert_eq!(
                    router
                        .fork_for_new_session()
                        .oauth_account_label()
                        .as_deref(),
                    enrolled.then_some("openai-otter"),
                    "{hint}"
                );
                if !enrolled {
                    assert_eq!(
                        ProviderRegistry::new(&router)
                            .active_compatible_profile_id()
                            .as_deref(),
                        Some("openai-compatible")
                    );
                }
            });
        }
    }
}
