#[test]
fn rotation_skips_exhausted_and_wraps() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let now = chrono::Utc::now().timestamp();
        let rt = enter_test_runtime();

        // X on otter runs out: otter is recorded exhausted, X moves to fox.
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: now + 3600,
            },
        );
        let x = account_session(&world, "X", ActiveProvider::Claude, false);
        x.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-otter", None)),
        )
        .unwrap();
        assert_eq!(complete_text(&rt, &x).unwrap(), "hi from claude-fox");
        assert_eq!(world.labels_for("X"), ["claude-otter", "claude-fox"]);

        // Y on panda (the last account) runs out. The rotation wraps to the
        // start (otter), skips otter because it is known to be exhausted
        // until its reset, and lands on fox. Otter gets no doomed request.
        world.set("claude-otter", FakeAccountBehavior::Ok);
        world.set(
            "claude-panda",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: now + 7200,
            },
        );
        let y = account_session(&world, "Y", ActiveProvider::Claude, false);
        y.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-panda", None)),
        )
        .unwrap();
        assert_eq!(complete_text(&rt, &y).unwrap(), "hi from claude-fox");
        assert_eq!(world.labels_for("Y"), ["claude-panda", "claude-fox"]);
    });
}

#[test]
fn all_exhausted_error_names_earliest_reset_and_falls_to_cross_provider_prompt() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let now = chrono::Utc::now().timestamp();
        let fox_reset = now + 3600 + 12 * 60;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: now + 3 * 3600,
            },
        );
        world.set(
            "claude-fox",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: fox_reset,
            },
        );
        world.set(
            "claude-panda",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: now + 2 * 3600,
            },
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, true);

        let rt = enter_test_runtime();
        let err = complete_text(&rt, &a).expect_err("every Claude account is out");
        // This low-level helper does not accept a cross-provider route.
        // The production countdown tests retain that approval boundary.
        let reason = err.to_string();
        assert!(
            reason.contains("All 3 Claude accounts are out of usage."),
            "{}",
            reason
        );
        assert!(
            reason.contains(&format!("First reset: claude-fox at {}", clock(fox_reset))),
            "{}",
            reason
        );
        assert_eq!(
            world.labels_for("A"),
            ["claude-otter", "claude-fox", "claude-panda"],
            "each account is tried once, nothing is sent to OpenAI"
        );
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude),
            None,
            "the original (unpinned) state is restored"
        );
        assert!(
            a.account_failover_home(AccountProviderKind::Claude)
                .is_none()
        );
    });
}

#[test]
fn approved_account_switch_records_home_only_after_success() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        let world = Arc::new(FakeAccountWorld::default());
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        let b = account_session(&world, "B", ActiveProvider::Claude, false);
        assert!(
            a.approved_account_switch_origin(ActiveProvider::Claude)
                .is_none()
        );
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::Claude,
            "claude-otter",
            Some(chrono::Utc::now().timestamp() + 3600),
        );
        let origin = a.approved_account_switch_origin(ActiveProvider::Claude);
        assert!(origin.is_some());
        assert!(
            a.account_failover_home(AccountProviderKind::Claude)
                .is_none()
        );
        assert!(a.drain_startup_notices().is_empty());
        assert!(world.labels_for("A").is_empty());
        a.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-fox", None)),
        )
        .unwrap();
        a.record_approved_account_switch(origin, "claude-fox");
        assert_eq!(
            a.account_failover_home(AccountProviderKind::Claude)
                .unwrap()
                .label,
            "claude-otter"
        );
        assert_eq!(a.drain_startup_notices().len(), 1);
        assert!(
            b.account_failover_home(AccountProviderKind::Claude)
                .is_none()
        );
        assert!(b.drain_startup_notices().is_empty());
        assert_eq!(
            crate::auth::claude::active_account_label().as_deref(),
            Some("claude-otter")
        );
    });
}

