//! Regression tests exercise the same cache store and mark writers as requests.
use super::*;
use crate::usage::{OpenAIUsageData, OpenAIUsageWindow};
use std::sync::Barrier;

#[test]
fn aggregate_usage_report_does_not_bypass_scoped_mark_clearing() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        let rt = enter_test_runtime();
        for reason in ["401 unauthorized: token rejected", "429 quota exhausted"] {
            record_provider_unavailable_for_account("openai", reason);
            let generation = crate::usage::openai_usage_generation();
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

const LABEL: &str = "openai-otter";

fn available() -> OpenAIUsageData {
    OpenAIUsageData {
        seven_day: Some(OpenAIUsageWindow {
            name: "7-day window".into(),
            usage_ratio: 0.0,
            resets_at: None,
        }),
        fetched_at: Some(std::time::Instant::now()),
        ..Default::default()
    }
}

fn mark_exhausted() {
    crate::provider::account_failover::record_account_exhausted(
        AccountProviderKind::OpenAi,
        LABEL,
        Some(chrono::Utc::now().timestamp() + 5 * 3600),
    );
}

fn mark_cooldown() {
    record_provider_unavailable_for_account_label("openai", Some(LABEL), "openai-otter is out of usage");
}

fn store_available() {
    crate::usage::store_openai_usage_for_generation_then(
        crate::usage::openai_usage_generation(),
        format!("label:{LABEL}"),
        available(),
        || {},
    );
}

/// The barrier is inside the real production store, after cache unlock and
/// before deferred removal. No sleeps, network, live credentials, or mock locks.
fn deferred_snapshot(mark: impl FnOnce()) {
    deferred_snapshot_for_key(format!("label:{LABEL}"), mark);
}

fn deferred_snapshot_for_key(cache_key: String, mark: impl FnOnce()) {
    let (accepted, accepted_rx) = std::sync::mpsc::channel();
    let resume = Barrier::new(2);
    std::thread::scope(|scope| {
        let resume_ref = &resume;
        let snapshot = scope.spawn(move || {
            crate::usage::store_openai_usage_for_generation_then(
                crate::usage::openai_usage_generation(),
                cache_key,
                available(),
                || {
                    accepted.send(()).unwrap();
                    resume_ref.wait();
                },
            );
        });
        accepted_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("snapshot must reach deferred clearing");
        mark();
        resume.wait();
        snapshot.join().unwrap();
    });
}

#[test]
fn deferred_snapshot_preserves_newer_exhaustion_mark() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        mark_exhausted();
        deferred_snapshot(mark_exhausted);
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                LABEL,
            )
            .is_some()
        );
    });
}

#[test]
fn deferred_snapshot_preserves_newer_quota_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        mark_cooldown();
        deferred_snapshot(mark_cooldown);
        assert!(provider_unavailability_detail_for_account_label("openai", Some(LABEL)).is_some());
    });
}

#[test]
fn deferred_snapshot_cannot_send_a_newly_exhausted_account_request() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        let world = Arc::new(FakeAccountWorld::default());
        let session = account_session(&world, "A", ActiveProvider::OpenAI, false);
        let rt = enter_test_runtime();
        deferred_snapshot(|| {
            mark_exhausted();
            mark_cooldown();
        });
        let result = complete_text(&rt, &session);
        assert!(result.is_err() && world.labels_for("A").is_empty());
    });
}

#[test]
fn accepted_snapshot_clears_older_exhaustion_mark() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        mark_exhausted();
        store_available();
        assert_eq!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                LABEL,
            ),
            None
        );
    });
}

#[test]
fn accepted_snapshot_clears_older_quota_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        mark_cooldown();
        store_available();
        assert_eq!(
            provider_unavailability_detail_for_account_label("openai", Some(LABEL)),
            None
        );
    });
}

#[test]
fn accepted_snapshot_preserves_non_quota_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        record_provider_unavailable_for_account_label("openai", Some(LABEL), "authentication failed");
        store_available();
        assert!(provider_unavailability_detail_for_account_label("openai", Some(LABEL)).is_some());
    });
}

#[test]
fn accepted_snapshot_clears_older_rate_limit_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        record_provider_unavailable_for_account_label("openai", Some(LABEL), "OpenAI API error (429): too many requests");
        store_available();
        assert_eq!(provider_unavailability_detail_for_account_label("openai", Some(LABEL)), None);
    });
}

#[test]
fn accepted_snapshot_preserves_auth_cooldown_mentioning_quota() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        record_provider_unavailable_for_account_label("openai", Some(LABEL), "OpenAI API error (401 Unauthorized): quota unavailable until authentication succeeds");
        store_available();
        assert!(provider_unavailability_detail_for_account_label("openai", Some(LABEL)).is_some());
    });
}

