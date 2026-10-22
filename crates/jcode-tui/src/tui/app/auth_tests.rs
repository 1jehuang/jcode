use super::{
    App, antigravity_input_requires_state_validation, save_tui_openai_compatible_api_base,
    save_tui_openai_compatible_key,
};

fn with_temp_jcode_home<T>(f: impl FnOnce() -> T) -> T {
    let _env_guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let saved_env = [
        "JCODE_HOME",
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_SETUP_URL",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "OPENAI_COMPAT_API_KEY",
        // Ambient named-profile / OpenRouter runtime env leaks across tests
        // and flips `openai_compatible_profile_is_configured` into the
        // named-profile path; clear it so each test starts hermetic.
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_TRANSPORT_STATE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_PROVIDER",
        "JCODE_OPENROUTER_NO_FALLBACK",
        "JCODE_API_BASE",
        "JCODE_API_KEY",
    ]
    .map(|key| (key, std::env::var_os(key)));

    crate::env::set_var("JCODE_HOME", temp.path());
    for (key, _) in saved_env.iter().skip(1) {
        crate::env::remove_var(key);
    }

    let result = f();

    for (key, value) in saved_env {
        if let Some(value) = value {
            crate::env::set_var(key, value);
        } else {
            crate::env::remove_var(key);
        }
    }
    result
}

#[test]
fn antigravity_auto_callback_code_skips_manual_callback_parser() {
    assert!(!antigravity_input_requires_state_validation(
        "raw_authorization_code",
        Some("expected_state")
    ));
}

#[test]
fn antigravity_manual_callback_url_keeps_state_validation() {
    assert!(antigravity_input_requires_state_validation(
        "http://127.0.0.1:51121/oauth-callback?code=abc&state=expected_state",
        Some("expected_state")
    ));
}

#[test]
fn oauth_preflight_mentions_browser_fallback_and_doctor() {
    let message = App::record_oauth_preflight("openai", false, Some("localhost:1455"), Some(true));
    assert!(message.contains("could not open a browser"));
    assert!(message.contains("auth doctor openai"));
}

#[test]
fn oauth_preflight_mentions_manual_safe_callback_mode() {
    let message = App::record_oauth_preflight(
        "gemini",
        true,
        Some("http://127.0.0.1:0/oauth2callback"),
        Some(false),
    );
    assert!(message.contains("manual-safe paste completion"));
    assert!(message.contains("oauth2callback"));
}

#[test]
fn tui_openai_compatible_api_base_accepts_localhost_override() -> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        let resolved = save_tui_openai_compatible_api_base("http://localhost:11434/v1")?;
        assert_eq!(resolved.api_base, "http://localhost:11434/v1");
        assert!(!resolved.requires_api_key);
        Ok(())
    })
}

#[test]
fn tui_openai_compatible_api_base_keeps_jcode_docs_and_remote_endpoint() -> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        let resolved = save_tui_openai_compatible_api_base("https://api.deepseek.com/")?;
        assert_eq!(resolved.api_base, "https://api.deepseek.com");
        assert!(resolved.requires_api_key);
        assert!(resolved.setup_url.contains("github.com/1jehuang/jcode"));
        assert!(!resolved.setup_url.contains("opencode.ai"));
        Ok(())
    })
}

#[test]
fn tui_openai_compatible_key_save_persists_key_for_current_session() -> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        let resolved = save_tui_openai_compatible_api_base("https://api.example.com/v1")?;
        let resolved = save_tui_openai_compatible_key(
            crate::provider_catalog::OPENAI_COMPAT_PROFILE,
            " sk-test-tui-login ",
        )
        .map(|_| resolved)?;

        assert!(
            crate::provider_catalog::openai_compatible_profile_is_configured(
                crate::provider_catalog::OPENAI_COMPAT_PROFILE,
            )
        );
        assert_eq!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                &resolved.api_key_env,
                &resolved.env_file,
            )
            .as_deref(),
            Some("sk-test-tui-login")
        );
        assert!(
            std::env::var_os(&resolved.api_key_env).is_none(),
            "saving must not poison the process env (issue #1386)"
        );
        Ok(())
    })
}