#[test]
fn returns_home_after_reset_at_turn_start() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        let world = Arc::new(FakeAccountWorld::default());
        let resets_at = chrono::Utc::now().timestamp() + 2;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit { resets_at },
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        let rt = enter_test_runtime();
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");

        // Before the reset: stay on fox.
        assert!(a.return_account_home_if_reset().is_empty());
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-fox".to_string())
        );

        std::thread::sleep(std::time::Duration::from_millis(2_200));
        world.set("claude-otter", FakeAccountBehavior::Ok);
        assert_eq!(
            a.return_account_home_if_reset(),
            vec![AccountProviderKind::Claude]
        );
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-otter".to_string())
        );
        assert!(
            a.account_failover_home(AccountProviderKind::Claude)
                .is_none()
        );
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-otter");

        // Config off: never returns on its own.
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: chrono::Utc::now().timestamp() + 1,
            },
        );
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");
        write_provider_config("account_failover_return_home = false");
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        assert!(a.return_account_home_if_reset().is_empty());
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-fox".to_string())
        );
    });
}

#[test]
fn session_failover_off_disables_rotation() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let resets_at = chrono::Utc::now().timestamp() + 3600;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit { resets_at },
        );
        let rt = enter_test_runtime();

        // This session turned failover off: the usage-limit error reaches the
        // consumer unchanged and no other account is tried.
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        a.set_account_failover(Some(false));
        let err = complete_text(&rt, &a).expect_err("failover is off for this session");
        assert!(err.to_string().contains("429"), "{err:#}");
        assert_eq!(world.labels_for("A"), ["claude-otter"]);
        assert_eq!(
            a.account_pin(AccountProviderKind::Claude)
                .map(|pin| pin.label),
            Some("claude-otter".into()),
            "request-start binding must not rotate when this window disables failover"
        );

        // Another session with the config default (on) still moves.
        let b = account_session(&world, "B", ActiveProvider::Claude, false);
        assert_eq!(complete_text(&rt, &b).unwrap(), "hi from claude-fox");

        // Session on, config off: the session value wins.
        write_provider_config("same_provider_account_failover = false");
        reset_account_failover_globals();
        let c = account_session(&world, "C", ActiveProvider::Claude, false);
        c.set_account_failover(Some(true));
        assert_eq!(complete_text(&rt, &c).unwrap(), "hi from claude-fox");
    });
}

#[test]
fn short_429_retries_same_account() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let rt = enter_test_runtime();

        // An ordinary 429 inside the stream is not account exhaustion: the
        // runtime owns its retries, the error is replayed untouched, and the
        // session stays on its account.
        world.set("claude-otter", FakeAccountBehavior::InStreamShort429);
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        let err = complete_text(&rt, &a).expect_err("the short 429 reaches the consumer");
        assert!(err.to_string().contains("429"), "{err:#}");
        assert_eq!(world.labels_for("A"), ["claude-otter"]);
        assert_eq!(a.account_pin(AccountProviderKind::Claude), None);
        assert!(a.drain_startup_notices().is_empty());

        // Not recorded as exhausted: the next turn goes to the same account.
        world.set("claude-otter", FakeAccountBehavior::Ok);
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-otter");

        // A synchronous short 429 does not move the session either, and
        // never touches the process-wide override.
        world.set(
            "claude-otter",
            FakeAccountBehavior::SyncError(
                "Anthropic API error (429 Too Many Requests): rate limited",
            ),
        );
        let b = account_session(&world, "B", ActiveProvider::Claude, false);
        let _ = complete_text(&rt, &b).expect_err("short 429");
        assert_eq!(world.labels_for("B"), ["claude-otter"]);
        assert_eq!(b.account_pin(AccountProviderKind::Claude), None);
        assert_eq!(crate::auth::claude::get_active_account_override(), None);
    });
}

