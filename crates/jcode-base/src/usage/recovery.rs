//! Live quota confirmations, separate from cached display snapshots.
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

static AVAILABLE: LazyLock<Mutex<HashMap<(String, String), i64>>> = LazyLock::new(Mutex::default);

pub(super) fn record_available(provider: &str, label: &str, started: Instant) {
    crate::provider::models::clear_quota_unavailability_after_probe(provider, label, started);
    if crate::provider::models::provider_unavailability_detail_for_account_label(
        provider,
        Some(label),
    )
    .is_some()
    {
        return;
    }
    let at = chrono::Utc::now().timestamp_millis()
        - i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let mut available = AVAILABLE.lock().unwrap_or_else(|p| p.into_inner());
    let entry = available
        .entry((provider.into(), label.into()))
        .or_default();
    *entry = (*entry).max(at);
}

/// Cached reports do not advance the start time of the last successful probe.
pub fn confirmed_available_accounts() -> Vec<jcode_protocol::AccountUsageRecovery> {
    AVAILABLE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(
            |((provider, label), at)| jcode_protocol::AccountUsageRecovery {
                provider: provider.clone(),
                account_label: label.clone(),
                checked_at_unix_ms: *at,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_recovery_is_account_scoped_and_rejects_a_newer_denial() {
        let label = "background-recovery-test";
        let started = Instant::now() - std::time::Duration::from_secs(60);
        let before = chrono::Utc::now().timestamp_millis();
        record_available("openai", label, started);
        let at = confirmed_available_accounts()
            .into_iter()
            .find(|item| item.provider == "openai" && item.account_label == label)
            .unwrap()
            .checked_at_unix_ms;
        assert!(at <= before - 59_000);
        crate::provider::models::record_provider_unavailable_for_account_label(
            "openai",
            Some(label),
            "usage_limit_reached",
        );
        record_available("openai", label, started);
        assert!(
            crate::provider::models::provider_unavailability_detail_for_account_label(
                "openai",
                Some(label)
            )
            .is_some()
        );
        assert_eq!(
            confirmed_available_accounts()
                .into_iter()
                .find(|item| item.provider == "openai" && item.account_label == label)
                .unwrap()
                .checked_at_unix_ms,
            at
        );
        record_available("openai", label, Instant::now());
        assert!(
            crate::provider::models::provider_unavailability_detail_for_account_label(
                "openai",
                Some(label)
            )
            .is_none()
        );
    }
}