#[test]
fn tui_api_key_logout_clears_saved_key_and_process_env() -> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        let resolved = save_tui_openai_compatible_api_base("https://api.example.com/v1")?;
        let resolved = save_tui_openai_compatible_key(
            crate::provider_catalog::OPENAI_COMPAT_PROFILE,
            " sk-test-tui-login ",
        )
        .map(|_| resolved)?;

        // Saving is file-only (#1386), so the key is readable from the env file
        // and never copied into the process environment.
        assert_eq!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                &resolved.api_key_env,
                &resolved.env_file,
            )
            .as_deref(),
            Some("sk-test-tui-login")
        );
        assert!(std::env::var_os(&resolved.api_key_env).is_none());

        App::clear_api_key_login(&resolved.api_key_env, &resolved.env_file)?;

        assert!(std::env::var_os(&resolved.api_key_env).is_none());
        assert!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                &resolved.api_key_env,
                &resolved.env_file,
            )
            .is_none()
        );
        Ok(())
    })
}

#[test]
fn tui_jcode_subscription_logout_clears_credentials_and_preserves_api_base() -> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        crate::provider_catalog::save_env_value_to_env_file(
            crate::subscription_catalog::JCODE_API_KEY_ENV,
            crate::subscription_catalog::JCODE_ENV_FILE,
            Some("test-jcode-key"),
        )?;
        crate::provider_catalog::save_env_value_to_env_file(
            crate::subscription_catalog::JCODE_API_BASE_ENV,
            crate::subscription_catalog::JCODE_ENV_FILE,
            Some("https://subscription.example/v1"),
        )?;
        crate::provider_catalog::save_env_value_to_env_file(
            crate::subscription_catalog::JCODE_ACCOUNT_ID_ENV,
            crate::subscription_catalog::JCODE_ENV_FILE,
            Some("acct_test"),
        )?;
        crate::provider_catalog::save_env_value_to_env_file(
            crate::subscription_catalog::JCODE_ACCOUNT_EMAIL_ENV,
            crate::subscription_catalog::JCODE_ENV_FILE,
            Some("user@example.com"),
        )?;

        crate::subscription_catalog::clear_account_credentials()?;

        assert!(std::env::var_os(crate::subscription_catalog::JCODE_API_KEY_ENV).is_none());
        assert_eq!(
            std::env::var(crate::subscription_catalog::JCODE_API_BASE_ENV).as_deref(),
            Ok("https://subscription.example/v1")
        );
        assert!(std::env::var_os(crate::subscription_catalog::JCODE_ACCOUNT_ID_ENV).is_none());
        assert!(std::env::var_os(crate::subscription_catalog::JCODE_ACCOUNT_EMAIL_ENV).is_none());
        assert!(crate::subscription_catalog::configured_api_key().is_none());
        for env_key in [
            crate::subscription_catalog::JCODE_ACCOUNT_ID_ENV,
            crate::subscription_catalog::JCODE_ACCOUNT_EMAIL_ENV,
        ] {
            assert!(
                crate::provider_catalog::load_env_value_from_env_or_config(
                    env_key,
                    crate::subscription_catalog::JCODE_ENV_FILE,
                )
                .is_none()
            );
        }
        assert_eq!(
            crate::subscription_catalog::configured_api_base().as_deref(),
            Some("https://subscription.example/v1")
        );
        Ok(())
    })
}

#[test]
fn tui_openai_compatible_local_key_save_allows_empty_key() -> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        let resolved = save_tui_openai_compatible_key(crate::provider_catalog::OLLAMA_PROFILE, "")?;
        assert_eq!(resolved.api_base, "http://localhost:11434/v1");
        assert!(
            crate::provider_catalog::openai_compatible_profile_is_configured(
                crate::provider_catalog::OLLAMA_PROFILE
            )
        );
        assert!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                &resolved.api_key_env,
                &resolved.env_file,
            )
            .is_none()
        );
        Ok(())
    })
}