#[test]
fn unavailability_is_per_label() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        write_provider_config("same_provider_account_failover = false");
        let world = Arc::new(FakeAccountWorld::default());
        world.set(
            "claude-otter",
            FakeAccountBehavior::SyncError("401 unauthorized: token revoked"),
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        a.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-otter", None)),
        )
        .unwrap();
        let b = account_session(&world, "B", ActiveProvider::Claude, false);
        b.set_account_pin(
            AccountProviderKind::Claude,
            Some(AccountPin::new("claude-fox", None)),
        )
        .unwrap();
        let rt = enter_test_runtime();

        let _ = complete_text(&rt, &a).expect_err("otter is broken");
        assert_eq!(world.labels_for("A"), ["claude-otter"]);

        // Otter's mark must not block a window on fox.
        assert_eq!(complete_text(&rt, &b).unwrap(), "hi from claude-fox");
        assert_eq!(world.labels_for("B"), ["claude-fox"]);

        // Otter itself stays marked: A's next turn sends nothing to it.
        let _ = complete_text(&rt, &a).expect_err("otter is still marked");
        assert_eq!(world.labels_for("A"), ["claude-otter"]);
        assert!(provider_unavailability_detail_for_label("claude", "claude-otter").is_some());
        assert!(provider_unavailability_detail_for_label("claude", "claude-fox").is_none());
    });
}

/// Greptile "Relogin inherits exhausted status": a new subscription logged in
/// under a label that ran out must not inherit its exhausted mark. A token
/// refresh of the same login keeps it.
#[test]
fn relogin_under_exhausted_label_is_not_exhausted() {
    with_account_failover_env(|| {
        let labels = store_claude_accounts(2);
        let otter = labels[0].clone();
        let now = chrono::Utc::now().timestamp();
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::Claude,
            &otter,
            Some(now + 3600),
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::Claude,
                &otter
            )
            .is_some()
        );

        // Same login, refreshed tokens: still exhausted.
        let mut auth = crate::auth::claude::load_auth_file().unwrap();
        let account = auth
            .anthropic_accounts
            .iter_mut()
            .find(|account| account.label == otter)
            .unwrap();
        account.access = "sk-ant-oat01-REFRESHED".to_string();
        account.refresh = "refresh-rotated".to_string();
        crate::auth::claude::save_auth_file(&auth).unwrap();
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::Claude,
                &otter
            )
            .is_some(),
            "a token refresh is the same subscription"
        );

        // Another subscription logged in under the same label.
        let mut auth = crate::auth::claude::load_auth_file().unwrap();
        let account = auth
            .anthropic_accounts
            .iter_mut()
            .find(|account| account.label == otter)
            .unwrap();
        account.email = Some("new-subscription@example.com".to_string());
        account.access = "sk-ant-oat01-NEWLOGIN".to_string();
        account.refresh = "refresh-newlogin".to_string();
        crate::auth::claude::save_auth_file(&auth).unwrap();
        assert_eq!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::Claude,
                &otter
            ),
            None,
            "a new login under the label must not inherit the old exhaustion"
        );
        assert_eq!(
            crate::provider::account_failover::account_resets_at(
                AccountProviderKind::Claude,
                &otter
            ),
            None
        );
    });
}

// ---------------------------------------------------------------------------
// No loops: failover never cycles through accounts that are all out of usage.
// ---------------------------------------------------------------------------

fn claude_all_limited(world: &FakeAccountWorld, resets: &[i64]) {
    for (label, resets_at) in ["claude-otter", "claude-fox", "claude-panda"]
        .into_iter()
        .zip(resets)
    {
        world.set(
            label,
            FakeAccountBehavior::InStreamUsageLimit {
                resets_at: *resets_at,
            },
        );
    }
}

/// Guard 1: one pass per call. With 3 accounts all out, exactly 3 requests,
/// then the earliest-reset error. Cross-provider fallback (a configured
/// OpenAI slot) must not re-enter same-provider failover.
#[test]
fn account_failover_all_limited_turn_tries_each_account_once() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let now = chrono::Utc::now().timestamp();
        claude_all_limited(&world, &[now + 3 * 3600, now + 3600, now + 2 * 3600]);
        let a = account_session(&world, "A", ActiveProvider::Claude, true);
        let rt = enter_test_runtime();
        let err = complete_text(&rt, &a).expect_err("all out");
        assert!(
            err.to_string()
                .contains("All 3 Claude accounts are out of usage. First reset: claude-fox"),
            "{err:#}"
        );
        assert_eq!(world.requests().len(), 3, "{:?}", world.requests());
        assert_eq!(
            world.labels_for("A"),
            ["claude-otter", "claude-fox", "claude-panda"]
        );
        // Some pre-marked: fewer requests, never more than the stored count.
        reset_account_failover_globals();
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::Claude,
            "claude-fox",
            Some(now + 3600),
        );
        let b = account_session(&world, "B", ActiveProvider::Claude, true);
        let _ = complete_text(&rt, &b).expect_err("all out");
        assert_eq!(world.labels_for("B"), ["claude-otter", "claude-panda"]);
    });
}

