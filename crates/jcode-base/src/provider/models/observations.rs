use super::*;

/// Fresh usage shows the OpenAI account `account_label` is open again. Drop
/// its usage-limit mark, and its provider cooldown only when that cooldown
/// was recorded for a usage or rate limit. An auth (401/403) or other
/// cooldown says nothing about quota and stays until it expires.
pub fn clear_openai_usage_limit_for_account_label(account_label: &str) {
    let key = provider_runtime_scope_key("openai", Some(account_label));
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write()
        && unavailable
            .get(&key)
            .is_some_and(|entry| is_usage_limit_cooldown_reason(&entry.reason))
    {
        unavailable.remove(&key);
    }
    crate::provider::account_failover::clear_account_exhausted(
        jcode_provider_core::AccountProviderKind::OpenAi,
        account_label,
    );
}

/// True when a cooldown reason describes a usage, quota or rate limit and not
/// an authentication or access failure.
fn is_usage_limit_cooldown_reason(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    let auth = [
        "401",
        "403",
        "unauthorized",
        "forbidden",
        "access denied",
        "authentication",
        "credentials",
        "token exchange",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if auth {
        return false;
    }
    [
        "usage",
        "quota",
        "rate limit",
        "rate-limit",
        "rate_limit",
        "too many requests",
        "429",
        "limit reached",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Compare and remove under each owning lock. No locks are nested, and a
/// snapshot accepted before a new mark cannot remove that newer observation.
pub(crate) fn clear_openai_unavailability_through_generation(label: &str, generation: u64) {
    let key = provider_runtime_scope_key("openai", Some(label));
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write()
        && unavailable.get(&key).is_some_and(|entry| {
            entry.generation <= generation
                && (jcode_provider_core::classify_account_usage_limit(&entry.reason).is_some()
                    || is_usage_limit_cooldown_reason(&entry.reason))
        })
    {
        unavailable.remove(&key);
    }
    crate::provider::account_failover::clear_openai_account_exhausted_through_generation(
        label, generation,
    );
}