#[test]
fn muse_subscription_pending_holds_attempt_cancel_slot() {
    use super::auth_types::PendingLogin;

    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let approved = std::sync::Arc::new(std::sync::Mutex::new(None));
    let pending = PendingLogin::MuseSubscription {
        attempt: "muse-subscription#abc123".to_string(),
        cancelled: std::sync::Arc::clone(&cancelled),
        approved: std::sync::Arc::clone(&approved),
    };
    // Telemetry still attributes the device-code method.
    assert_eq!(
        pending.telemetry_context(),
        Some(("muse-subscription".to_string(), "device_code".to_string()))
    );
    // /cancel flips the shared flag and drops the slot: the background task
    // observes the flag before every poll and before handoff, and the handler
    // takes credentials from the slot only for a matching attempt.
    if let PendingLogin::MuseSubscription {
        cancelled,
        approved,
        ..
    } = &pending
    {
        cancelled.store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut slot) = approved.lock() {
            *slot = None;
        }
    }
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    assert!(approved.lock().map(|slot| slot.is_none()).unwrap_or(false));
}

#[test]
fn muse_subscription_code_progress_renders_without_success_side_effects() {
    let _env_guard = crate::storage::lock_test_env();
    let mut app = App::new_for_remote(None);
    app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
        attempt: "muse-subscription#abc123".to_string(),
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        approved: std::sync::Arc::new(std::sync::Mutex::new(None)),
    });
    assert!(app.recent_authenticated_provider.is_none());

    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse_subscription_code#abc123".to_string(),
        success: true,
        message: "Muse Subscription Login\n\nYour code: ABCD-1234\n\nWaiting...".to_string(),
    });

    // Pending approval must not mark a completed login, trigger catalog
    // refresh, or record auth success.
    assert!(app.recent_authenticated_provider.is_none());
    assert!(!app.auth_catalog_refresh_pending);
    assert_eq!(
        app.status_notice
            .as_ref()
            .map(|(notice, _)| notice.as_str()),
        Some("Login: enter ABCD-1234 at Meta")
    );
}

#[test]
fn muse_subscription_stale_events_after_cancel_are_ignored() {
    let _env_guard = crate::storage::lock_test_env();
    let mut app = App::new_for_remote(None);

    // A stale progress event for a superseded attempt renders nothing.
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse_subscription_code#old999".to_string(),
        success: true,
        message: "Muse Subscription Login\n\nYour code: STALE-0000\n\nWaiting...".to_string(),
    });
    assert!(app.recent_authenticated_provider.is_none());
    assert!(app.status_notice.is_none());

    // A stale approval event with no matching pending attempt saves nothing
    // and switches no provider.
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse-subscription#old999".to_string(),
        success: true,
        message: "Muse subscription approved in the browser; finalizing...".to_string(),
    });
    assert!(app.recent_authenticated_provider.is_none());
    assert!(!app.auth_catalog_refresh_pending);

    // A cancelled pending attempt rejects even its own approval event.
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
        attempt: "muse-subscription#cancelled1".to_string(),
        cancelled,
        approved: std::sync::Arc::new(std::sync::Mutex::new(None)),
    });
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse-subscription#cancelled1".to_string(),
        success: true,
        message: "Muse subscription approved in the browser; finalizing...".to_string(),
    });
    assert!(app.recent_authenticated_provider.is_none());
    assert!(!app.auth_catalog_refresh_pending);
}