/// Guard 2: once every account is marked out, the next turn sends nothing
/// until the earliest reset and fails with the same earliest-reset message.
#[test]
fn account_failover_next_turn_after_all_limited_sends_no_requests() {
    with_account_failover_env(|| {
        store_claude_accounts(3);
        let world = Arc::new(FakeAccountWorld::default());
        let now = chrono::Utc::now().timestamp();
        claude_all_limited(&world, &[now + 3 * 3600, now + 3600, now + 2 * 3600]);
        let a = account_session(&world, "A", ActiveProvider::Claude, true);
        let rt = enter_test_runtime();
        let first = complete_text(&rt, &a).expect_err("all out");
        assert_eq!(world.requests().len(), 3);
        for turn in 0..3 {
            let err = complete_text(&rt, &a).expect_err("still all out");
            assert!(
                err.to_string()
                    .contains("All 3 Claude accounts are out of usage. First reset: claude-fox"),
                "turn {turn}: {err:#}"
            );
            assert_eq!(world.requests().len(), 3, "turn {turn} sent a request");
        }
        // Another window on the same accounts sends nothing either.
        let b = account_session(&world, "B", ActiveProvider::Claude, true);
        let _ = complete_text(&rt, &b).expect_err("all out");
        assert!(
            world.labels_for("B").is_empty(),
            "{:?}",
            world.labels_for("B")
        );
        drop(first);
    });
}

/// Guard 3: a usage-limit 429 without reset headers keeps the account out
/// for UNKNOWN_RESET_EXHAUSTION_SECS, so later turns do not hammer it.
#[test]
fn account_failover_unknown_reset_not_retried_within_window() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        let world = Arc::new(FakeAccountWorld::default());
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimitNoReset,
        );
        world.set("claude-fox", FakeAccountBehavior::InStreamUsageLimitNoReset);
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        let rt = enter_test_runtime();
        let _ = complete_text(&rt, &a).expect_err("both out");
        assert_eq!(world.requests().len(), 2);
        let until = crate::provider::account_failover::account_exhausted(
            AccountProviderKind::Claude,
            "claude-otter",
        )
        .expect("marked");
        let window = crate::provider::account_failover::UNKNOWN_RESET_EXHAUSTION_SECS;
        let now = chrono::Utc::now().timestamp();
        assert!(
            until >= now + window - 5 && until <= now + window,
            "{until}"
        );
        for _ in 0..3 {
            let _ = complete_text(&rt, &a).expect_err("still out");
        }
        assert_eq!(world.requests().len(), 2, "{:?}", world.requests());
    });
}

