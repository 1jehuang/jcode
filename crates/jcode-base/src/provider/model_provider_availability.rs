use super::*;

pub(super) fn runtime_provider_unavailability(
    provider: &str,
) -> Option<RuntimeProviderUnavailability> {
    let key = current_provider_runtime_scope_key(provider);
    runtime_provider_unavailability_for_key(&key)
}

pub(super) fn runtime_provider_unavailability_for_key(
    key: &str,
) -> Option<RuntimeProviderUnavailability> {
    let mut unavailable = match ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        Ok(unavailable) => unavailable,
        Err(error) => {
            crate::logging::warn(&format!("Provider availability lock poisoned: {error}"));
            return None;
        }
    };
    if let Some(entry) = unavailable.get(key) {
        if entry.recorded_at.elapsed() <= PROVIDER_RUNTIME_UNAVAILABLE_TTL {
            return Some(entry.clone());
        }
        unavailable.remove(key);
    }
    None
}

pub fn record_provider_unavailable_for_account(provider: &str, reason: &str) {
    let key = current_provider_runtime_scope_key(provider);
    record_provider_unavailable_for_key(key, reason);
}

/// Record a session-bound login failure without changing global auth selection.
pub fn record_provider_unavailable_for_account_label(
    provider: &str,
    label: Option<&str>,
    reason: &str,
) {
    record_provider_unavailable_for_key(provider_runtime_scope_key(provider, label), reason);
}

pub(super) fn record_provider_unavailable_for_key(key: String, reason: &str) {
    if key.trim().is_empty() {
        return;
    }

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        unavailable.insert(
            key,
            RuntimeProviderUnavailability {
                reason: reason.trim().to_string(),
                recorded_at: Instant::now(),
                observed_at: SystemTime::now(),
            },
        );
        crate::provider::bump_catalog_generation();
    }
}

pub fn clear_provider_unavailable_for_account(provider: &str) {
    let key = current_provider_runtime_scope_key(provider);
    if key.trim().is_empty() {
        return;
    }

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write()
        && unavailable.remove(&key).is_some()
    {
        crate::provider::bump_catalog_generation();
    }
}

/// A successful live usage probe can supersede an older quota failure, but
/// cannot prove that authentication, billing, or transport failures recovered.
/// Compare the request start, not completion, so an in-flight probe cannot
/// erase a newer failure from an actual completion request.
pub(crate) fn clear_quota_unavailability_after_probe(
    provider: &str,
    label: &str,
    probe_started: Instant,
) {
    let key = provider_runtime_scope_key(provider, Some(label));
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write()
        && unavailable.get(&key).is_some_and(|entry| {
            entry.recorded_at <= probe_started && is_quota_failure(&entry.reason)
        })
    {
        unavailable.remove(&key);
        crate::provider::bump_catalog_generation();
    }
}