#[test]
fn muse_subscription_canonical_success_publishes_untagged_event_and_updates_local_state()
-> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        let prev_opt_in = std::env::var(crate::auth::muse::EXPERIMENTAL_ENV_VAR).ok();
        crate::env::set_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR, "1");
        struct RestoreOptIn(Option<String>);
        impl Drop for RestoreOptIn {
            fn drop(&mut self) {
                if let Some(val) = &self.0 {
                    crate::env::set_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR, val);
                } else {
                    crate::env::remove_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR);
                }
            }
        }
        let _restore = RestoreOptIn(prev_opt_in);

        // This test exercises returning-user login state. A fresh install
        // intentionally replaces the ready notice with onboarding guidance.
        let mut hints = crate::setup_hints::SetupHintsState::load();
        hints.onboarding_start_choice_shown = true;
        hints.save()?;
        let mut app = App::new_for_remote(None);
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let approved = std::sync::Arc::new(std::sync::Mutex::new(None));
        let attempt = "attempt_succ_123";

        // Subscribe to Bus before the approval is processed
        let mut bus_rx = crate::bus::Bus::global().subscribe();

        app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
            attempt: attempt.to_string(),
            cancelled: std::sync::Arc::clone(&cancelled),
            approved: std::sync::Arc::clone(&approved),
        });

        // Place approved credentials in the slot
        *approved.lock().unwrap() = Some(crate::auth::muse::MuseCredentials {
            api_key: "test_key_123".to_string(),
            access_token: "test_token_123".to_string(),
            base_url: crate::auth::muse::EXPECTED_API_BASE_URL.to_string(),
            user_email: Some("user@example.com".to_string()),
            user_full_name: Some("Test User".to_string()),
            is_subs_active: true,
            minted_at_ms: 1000,
            subs_usage: None,
        });

        // Handle the attempt-scoped approval event
        app.handle_login_completed(crate::bus::LoginCompleted {
            provider: format!("muse-subscription#{attempt}"),
            success: true,
            message: "Muse subscription approved in the browser; finalizing...".to_string(),
        });

        // Credentials must be saved to disk
        assert!(crate::auth::muse::load_credentials().is_ok());

        // Exactly one canonical untagged event must be published
        let mut canonical_event = None;
        while let Ok(event) = bus_rx.try_recv() {
            if let crate::bus::BusEvent::LoginCompleted(login) = event
                && login.provider == "muse-subscription"
            {
                canonical_event = Some(login);
                break;
            }
        }
        let canonical = canonical_event.expect("canonical untagged event must be published");
        assert!(canonical.success);
        assert_eq!(canonical.provider, "muse-subscription");

        // Verify remote classification helper:
        // is_completed_login is true for canonical event, false for attempt-scoped event
        assert!(super::auth_muse_helpers::is_completed_login(&canonical));
        assert!(!super::auth_muse_helpers::is_completed_login(
            &crate::bus::LoginCompleted {
                provider: format!("muse-subscription#{attempt}"),
                success: true,
                message: "tagged".to_string(),
            }
        ));

        // Now dispatch the canonical event into handle_login_completed
        app.handle_login_completed(canonical);

        // Local state must now be fully updated
        assert_eq!(
            app.recent_authenticated_provider
                .as_ref()
                .map(|(p, _)| p.as_str()),
            Some("muse-subscription")
        );
        assert!(app.auth_catalog_refresh_pending);
        assert_eq!(
            app.status_notice.as_ref().map(|(n, _)| n.as_str()),
            Some("Login: muse-subscription ready")
        );
        assert!(app.pending_login.is_none());
        Ok(())
    })
}

#[test]
fn muse_subscription_failed_attempt_surfaces_message_and_clears_pending() {
    let _env_guard = crate::storage::lock_test_env();
    let mut app = App::new_for_remote(None);
    let attempt = "attempt_fail_456";
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let approved = std::sync::Arc::new(std::sync::Mutex::new(None));

    app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
        attempt: attempt.to_string(),
        cancelled,
        approved,
    });

    // Handle a failed attempt event (denial, expiry, or network failure)
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: format!("muse-subscription#{attempt}"),
        success: false,
        message: "Muse subscription login was denied or cancelled in the browser.".to_string(),
    });

    // The failure message must be surfaced to the user
    let error_displayed = app.display_messages().iter().any(|m| {
        m.content
            .contains("Muse subscription login was denied or cancelled in the browser.")
    });
    assert!(error_displayed, "error message must be displayed to user");

    // Status notice must indicate failure rather than remaining stuck waiting
    assert_eq!(
        app.status_notice.as_ref().map(|(n, _)| n.as_str()),
        Some("Login: muse-subscription failed")
    );

    // Pending login must be cleared
    assert!(app.pending_login.is_none());
    assert!(app.recent_authenticated_provider.is_none());
}

