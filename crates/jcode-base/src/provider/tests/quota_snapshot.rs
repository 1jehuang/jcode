//! Regression tests exercise the same cache store and mark writers as requests.
use super::*;
use crate::usage::{OpenAIUsageData, OpenAIUsageWindow};
use std::sync::Barrier;

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
    record_provider_unavailable_for_label("openai", LABEL, "openai-otter is out of usage");
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
    let (accepted, accepted_rx) = std::sync::mpsc::channel();
    let resume = Barrier::new(2);
    std::thread::scope(|scope| {
        let resume_ref = &resume;
        let snapshot = scope.spawn(move || {
            crate::usage::store_openai_usage_for_generation_then(
                crate::usage::openai_usage_generation(),
                format!("label:{LABEL}"),
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
        assert!(provider_unavailability_detail_for_label("openai", LABEL).is_some());
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
            provider_unavailability_detail_for_label("openai", LABEL),
            None
        );
    });
}

#[test]
fn accepted_snapshot_preserves_non_quota_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        record_provider_unavailable_for_label("openai", LABEL, "authentication failed");
        store_available();
        assert!(provider_unavailability_detail_for_label("openai", LABEL).is_some());
    });
}

#[test]
fn accepted_snapshot_clears_marker_quota_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(1);
        record_provider_unavailable_for_label(
            "openai",
            LABEL,
            &jcode_provider_core::account_usage_limit_marker(None),
        );
        store_available();
        assert_eq!(
            provider_unavailability_detail_for_label("openai", LABEL),
            None
        );
    });
}

#[test]
fn accepted_snapshot_preserves_other_account_cooldown() {
    with_account_failover_env(|| {
        store_openai_accounts(2);
        record_provider_unavailable_for_label("openai", "openai-fox", "out of usage");
        store_available();
        assert!(provider_unavailability_detail_for_label("openai", "openai-fox").is_some());
    });
}

#[test]
fn claude_cooldown_does_not_advance_openai_generation() {
    with_account_failover_env(|| {
        let generation = crate::usage::openai_usage_generation();
        record_provider_unavailable_for_label("claude", "claude-otter", "out of usage");
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
