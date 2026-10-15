#[test]
fn aggregate_usage_report_does_not_bypass_scoped_mark_clearing() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        let rt = enter_test_runtime();
        for reason in ["401 unauthorized: token rejected", "429 quota exhausted"] {
            record_provider_unavailable_for_account("openai", reason);
            let generation = crate::usage::openai_usage_generation_for_tests();
            let report = crate::usage::ProviderUsage {
                provider_name: "OpenAI (ChatGPT)".into(),
                limits: vec![crate::usage::UsageLimit {
                    name: "Weekly".into(),
                    usage_percent: 0.0,
                    resets_at: None,
                }],
                ..Default::default()
            };
            rt.block_on(crate::usage::sync_openai_usage_reports_for_tests(&[report], generation));
            assert!(
                provider_unavailability_detail_for_account("openai").is_some(),
                "aggregate display synchronization must not clear {reason}; exact-label generation-bound cache storage owns clearing"
            );
            clear_provider_unavailable_for_account("openai");
        }
    });
}

fn complete_quota_text(
    rt: &tokio::runtime::Runtime,
    provider: &MultiProvider,
) -> anyhow::Result<String> {
    rt.block_on(async {
        let stream = provider
            .complete(&billing_test_messages(), &[], "", None)
            .await?;
        drain_text(stream).await
    })
}

/// Reported bug: an OpenAI account hit its limit, the usage reset early (a
/// banked reset, or the window rolled over), and `/usage` showed it open
/// again, yet every resume kept being skipped with "out of usage (resets
/// 02:10)". The mark held the reported reset time and nothing re-read usage.
/// A fresh snapshot confirming the account is open must clear the mark and
/// the provider cooldown so the resume is actually sent.
#[test]
fn openai_early_reset_clears_usage_limit_mark_and_resume_is_sent() {
    with_account_failover_env(|| {
        crate::usage::reset_openai_exhaustion_revalidation_for_tests();
        let labels = store_openai_accounts(1);
        assert_eq!(labels, ["openai-otter"]);
        let world = Arc::new(FakeAccountWorld::default());
        let a = account_session(&world, "A", ActiveProvider::OpenAI, false);
        let rt = enter_test_runtime();

        // The limit was hit with a reset hours away, exactly as in the logs.
        let far_reset = chrono::Utc::now().timestamp() + 5 * 3600;
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-otter",
            Some(far_reset),
        );
        record_provider_unavailable_for_label(
            "openai",
            "openai-otter",
            "openai-otter is out of usage",
        );
        crate::usage::store_openai_usage_for_label_for_tests(
            "openai-otter",
            crate::usage::OpenAIUsageData {
                hard_limit_reached: true,
                fetched_at: Some(std::time::Instant::now()),
                ..Default::default()
            },
        );
        assert!(
            complete_quota_text(&rt, &a).is_err(),
            "while really exhausted the request is still skipped"
        );
        assert!(world.labels_for("A").is_empty());

        // A partial or failed snapshot must not be read as "open again".
        crate::usage::store_openai_usage_for_label_for_tests(
            "openai-otter",
            crate::usage::OpenAIUsageData {
                fetched_at: Some(std::time::Instant::now()),
                last_error: Some("API error (500)".to_string()),
                ..Default::default()
            },
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                "openai-otter"
            )
            .is_some(),
            "an error snapshot proves nothing"
        );

        // Fresh usage after the early reset: weekly 0%, ordinary use allowed.
        crate::usage::store_openai_usage_for_label_for_tests(
            "openai-otter",
            crate::usage::OpenAIUsageData {
                seven_day: Some(crate::usage::OpenAIUsageWindow {
                    name: "7-day window".to_string(),
                    usage_ratio: 0.0,
                    resets_at: None,
                }),
                fetched_at: Some(std::time::Instant::now()),
                ..Default::default()
            },
        );
        assert_eq!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                "openai-otter"
            ),
            None,
            "fresh usage that confirms the reset must clear the mark"
        );
        assert_eq!(
            provider_unavailability_detail_for_label("openai", "openai-otter"),
            None,
            "and the provider cooldown"
        );
        assert_eq!(
            complete_quota_text(&rt, &a).expect("the resume must be sent after the reset"),
            "hi from openai-otter"
        );
        assert_eq!(world.labels_for("A"), ["openai-otter"]);
        crate::usage::forget_openai_usage_for_label_for_tests("openai-otter");
    });
}