#[test]
fn muse_subscription_stale_and_cancelled_progress_and_failure_are_ignored() {
    let _env_guard = crate::storage::lock_test_env();
    let mut app = App::new_for_remote(None);
    let active_attempt = "active_attempt_789";
    let cancelled_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let approved = std::sync::Arc::new(std::sync::Mutex::new(None));

    app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
        attempt: active_attempt.to_string(),
        cancelled: std::sync::Arc::clone(&cancelled_flag),
        approved: std::sync::Arc::clone(&approved),
    });
    app.set_status_notice("Login: active waiting");

    // 1. Stale progress from an old attempt is ignored
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse_subscription_code#old_stale_attempt".to_string(),
        success: true,
        message: "Muse Subscription Login\n\nYour code: OLD-CODE\n\nWaiting...".to_string(),
    });
    assert_eq!(
        app.status_notice.as_ref().map(|(n, _)| n.as_str()),
        Some("Login: active waiting")
    );
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("OLD-CODE"))
    );

    // 2. Stale failure from an old attempt is ignored
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse-subscription#old_stale_attempt".to_string(),
        success: false,
        message: "Old attempt failed".to_string(),
    });
    assert!(app.pending_login.is_some());
    assert_eq!(
        app.status_notice.as_ref().map(|(n, _)| n.as_str()),
        Some("Login: active waiting")
    );

    // 3. Mark the active attempt cancelled
    cancelled_flag.store(true, std::sync::atomic::Ordering::Release);

    // Queued progress for the cancelled attempt must be ignored
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: format!("muse_subscription_code#{active_attempt}"),
        success: true,
        message: "Muse Subscription Login\n\nYour code: CANCELLED-CODE\n\nWaiting...".to_string(),
    });
    assert_eq!(
        app.status_notice.as_ref().map(|(n, _)| n.as_str()),
        Some("Login: active waiting")
    );
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("CANCELLED-CODE"))
    );

    // Queued failure for the cancelled attempt must be ignored
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: format!("muse-subscription#{active_attempt}"),
        success: false,
        message: "Cancelled attempt failure".to_string(),
    });
    assert_eq!(
        app.status_notice.as_ref().map(|(n, _)| n.as_str()),
        Some("Login: active waiting")
    );
}

#[test]
fn muse_subscription_accumulating_backoff_increases_interval() {
    use super::auth_muse_helpers::{next_slow_down_interval, pending_attempt_matches};

    // Test attempt matching helper with and without prefix
    assert!(pending_attempt_matches("muse-subscription#abc", "abc"));
    assert!(pending_attempt_matches("abc", "muse-subscription#abc"));
    assert!(pending_attempt_matches(
        "muse-subscription#abc",
        "muse-subscription#abc"
    ));
    assert!(pending_attempt_matches("abc", "abc"));
    assert!(!pending_attempt_matches("abc", "def"));
    assert!(!pending_attempt_matches(
        "muse-subscription#abc",
        "muse-subscription#def"
    ));

    let initial_interval = 5u64;

    // First slow_down response with retry_after = 10
    let interval_1 = next_slow_down_interval(initial_interval, 10);
    assert_eq!(interval_1, 10);

    // Second consecutive slow_down: carrying forward interval_1 (10), next retry_after = 15
    let interval_2 = next_slow_down_interval(interval_1, 15);
    assert_eq!(interval_2, 15);

    // Third slow_down: server returns a non-increasing retry_after (e.g. 5)
    // RFC 8628 requires that each slow_down increases the interval by at least 5 seconds.
    let interval_3 = next_slow_down_interval(interval_2, 5);
    assert!(interval_3 >= 20);
    assert_eq!(interval_3, 20);

    // Fourth slow_down: server returns retry_after = 0
    let interval_4 = next_slow_down_interval(interval_3, 0);
    assert!(interval_4 >= 25);
    assert_eq!(interval_4, 25);
}