/// Guard 5: return home only after the recorded reset passed, at most once
/// per reset. Home still limited after returning: record it, move away once,
/// and do not bounce back each turn.
#[test]
fn account_failover_return_home_still_limited_does_not_bounce() {
    with_account_failover_env(|| {
        store_claude_accounts(2);
        let world = Arc::new(FakeAccountWorld::default());
        let rt = enter_test_runtime();
        let soon = chrono::Utc::now().timestamp() + 2;
        world.set(
            "claude-otter",
            FakeAccountBehavior::InStreamUsageLimit { resets_at: soon },
        );
        let a = account_session(&world, "A", ActiveProvider::Claude, false);
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");
        assert!(
            a.return_account_home_if_reset().is_empty(),
            "not before the reset"
        );

        std::thread::sleep(std::time::Duration::from_millis(2_200));
        // The provider still reports the old (now past) reset time.
        assert_eq!(
            a.return_account_home_if_reset(),
            vec![AccountProviderKind::Claude]
        );
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");
        assert_eq!(
            world.labels_for("A"),
            ["claude-otter", "claude-fox", "claude-otter", "claude-fox"],
            "home tried once after its reset, then one move away"
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::Claude,
                "claude-otter"
            )
            .is_some(),
            "a past reset time must not clear the new mark at once"
        );
        for _ in 0..3 {
            assert!(a.return_account_home_if_reset().is_empty(), "no bounce");
            assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-fox");
        }
        assert_eq!(
            world
                .labels_for("A")
                .iter()
                .filter(|l| *l == "claude-otter")
                .count(),
            2,
            "otter got no more requests: {:?}",
            world.labels_for("A")
        );

        // A new reset time passes: return home once more.
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::Claude,
            "claude-otter",
            Some(chrono::Utc::now().timestamp() + 1),
        );
        world.set("claude-otter", FakeAccountBehavior::Ok);
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        assert_eq!(
            a.return_account_home_if_reset(),
            vec![AccountProviderKind::Claude]
        );
        assert_eq!(complete_text(&rt, &a).unwrap(), "hi from claude-otter");
    });
}

#[test]
fn production_usage_limit_prompt_then_accepted_switch_preserves_home() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            super::account_failover::reset_account_exhaustion_for_tests();
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            let reset = chrono::Utc::now().timestamp() + 3600;
            let failure = format!(
                "429 subscription usage exhausted\n{}",
                jcode_provider_core::account_usage_limit_marker(Some(reset))
            );
            bind_fixture_account(
                &provider,
                &attempts,
                "openai-otter",
                &[("openai-otter", &failure)],
            );
            let global_before = auth::codex::active_account_label();
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .expect("countdown before switch");
            let prompt =
                parse_failover_prompt_message(&error.to_string()).expect("exact target prompt");
            assert_eq!(
                prompt.target_route.as_deref(),
                Some("openai-oauth@openai-fox:gpt-5.4")
            );
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
            assert_eq!(
                super::account_failover::account_resets_at(
                    AccountProviderKind::OpenAi,
                    "openai-otter"
                ),
                Some(reset)
            );
            assert!(
                provider
                    .account_failover_home(AccountProviderKind::OpenAi)
                    .is_none()
            );
            provider
                .set_model(prompt.target_route.as_deref().unwrap())
                .unwrap();
            assert_eq!(
                provider
                    .account_pin(AccountProviderKind::OpenAi)
                    .unwrap()
                    .label,
                "openai-fox"
            );
            assert_eq!(
                provider
                    .account_failover_home(AccountProviderKind::OpenAi)
                    .unwrap()
                    .label,
                "openai-otter"
            );
            assert_eq!(
                *attempts.lock().unwrap(),
                ["openai-otter"],
                "acceptance does not issue a hidden request"
            );
            assert_eq!(auth::codex::active_account_label(), global_before);
            super::account_failover::reset_account_exhaustion_for_tests();
        });
    });
}

#[test]
fn production_short_rate_limit_and_context_error_do_not_arm_home() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            for failure in [
                "429 Too Many Requests retry-after: 5",
                "context length exceeded",
            ] {
                super::account_failover::reset_account_exhaustion_for_tests();
                let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                bind_fixture_account(
                    &provider,
                    &attempts,
                    "openai-otter",
                    &[("openai-otter", failure)],
                );
                let _ = provider.complete(&[], &[], "system", None).await;
                assert!(
                    super::account_failover::account_exhausted(
                        AccountProviderKind::OpenAi,
                        "openai-otter"
                    )
                    .is_none(),
                    "{failure}"
                );
                assert!(
                    provider
                        .approved_account_switch_origin(ActiveProvider::OpenAI)
                        .is_none(),
                    "{failure}"
                );
                assert!(
                    provider
                        .account_failover_home(AccountProviderKind::OpenAi)
                        .is_none()
                );
            }
        });
    });
}