#[test]
fn accepted_snapshot_clears_marker_quota_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        record_provider_unavailable_for_account_label("openai", Some(LABEL),
            &jcode_provider_core::account_usage_limit_marker(None),
        );
        store_available();
        assert_eq!(
            provider_unavailability_detail_for_account_label("openai", Some(LABEL)),
            None
        );
    });
}

#[test]
fn accepted_snapshot_preserves_other_account_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(2);
        record_provider_unavailable_for_account_label("openai", Some("openai-fox"), "out of usage");
        store_available();
        assert!(provider_unavailability_detail_for_account_label("openai", Some("openai-fox")).is_some());
    });
}

#[test]
fn claude_cooldown_does_not_advance_openai_generation() {
    with_account_failover_env(|| {
        let generation = crate::usage::openai_usage_generation();
        record_provider_unavailable_for_account_label("claude", Some("claude-otter"), "out of usage");
        assert_eq!(crate::usage::openai_usage_generation(), generation);
    });
}

#[test]
fn snapshot_started_before_exhaustion_cannot_clear_it() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        let generation = crate::usage::openai_usage_generation();
        mark_exhausted();
        crate::usage::store_openai_usage_for_generation_then(
            generation,
            format!("label:{LABEL}"),
            available(),
            || panic!("a stale snapshot must not reach deferred clearing"),
        );
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                LABEL,
            )
            .is_some()
        );
    });
}

struct DefaultQuotaScope;

impl Drop for DefaultQuotaScope {
    fn drop(&mut self) {
        crate::provider::models::clear_openai_provider_unavailability_for_account_label(None);
    }
}

fn with_default_quota_env(f: impl FnOnce()) {
    with_account_failover_env(|| {
        crate::provider::models::clear_openai_provider_unavailability_for_account_label(None);
        let _scope = DefaultQuotaScope;
        f();
    });
}

fn default_cooldown() -> Option<String> {
    provider_unavailability_detail_for_account_label("openai", None)
}

fn mark_default(reason: &str) {
    record_provider_unavailable_for_account_label("openai", None, reason);
}

fn store_default_available() {
    crate::usage::store_openai_usage_for_generation_then(
        crate::usage::openai_usage_generation(),
        "token:default-snapshot".to_string(),
        available(),
        || {},
    );
}

#[test]
fn accepted_default_snapshot_clears_only_older_default_quota() {
    with_default_quota_env(|| {
        store_openai_accounts(1);
        mark_exhausted();
        mark_cooldown();
        mark_default("429 out of usage");
        assert!(default_cooldown().is_some());
        store_default_available();
        assert_eq!(default_cooldown(), None);
        assert!(provider_unavailability_detail_for_account_label("openai", Some(LABEL)).is_some());
        assert!(
            crate::provider::account_failover::account_exhausted(
                AccountProviderKind::OpenAi,
                LABEL,
            )
            .is_some()
        );
    });
}

#[test]
fn accepted_default_snapshot_preserves_auth_quota_cooldown() {
    with_default_quota_env(|| {
        store_openai_accounts(1);
        for reason in [
            "401 unauthorized quota endpoint".to_string(),
            format!(
                "401 unauthorized {}",
                jcode_provider_core::account_usage_limit_marker(None)
            ),
        ] {
            mark_default(&reason);
            store_default_available();
            assert!(default_cooldown().is_some());
        }
    });
}

#[test]
fn deferred_default_snapshot_preserves_newer_quota_cooldown() {
    with_default_quota_env(|| {
        store_openai_accounts(1);
        mark_default("out of usage");
        deferred_snapshot_for_key("token:default-race".to_string(), || {
            mark_default("429 newer quota limit");
        });
        assert!(default_cooldown().is_some_and(|reason| reason.starts_with("429 newer quota limit")));
    });
}

#[test]
fn snapshot_started_before_default_quota_cannot_clear_it() {
    with_default_quota_env(|| {
        let generation = crate::usage::openai_usage_generation();
        mark_default("out of usage");
        crate::usage::store_openai_usage_for_generation_then(
            generation,
            "token:stale-default".to_string(),
            available(),
            || panic!("stale default snapshot must not reach clearing"),
        );
        assert!(default_cooldown().is_some());
    });
}

#[test]
fn accepted_label_snapshot_does_not_clear_default_quota() {
    with_default_quota_env(|| {
        store_openai_accounts(1);
        mark_default("out of usage");
        store_available();
        assert!(default_cooldown().is_some());
    });
}

#[test]
fn unconfirmed_default_snapshot_does_not_clear_quota() {
    with_default_quota_env(|| {
        mark_default("out of usage");
        crate::usage::store_openai_usage_for_generation_then(
            crate::usage::openai_usage_generation(),
            "token:missing-usage".to_string(),
            OpenAIUsageData::default(),
            || panic!("missing usage must not reach clearing"),
        );
        assert!(default_cooldown().is_some());
    });
}