#[test]
fn muse_subscription_old_success_arriving_after_new_attempt_leaves_new_pending_intact()
-> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        let prev_opt_in = std::env::var(crate::auth::muse::EXPERIMENTAL_ENV_VAR).ok();
        crate::env::set_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR, "1");
        struct RestoreOptIn(Option<String>);
        impl Drop for RestoreOptIn {
            fn drop(&mut self) {
                if let Some(val) = &self.0 {
                    crate::env::set_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR, val);
                } else {
                    crate::env::remove_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR);
                }
            }
        }
        let _restore = RestoreOptIn(prev_opt_in);

        let mut app = App::new_for_remote(None);
        let attempt_1 = "attempt_old_111";
        let cancelled_1 = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let approved_1 = std::sync::Arc::new(std::sync::Mutex::new(None));

        let mut bus_rx = crate::bus::Bus::global().subscribe();

        app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
            attempt: attempt_1.to_string(),
            cancelled: std::sync::Arc::clone(&cancelled_1),
            approved: std::sync::Arc::clone(&approved_1),
        });

        // Place approved credentials in the slot for attempt 1
        *approved_1.lock().unwrap() = Some(crate::auth::muse::MuseCredentials {
            api_key: "old_key_111".to_string(),
            access_token: "old_token_111".to_string(),
            base_url: crate::auth::muse::EXPECTED_API_BASE_URL.to_string(),
            user_email: Some("old_user@example.com".to_string()),
            user_full_name: Some("Old User".to_string()),
            is_subs_active: true,
            minted_at_ms: 1000,
            subs_usage: None,
        });

        // Process attempt 1 approval: saves credentials, clears attempt 1, publishes canonical event
        app.handle_login_completed(crate::bus::LoginCompleted {
            provider: format!("muse-subscription#{attempt_1}"),
            success: true,
            message: "Muse subscription approved in the browser; finalizing...".to_string(),
        });

        // Credentials must be saved
        assert!(crate::auth::muse::load_credentials().is_ok());

        // Canonical untagged event must be published for the daemon
        let mut canonical_event = None;
        while let Ok(event) = bus_rx.try_recv() {
            if let crate::bus::BusEvent::LoginCompleted(login) = event
                && login.provider == "muse-subscription"
            {
                canonical_event = Some(login);
                break;
            }
        }
        let canonical = canonical_event.expect("canonical untagged event must be published");
        assert!(canonical.success);
        assert_eq!(canonical.provider, "muse-subscription");

        // Now user starts a new attempt 2 while canonical event is queued
        let attempt_2 = "attempt_new_222";
        let cancelled_2 = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let approved_2 = std::sync::Arc::new(std::sync::Mutex::new(None));
        app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
            attempt: attempt_2.to_string(),
            cancelled: std::sync::Arc::clone(&cancelled_2),
            approved: std::sync::Arc::clone(&approved_2),
        });

        // Stale attempt-scoped approval from an old attempt arriving while attempt 2 is pending
        // must also be ignored and leave attempt 2 intact
        app.handle_login_completed(crate::bus::LoginCompleted {
            provider: "muse-subscription#attempt_stale_999".to_string(),
            success: true,
            message: "Stale attempt approval".to_string(),
        });
        match &app.pending_login {
            Some(super::auth_types::PendingLogin::MuseSubscription { attempt, .. }) => {
                assert_eq!(attempt, attempt_2);
            }
            _ => panic!("attempt 2 pending login must remain intact"),
        }

        // Canonical event from attempt 1 arrives and is handled
        app.handle_login_completed(canonical);

        // Attempt 2's pending login must remain intact
        match &app.pending_login {
            Some(super::auth_types::PendingLogin::MuseSubscription { attempt, .. }) => {
                assert_eq!(attempt, attempt_2);
            }
            _ => panic!("attempt 2 pending login must remain intact"),
        }

        Ok(())
    })
}

#[test]
fn muse_subscription_old_failure_does_not_clear_newer_pending_attempt() {
    let _env_guard = crate::storage::lock_test_env();
    let mut app = App::new_for_remote(None);

    let active_attempt = "attempt_active_333";
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let approved = std::sync::Arc::new(std::sync::Mutex::new(None));
    app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
        attempt: active_attempt.to_string(),
        cancelled,
        approved,
    });
    app.set_status_notice("Login: active waiting");

    // 1. Old attempt's attempt-scoped failure event arrives
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse-subscription#attempt_old_000".to_string(),
        success: false,
        message: "Old attempt failed".to_string(),
    });

    // Active attempt pending login must remain intact
    match &app.pending_login {
        Some(super::auth_types::PendingLogin::MuseSubscription { attempt, .. }) => {
            assert_eq!(attempt, active_attempt);
        }
        _ => panic!("active attempt pending login must remain intact"),
    }
    assert_eq!(
        app.status_notice.as_ref().map(|(n, _)| n.as_str()),
        Some("Login: active waiting")
    );

    // 2. An untagged failure event must also not clear the active attempt
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse-subscription".to_string(),
        success: false,
        message: "Generic failure".to_string(),
    });

    match &app.pending_login {
        Some(super::auth_types::PendingLogin::MuseSubscription { attempt, .. }) => {
            assert_eq!(attempt, active_attempt);
        }
        _ => panic!("active attempt pending login must remain intact"),
    }
}