#[path = "live_usage.rs"]
mod live_usage;
use live_usage::serve_openai_usage;

const OPENAI_USAGE_OPEN_BODY: &str = r#"{"rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":3,"limit_window_seconds":18000},"secondary_window":{"used_percent":10,"limit_window_seconds":604800}}}"#;

/// The live recheck path end to end: the session's own marked account is
/// rechecked against the (mock) usage endpoint, and the resume is sent.
#[test]
fn openai_marked_account_is_rechecked_live_and_the_resume_is_sent() {
    with_account_failover_env(|| {
        crate::usage::reset_openai_exhaustion_revalidation_for_tests();
        store_openai_accounts(1);
        let world = Arc::new(FakeAccountWorld::default());
        let a = account_session(&world, "A", ActiveProvider::OpenAI, false);
        let rt = enter_test_runtime();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        crate::usage::set_openai_usage_url_for_tests(Some(serve_openai_usage(
            &rt,
            OPENAI_USAGE_OPEN_BODY,
            Arc::clone(&hits),
        )));

        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-otter",
            Some(chrono::Utc::now().timestamp() + 5 * 3600),
        );
        record_provider_unavailable_for_label(
            "openai",
            "openai-otter",
            "openai-otter is out of usage",
        );

        let text = complete_quota_text(&rt, &a);
        crate::usage::set_openai_usage_url_for_tests(None);
        crate::usage::forget_openai_usage_for_label_for_tests("openai-otter");
        assert_eq!(text.expect("resume is sent"), "hi from openai-otter");
        assert!(
            hits.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "usage was fetched live"
        );
    });
}

/// The current account is still out, but a marked alternative reset early.
/// Rotation must recheck the alternative live instead of filtering it out.
#[test]
fn openai_rotation_rechecks_a_marked_alternative_live() {
    with_account_failover_env(|| {
        crate::usage::reset_openai_exhaustion_revalidation_for_tests();
        let labels = store_openai_accounts(2);
        assert_eq!(labels, ["openai-otter", "openai-fox"]);
        let world = Arc::new(FakeAccountWorld::default());
        let a = account_session(&world, "A", ActiveProvider::OpenAI, false);
        let rt = enter_test_runtime();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        crate::usage::set_openai_usage_url_for_tests(Some(serve_openai_usage(
            &rt,
            OPENAI_USAGE_OPEN_BODY,
            Arc::clone(&hits),
        )));

        let far = chrono::Utc::now().timestamp() + 5 * 3600;
        // otter is really out: its recheck is throttled, so it stays marked.
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-otter",
            Some(far),
        );
        crate::usage::mark_openai_exhaustion_revalidated_for_tests("openai-otter");
        // fox was marked too, but has reset early.
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-fox",
            Some(far),
        );

        let text = complete_text(&rt, &a);
        crate::usage::set_openai_usage_url_for_tests(None);
        crate::usage::forget_openai_usage_for_label_for_tests("openai-fox");
        assert_eq!(
            text.expect("the reset alternative takes the request"),
            "hi from openai-fox"
        );
        assert_eq!(world.labels_for("A"), ["openai-fox"]);
    });
}