#[test]
fn auto_subscription_failure_is_pinned_before_any_api_spending() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            let mut pool = crate::auth::account_pool::AccountPool::load();
            pool.excluded.push("openai-api".into());
            pool.save().unwrap();
            let original = Arc::new(BoundAccountRoutingProvider {
                attempts: attempts.clone(),
                failures: [("openai-otter".into(), "401 OAuth refresh rejected".into())].into(),
                label: Mutex::new("unbound".into()),
                mode: Mutex::new(CredentialMode::Auto),
            });
            *provider.openai.write().unwrap() = Some(original.clone());
            let global_before = auth::codex::active_account_label();
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .unwrap();
            let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
            assert_eq!(
                prompt.target_route.as_deref(),
                Some("openai-oauth@openai-fox:gpt-5.4")
            );
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
            assert_eq!(
                provider.oauth_account_label().as_deref(),
                Some("openai-otter")
            );
            assert_eq!(
                provider.openai_provider().unwrap().credential_mode(),
                CredentialMode::OAuth
            );
            assert_eq!(original.credential_mode(), CredentialMode::Auto);
            assert_eq!(original.oauth_account_label().as_deref(), Some("unbound"));
            assert_eq!(auth::codex::active_account_label(), global_before);
        });
    });
}

#[test]
fn exact_target_readiness_rejects_dead_refresh_and_unavailable_model() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let (_provider, _attempts, _cleanup) = account_routing_fixture(&[]);
        let route = "openai-oauth@openai-fox:gpt-5.4";
        auth::codex::upsert_account_from_tokens("openai-fox", "expired", "", None, Some(1))
            .unwrap();
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        auth::codex::upsert_account_from_tokens(
            "openai-fox",
            "expired",
            "dead-refresh-fox",
            None,
            Some(1),
        )
        .unwrap();
        crate::auth::refresh_state::record_permanent_rejection(
            "openai",
            "dead-refresh-fox",
            "invalid_grant",
        )
        .unwrap();
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        auth::codex::upsert_account_from_tokens(
            "openai-fox",
            "fresh",
            "new-refresh-fox",
            None,
            None,
        )
        .unwrap();
        assert!(MultiProvider::is_failover_route_pool_approved(route));
        super::models::record_model_unavailable_for_scope(
            "openai-fox",
            "gpt-5.4",
            "model access denied",
        );
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        super::models::clear_model_unavailable_for_scope("openai-fox", "gpt-5.4");
        assert!(!MultiProvider::is_failover_route_pool_approved(
            "openai-oauth@openai-fox:not-a-model"
        ));
    });
}

#[test]
fn context_failover_never_offers_known_too_small_routes() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let (provider, _attempts, _cleanup) = account_routing_fixture(&[]);
        assert!(
            provider
                .cross_provider_pool_prompt(
                    ActiveProvider::OpenAI,
                    ActiveProvider::Claude,
                    "context length exceeded".into(),
                    0,
                    usize::MAX
                )
                .is_none()
        );
    });
}

#[test]
fn pinned_account_prompts_use_session_identity_and_apply_without_global_override() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            use futures::StreamExt;
            for split in [false, true] {
                let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                bind_fixture_account(
                    &provider,
                    &attempts,
                    "openai-fox",
                    &[("openai-fox", "429 quota exhausted")],
                );
                let sibling = provider.fork();
                let result = if split {
                    provider
                        .complete_split(&[], &[], "static", "dynamic", None)
                        .await
                } else {
                    provider.complete(&[], &[], "system", None).await
                };
                let error = result.err().unwrap();
                let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
                let route = prompt.target_route.unwrap();
                assert_eq!(route, "openai-oauth@openai-panda:gpt-5.4");
                assert_eq!(*attempts.lock().unwrap(), ["openai-fox"]);
                assert_eq!(
                    provider.oauth_account_label().as_deref(),
                    Some("openai-fox")
                );
                assert_eq!(sibling.oauth_account_label().as_deref(), Some("openai-fox"));
                assert!(
                    super::models::provider_unavailability_detail_for_account_label(
                        "openai",
                        Some("openai-fox")
                    )
                    .is_some()
                );
                assert!(
                    super::models::provider_unavailability_detail_for_account_label(
                        "openai",
                        Some("openai-otter")
                    )
                    .is_none()
                );
                assert!(MultiProvider::is_failover_route_pool_approved(&route));
                provider.set_model(&route).unwrap();
                let mut stream = provider.complete(&[], &[], "system", None).await.unwrap();
                assert!(stream.next().await.unwrap().is_ok());
                assert_eq!(*attempts.lock().unwrap(), ["openai-fox", "openai-panda"]);
                assert_eq!(
                    provider.oauth_account_label().as_deref(),
                    Some("openai-panda")
                );
                assert_eq!(sibling.oauth_account_label().as_deref(), Some("openai-fox"));
                assert_eq!(
                    auth::codex::active_account_label().as_deref(),
                    Some("openai-otter")
                );
            }
        });
    });
}