fn is_quota_failure(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    // Unknown and mixed failure reasons remain unavailable conservatively.
    if [
        "auth",
        "401",
        "403",
        "forbidden",
        "access denied",
        "billing",
        "credit",
        "payment",
        "timeout",
        "timed out",
        "connection",
        "network",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return false;
    }
    ["quota", "usage_limit", "usage limit", "usage exhausted"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Clear the quota cooldown for the exact OpenAI account that was reset, even
/// if the active account changed while confirmation or redemption was pending.
/// `None` refers to the default scope, not the current active account.
pub fn clear_openai_provider_unavailability_for_account_label(account_label: Option<&str>) {
    let key = provider_runtime_scope_key("openai", account_label);
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write()
        && unavailable.remove(&key).is_some()
    {
        crate::provider::bump_catalog_generation();
    }
}

/// Clear the quota cooldown for the exact Claude login whose limits were reset.
/// `None` refers to the active (or default) login.
pub fn clear_claude_provider_unavailability_for_account_label(account_label: Option<&str>) {
    let key = provider_runtime_scope_key("claude", account_label);
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write()
        && unavailable.remove(&key).is_some()
    {
        crate::provider::bump_catalog_generation();
    }
}

/// Clear all runtime model unavailability markers.
pub fn clear_all_model_unavailability_for_account() {
    let scope = current_openai_account_scope();
    OPENAI_MODEL_CATALOG_SERVICE.clear_runtime_model_unavailable_scope(&scope);
    crate::provider::bump_catalog_generation();
}

/// Clear all runtime provider unavailability markers.
pub fn clear_all_provider_unavailability_for_account() {
    let scope = current_openai_account_scope();
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        let before = unavailable.len();
        unavailable.retain(|key, _| !key.starts_with(&format!("openai::{}", scope)));
        if unavailable.len() != before {
            crate::provider::bump_catalog_generation();
        }
    }
}

pub fn provider_unavailability_detail_for_account(provider: &str) -> Option<String> {
    let entry = runtime_provider_unavailability(provider)?;
    Some(format_provider_unavailability_detail(entry))
}

/// Read another login's cooldown without changing the process-wide active login.
pub fn provider_unavailability_detail_for_account_label(
    provider: &str,
    account_label: Option<&str>,
) -> Option<String> {
    let key = provider_runtime_scope_key(provider, account_label);
    let entry = runtime_provider_unavailability_for_key(&key)?;
    Some(format_provider_unavailability_detail(entry))
}

pub(super) fn format_provider_unavailability_detail(
    entry: RuntimeProviderUnavailability,
) -> String {
    let mut detail = entry.reason;
    if let Ok(elapsed) = SystemTime::now().duration_since(entry.observed_at) {
        detail.push_str(&format!(
            " (runtime-error, {} ago)",
            format_elapsed_duration_short(elapsed)
        ));
    }

    detail
}

#[cfg(test)]
mod quota_recovery_tests {
    use super::*;

    #[test]
    fn live_quota_recovery_is_account_scoped_and_preserves_newer_failures() {
        let label = "usage-recovery-scope-test";
        let other = "usage-recovery-other-test";
        record_provider_unavailable_for_account_label("openai", Some(label), "usage_limit_reached");
        record_provider_unavailable_for_account_label("openai", Some(other), "quota exhausted");
        record_provider_unavailable_for_account_label("claude", Some(label), "quota exhausted");
        let started = Instant::now();
        clear_quota_unavailability_after_probe("openai", label, started);
        assert!(provider_unavailability_detail_for_account_label("openai", Some(label)).is_none());
        assert!(provider_unavailability_detail_for_account_label("openai", Some(other)).is_some());
        assert!(provider_unavailability_detail_for_account_label("claude", Some(label)).is_some());

        record_provider_unavailable_for_account_label("openai", Some(label), "usage_limit_reached");
        clear_quota_unavailability_after_probe("openai", label, started);
        assert!(provider_unavailability_detail_for_account_label("openai", Some(label)).is_some());
        clear_openai_provider_unavailability_for_account_label(Some(label));
        clear_openai_provider_unavailability_for_account_label(Some(other));
        clear_claude_provider_unavailability_for_account_label(Some(label));
    }

    #[test]
    fn live_quota_recovery_preserves_auth_transport_and_billing_failures() {
        let label = "usage-recovery-failure-test";
        for reason in [
            "401 unauthorized",
            "authentication failed",
            "403 forbidden",
            "connection failed",
            "network timeout",
            "billing quota exhausted",
            "insufficient credit",
            "unknown failure",
            "quota lookup unauthorized",
            "429 Too Many Requests",
            "rate limit",
            "rate-limited due to requests per minute",
        ] {
            record_provider_unavailable_for_account_label("claude", Some(label), reason);
            clear_quota_unavailability_after_probe("claude", label, Instant::now());
            assert!(
                provider_unavailability_detail_for_account_label("claude", Some(label)).is_some(),
                "{reason}"
            );
        }
        clear_claude_provider_unavailability_for_account_label(Some(label));
        for reason in [
            "usage_limit_reached",
            "usage limit reached",
            "quota exhausted",
        ] {
            assert!(is_quota_failure(reason), "{reason}");
        }
    }
}