#[test]
fn production_prompt_revalidates_only_marked_approved_alternative_before_acceptance() {
    with_account_failover_env(|| {
        crate::usage::reset_openai_exhaustion_revalidation_for_tests();
        let rt = enter_test_runtime();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        crate::usage::set_openai_usage_url_for_tests(Some(serve_openai_usage(
            &rt,
            OPENAI_USAGE_OPEN_BODY,
            Arc::clone(&hits),
        )));
        rt.block_on(async {
            let (provider, attempts, _cleanup) = account_routing_fixture(&[]);
            bind_fixture_account(&provider, &attempts, "openai-otter", &[]);
            let global_before = auth::codex::active_account_label();
            for label in ["openai-otter", "openai-fox", "openai-wolf"] {
                crate::provider::account_failover::record_account_exhausted(
                    AccountProviderKind::OpenAi,
                    label,
                    Some(chrono::Utc::now().timestamp() + 5 * 3600),
                );
                record_provider_unavailable_for_label("openai", label, "429 quota exhausted");
            }
            // The current login remains out. Fox reset early, while wolf is
            // explicitly excluded and panda has no mark needing a usage fetch.
            crate::usage::mark_openai_exhaustion_revalidated_for_tests("openai-otter");
            let error = provider
                .complete(&[], &[], "system", None)
                .await
                .err()
                .expect("consent prompt");
            let prompt = parse_failover_prompt_message(&error.to_string()).expect("exact target");
            assert_eq!(
                prompt.target_route.as_deref(),
                Some("openai-oauth@openai-fox:gpt-5.4")
            );
            assert_eq!(
                hits.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "only marked approved fox was rechecked"
            );
            assert!(
                attempts.lock().unwrap().is_empty(),
                "usage lookup must not issue any completion"
            );
            assert_eq!(
                provider
                    .account_pin(AccountProviderKind::OpenAi)
                    .unwrap()
                    .label,
                "openai-otter"
            );
            assert!(
                provider
                    .account_failover_home(AccountProviderKind::OpenAi)
                    .is_none()
            );
            assert_eq!(auth::codex::active_account_label(), global_before);
            assert!(
                crate::provider::account_failover::account_exhausted(
                    AccountProviderKind::OpenAi,
                    "openai-otter"
                )
                .is_some()
            );
            assert!(
                crate::provider::account_failover::account_exhausted(
                    AccountProviderKind::OpenAi,
                    "openai-fox"
                )
                .is_none()
            );
            assert!(
                crate::provider::account_failover::account_exhausted(
                    AccountProviderKind::OpenAi,
                    "openai-wolf"
                )
                .is_some()
            );
            provider
                .set_model(prompt.target_route.as_deref().unwrap())
                .unwrap();
            assert!(
                attempts.lock().unwrap().is_empty(),
                "acceptance alone does not send a request"
            );
            let stream = provider.complete(&[], &[], "system", None).await.unwrap();
            assert_eq!(drain_text(stream).await.unwrap(), "served by openai-fox");
            assert_eq!(*attempts.lock().unwrap(), ["openai-fox"]);
            assert_eq!(auth::codex::active_account_label(), global_before);
        });
        crate::usage::set_openai_usage_url_for_tests(None);
        for label in ["openai-otter", "openai-fox", "openai-wolf"] {
            crate::usage::forget_openai_usage_for_label_for_tests(label);
        }
    });
}

/// A usage fetch that started before a new limit was recorded must not clear
/// that newer mark when it lands.
#[test]
fn stale_available_snapshot_does_not_clear_a_newer_exhaustion_mark() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        let started_at = crate::usage::openai_usage_generation_for_tests();
        crate::provider::account_failover::record_account_exhausted(
            AccountProviderKind::OpenAi,
            "openai-otter",
            Some(chrono::Utc::now().timestamp() + 3600),
        );
        crate::usage::store_openai_usage_for_label_at_generation_for_tests(
            started_at,
            "openai-otter",
            crate::usage::OpenAIUsageData {
                seven_day: Some(crate::usage::OpenAIUsageWindow {
                    name: "7-day window".to_string(),
                    usage_ratio: 0.0,
                    resets_at: None,
                }),
                fetched_at: Some(std::time::Instant::now()),
                ..Default::default()
            },
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                "openai-otter"
            )
            .is_some(),
            "the stale snapshot must not clear the newer mark"
        );
        crate::usage::forget_openai_usage_for_label_for_tests("openai-otter");
    });
}

/// An available snapshot says nothing about credentials. A 401/403 cooldown
/// must stay until it expires.
#[test]
fn available_snapshot_keeps_an_auth_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        record_provider_unavailable_for_label(
            "openai",
            "openai-otter",
            "OpenAI API error (401 Unauthorized): token expired",
        );
        crate::usage::store_openai_usage_for_label_for_tests(
            "openai-otter",
            crate::usage::OpenAIUsageData {
                seven_day: Some(crate::usage::OpenAIUsageWindow {
                    name: "7-day window".to_string(),
                    usage_ratio: 0.0,
                    resets_at: None,
                }),
                fetched_at: Some(std::time::Instant::now()),
                ..Default::default()
            },
        );
        assert!(
            provider_unavailability_detail_for_label("openai", "openai-otter").is_some(),
            "an auth cooldown is not a usage limit"
        );
        crate::usage::forget_openai_usage_for_label_for_tests("openai-otter");
    });
}