#[test]
fn pool_order_can_choose_explicit_api_before_same_vendor_oauth() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            bind_fixture_account(
                &provider,
                &attempts,
                "openai-otter",
                &[("openai-otter", "429 quota exhausted")],
            );
            let mut pool = crate::auth::account_pool::AccountPool::load();
            pool.members.insert(1, "openai-api".into());
            pool.save().unwrap();
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .unwrap();
            let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
            let route = prompt.target_route.unwrap();
            assert!(
                route.starts_with("openai-api:"),
                "exact metered identity should follow configured order: {route}"
            );
            assert!(MultiProvider::is_failover_route_pool_approved(&route));
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
            pool.members.retain(|key| key != "openai-api");
            pool.excluded.push("openai-api".into());
            pool.save().unwrap();
            assert!(!MultiProvider::is_failover_route_pool_approved(&route));
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .unwrap();
            let prompt = parse_failover_prompt_message(&error.to_string()).unwrap();
            assert_eq!(
                prompt.target_route.as_deref(),
                Some("openai-oauth@openai-fox:gpt-5.4")
            );
            assert_eq!(*attempts.lock().unwrap(), ["openai-otter"]);
        });
    });
}

#[test]
fn missing_or_excluded_pins_never_fallback() {
    with_clean_provider_test_env(|| {
        enter_test_runtime().block_on(async {
            for label in ["missing", "openai-wolf"] {
                let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
                bind_fixture_account(
                    &provider,
                    &attempts,
                    label,
                    &[(label, "429 quota exhausted")],
                );
                let error = provider
                    .complete(&[], &[], "system", None)
                    .await
                    .err()
                    .unwrap();
                assert!(parse_failover_prompt_message(&error.to_string()).is_none());
                assert_eq!(
                    attempts.lock().unwrap().len(),
                    usize::from(label != "missing")
                );
                assert_eq!(provider.oauth_account_label().as_deref(), Some(label));
                assert_eq!(
                    auth::codex::active_account_label().as_deref(),
                    Some("openai-otter")
                );
            }
        });
    });
}

#[test]
fn timer_target_revalidation_rejects_deleted_excluded_and_cooled_accounts() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let (_provider, _attempts, _cleanup) = account_routing_fixture(&[]);
        let route = "openai-oauth@openai-fox:gpt-5.4";
        assert!(MultiProvider::is_failover_route_pool_approved(route));
        let mut pool = crate::auth::account_pool::AccountPool::load();
        pool.members.retain(|member| member != "openai:openai-fox");
        pool.excluded.push("openai:openai-fox".into());
        pool.save().unwrap();
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        pool.excluded.retain(|member| member != "openai:openai-fox");
        pool.save().unwrap();
        super::models::record_provider_unavailable_for_account_label(
            "openai",
            Some("openai-fox"),
            "429 quota exhausted",
        );
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        super::models::clear_openai_provider_unavailability_for_account_label(Some("openai-fox"));
        auth::codex::remove_account("openai-fox").unwrap();
        assert!(!MultiProvider::is_failover_route_pool_approved(route));
        assert!(!MultiProvider::is_failover_route_pool_approved(
            "openai-oauth:gpt-5.4"
        ));
        assert!(!MultiProvider::is_failover_route_pool_approved(
            "openai-api:gpt-5.4"
        ));
    });
}