/// An approval that lands while the user is still typing or pasting the device
/// code must not leave that text armed as the next model prompt. The generic
/// same-provider cleanup in `handle_login_completed` is skipped for Muse
/// because Muse pending state is attempt-owned, so the equivalent has to run
/// when the matching attempt finishes.
#[test]
fn muse_subscription_approval_clears_leftover_login_input_for_the_finished_attempt()
-> anyhow::Result<()> {
    with_temp_jcode_home(|| {
        let prev_opt_in = std::env::var(crate::auth::muse::EXPERIMENTAL_ENV_VAR).ok();
        crate::env::set_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR, "1");
        struct RestoreOptIn(Option<String>);
        impl Drop for RestoreOptIn {
            fn drop(&mut self) {
                if let Some(val) = &self.0 {
                    crate::env::set_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR, val);
                } else {
                    crate::env::remove_var(crate::auth::muse::EXPERIMENTAL_ENV_VAR);
                }
            }
        }
        let _restore = RestoreOptIn(prev_opt_in);

        let mut app = App::new_for_remote(None);
        let attempt = "attempt_input_clear_1";
        let approved = std::sync::Arc::new(std::sync::Mutex::new(Some(
            crate::auth::muse::MuseCredentials {
                api_key: "input_clear_key".to_string(),
                access_token: "input_clear_token".to_string(),
                base_url: crate::auth::muse::EXPECTED_API_BASE_URL.to_string(),
                user_email: None,
                user_full_name: None,
                is_subs_active: true,
                minted_at_ms: 1000,
                subs_usage: None,
            },
        )));
        app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
            attempt: attempt.to_string(),
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            approved,
        });
        // Text the user typed or pasted while the browser step was pending.
        app.input = "ABCD-1234".to_string();
        app.cursor_pos = app.input.len();
        app.pasted_contents = vec!["pasted device code blob".to_string()];

        app.handle_login_completed(crate::bus::LoginCompleted {
            provider: format!("muse-subscription#{attempt}"),
            success: true,
            message: "Muse subscription approved in the browser; finalizing...".to_string(),
        });

        assert!(
            app.input.is_empty(),
            "leftover login text must not stay armed, got {:?}",
            app.input
        );
        assert_eq!(app.cursor_pos, 0);
        assert!(
            app.pasted_contents.is_empty(),
            "pasted login contents must be released"
        );
        assert!(app.pending_login.is_none());
        Ok(())
    })
}

/// The complement of the test above: finishing an old attempt must not disturb
/// text the user is typing for a newer login.
#[test]
fn muse_subscription_old_attempt_event_does_not_clear_newer_login_input() {
    let _env_guard = crate::storage::lock_test_env();
    let mut app = App::new_for_remote(None);
    let newer_attempt = "attempt_newer_input_2";
    app.pending_login = Some(super::auth_types::PendingLogin::MuseSubscription {
        attempt: newer_attempt.to_string(),
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        approved: std::sync::Arc::new(std::sync::Mutex::new(None)),
    });
    app.input = "code for the new attempt".to_string();
    app.cursor_pos = app.input.len();
    app.pasted_contents = vec!["new attempt paste".to_string()];

    // Canonical (untagged) success from the previous attempt.
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "muse-subscription".to_string(),
        success: true,
        message: "previous attempt finished".to_string(),
    });

    assert_eq!(app.input, "code for the new attempt");
    assert_eq!(app.pasted_contents.len(), 1);
    match &app.pending_login {
        Some(super::auth_types::PendingLogin::MuseSubscription { attempt, .. }) => {
            assert_eq!(attempt, newer_attempt);
        }
        _ => panic!("newer attempt's pending login must remain intact"),
    }
}
