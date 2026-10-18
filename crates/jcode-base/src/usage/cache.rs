use super::openai_helpers::{classify_openai_limits, usage_percent_to_ratio};
use super::{
    AccountUsageSnapshot, ModelScopedUsageWindow, OpenAIUsageData, ProviderUsage, UsageData,
    UsageLimit,
};
use std::collections::HashMap;
use std::time::Instant;

/// Shared Anthropic usage cache used by the info widget, `/usage`, and
/// multi-account fallback logic so they don't hammer the same endpoint through
/// separate code paths.
static ANTHROPIC_USAGE_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, UsageData>>> =
    std::sync::OnceLock::new();

static OPENAI_USAGE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn openai_usage_generation() -> u64 {
    OPENAI_USAGE_GENERATION.load(std::sync::atomic::Ordering::SeqCst)
}

/// Shared OpenAI usage cache keyed by account label/token prefix.
static OPENAI_ACCOUNT_USAGE_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, OpenAIUsageData>>,
> = std::sync::OnceLock::new();

fn anthropic_usage_cache() -> &'static std::sync::Mutex<HashMap<String, UsageData>> {
    ANTHROPIC_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn openai_usage_cache() -> &'static std::sync::Mutex<HashMap<String, OpenAIUsageData>> {
    OPENAI_ACCOUNT_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Read fresh usage attributed to a saved account without probing credentials,
/// reading files, waiting for a cache lock, or scheduling a network refresh.
/// Missing, stale, and failed reports are unknown, not zero usage.
pub fn cached_account_usage_snapshot(
    provider: super::MultiAccountProviderKind,
    label: &str,
) -> Option<AccountUsageSnapshot> {
    cached_account_usage_status_snapshot(provider, label).filter(|usage| usage.error.is_none())
}

/// Read fresh account-scoped usage, including failed probes for status indicators.
/// A failed probe must not be interpreted as zero usage or an exhausted quota.
/// Like the quota-only accessor, this never probes or blocks the render thread.
pub fn cached_account_usage_status_snapshot(
    provider: super::MultiAccountProviderKind,
    label: &str,
) -> Option<AccountUsageSnapshot> {
    if let Some(remote) = super::remote_accounts::cached(provider, label) {
        return remote;
    }
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    let key = format!("label:{label}");
    match provider {
        super::MultiAccountProviderKind::Anthropic => {
            let map = ANTHROPIC_USAGE_CACHE.get()?.try_lock().ok()?;
            let data = map.get(&key)?;
            if data.is_stale() {
                return None;
            }
            Some(anthropic_snapshot_from_usage(label.into(), None, data))
        }
        super::MultiAccountProviderKind::OpenAI => {
            let map = OPENAI_ACCOUNT_USAGE_CACHE.get()?.try_lock().ok()?;
            let data = map.get(&key)?;
            if data.is_stale() {
                return None;
            }
            Some(openai_snapshot_from_usage(label.into(), None, data))
        }
    }
}

#[cfg(test)]
mod account_snapshot_tests {
    use super::*;
    use crate::usage::MultiAccountProviderKind;

    #[test]
    fn anthropic_keys_share_saved_identity_without_exposing_token_prefixes() {
        let token = "shared-prefix-20chars-account-a";
        let accounts = [("saved-a", token)];
        let key =
            |label| anthropic_usage_cache_key_with_accounts(token, label, accounts.into_iter());
        assert_eq!(key(Some("saved-a")), "label:saved-a");
        assert_eq!(key(None), key(Some("saved-a")));
        assert_eq!(key(Some("default")), key(None));
        let unknown = anthropic_usage_cache_key_with_accounts(token, None, std::iter::empty());
        let other = anthropic_usage_cache_key_with_accounts(
            "shared-prefix-20chars-account-b",
            None,
            std::iter::empty(),
        );
        assert_ne!(unknown, other);
        assert!(unknown.starts_with("token:sha256:"));
        assert!(!unknown.contains("shared-prefix"));
        assert_eq!(
            unknown,
            anthropic_usage_cache_key_with_accounts(token, Some("default"), std::iter::empty())
        );
    }

    #[test]
    fn anthropic_snapshot_either_shared_window_exhausted_and_errors_unknown() {
        for (five_hour, seven_day, expected) in [
            (1.0, 0.2, true),
            (0.2, 1.0, true),
            (1.0, 1.0, true),
            (0.2, 0.3, false),
        ] {
            let mut usage = UsageData {
                five_hour,
                seven_day,
                fetched_at: Some(Instant::now()),
                ..Default::default()
            };
            let snapshot = anthropic_snapshot_from_usage("scope".into(), None, &usage);
            assert_eq!(snapshot.exhausted, expected);
            assert_eq!(snapshot.five_hour_ratio, Some(five_hour));
            assert_eq!(snapshot.seven_day_ratio, Some(seven_day));
            usage.last_error = Some("transport failed".into());
            let unknown = anthropic_snapshot_from_usage("scope".into(), None, &usage);
            assert!(!unknown.exhausted);
            assert!(unknown.five_hour_ratio.is_none());
            assert!(unknown.seven_day_ratio.is_none());
            assert!(unknown.error.is_some());
        }
    }

    #[test]
    fn cached_account_snapshot_is_scoped_fresh_and_nonblocking() {
        let label = "router-cache-only-test";
        let key = format!("label:{label}");
        let provider = MultiAccountProviderKind::Anthropic;
        let data = UsageData {
            five_hour: 0.42,
            fetched_at: Some(Instant::now()),
            ..Default::default()
        };
        store_anthropic_usage(key.clone(), data.clone());
        let snapshot = cached_account_usage_snapshot(provider, label).unwrap();
        assert_eq!(snapshot.five_hour_ratio, Some(0.42));
        assert_eq!(snapshot.label, label);
        assert!(cached_account_usage_snapshot(provider, "router-other-account").is_none());
        assert!(cached_account_usage_status_snapshot(provider, "router-other-account").is_none());
        assert!(cached_account_usage_status_snapshot(provider, " ").is_none());
        assert!(cached_account_usage_snapshot(MultiAccountProviderKind::OpenAI, label).is_none());
        {
            let _guard = anthropic_usage_cache().lock().unwrap();
            assert!(cached_account_usage_snapshot(provider, label).is_none());
            assert!(cached_account_usage_status_snapshot(provider, label).is_none());
        }
        store_anthropic_usage(
            key.clone(),
            UsageData {
                fetched_at: Some(
                    Instant::now()
                        - super::super::ANTHROPIC_CACHE_DURATION
                        - std::time::Duration::from_secs(1),
                ),
                ..data.clone()
            },
        );
        assert!(cached_account_usage_snapshot(provider, label).is_none());
        assert!(cached_account_usage_status_snapshot(provider, label).is_none());
        store_anthropic_usage(
            key.clone(),
            UsageData {
                last_error: Some("unavailable".into()),
                last_success_at: Some(Instant::now()),
                five_hour_resets_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                ),
                ..data
            },
        );
        assert!(cached_account_usage_snapshot(provider, label).is_none());
        let stale = cached_account_usage_status_snapshot(provider, label).unwrap();
        assert_eq!(stale.error.as_deref(), Some("unavailable"));
        assert_eq!(stale.five_hour_ratio, Some(0.42));
        assert!(!stale.exhausted);
        anthropic_usage_cache().lock().unwrap().remove(&key);
    }
}

pub(super) fn invalidate_openai_usage_after_reset(access_token: &str, account_label: Option<&str>) {
    if let Ok(mut map) = openai_usage_cache().lock() {
        OPENAI_USAGE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if account_label.is_none() && access_token.is_empty() {
            // A daemon cannot receive the TUI's bearer token over the protocol.
            // Legacy/external OAuth caches have token keys, not account labels.
            map.retain(|key, _| !key.starts_with("token:"));
        } else {
            map.remove(&openai_usage_cache_key(access_token, account_label));
            map.remove(&openai_usage_cache_key(access_token, None));
        }
    }
    if let Some(cache) = super::PROVIDER_USAGE_CACHE.get()
        && let Ok(mut map) = cache.lock()
    {
        // Removing just OpenAI can leave an all-fresh map of other providers,
        // causing fetch_all_provider_usage to skip fetching OpenAI entirely.
        // Their per-account caches remain intact, so clearing this aggregate is cheap.
        map.clear();
    }
}

/// Forget one Claude login's quota after a session-limit reset so the next
/// check fetches fresh limits. `None` clears token-keyed (unlabelled) entries.
pub(super) fn invalidate_anthropic_usage_after_reset(account_label: Option<&str>) {
    let label = account_label
        .map(str::trim)
        .filter(|label| !label.is_empty());
    if let Ok(mut map) = anthropic_usage_cache().lock() {
        match label {
            Some(label) => {
                map.remove(&format!("label:{label}"));
            }
            None => {
                map.retain(|key, _| !key.starts_with("token:") && key != "label:default");
            }
        }
    }
    // Never hold the memory mutex while waiting for the on-disk transaction.
    // Fetch completion writes disk first, then memory.
    match label {
        Some(label) => {
            let key = format!("label:{label}");
            super::disk_cache::invalidate(|candidate| candidate == key);
        }
        None => {
            super::disk_cache::invalidate(|key| key.starts_with("token:") || key == "label:default")
        }
    }
    if let Some(cache) = super::PROVIDER_USAGE_CACHE.get()
        && let Ok(mut map) = cache.lock()
    {
        // As for OpenAI: a partially cleared aggregate would skip the refetch.
        map.clear();
    }
}

pub(super) fn anthropic_usage_cache_key(access_token: &str, account_label: Option<&str>) -> String {
    let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
    anthropic_usage_cache_key_with_accounts(
        access_token,
        account_label,
        accounts
            .iter()
            .map(|a| (a.label.as_str(), a.access.as_str())),
    )
}

fn anthropic_usage_cache_key_with_accounts<'a>(
    access_token: &str,
    account_label: Option<&str>,
    accounts: impl Iterator<Item = (&'a str, &'a str)>,
) -> String {
    // Routing passes a token while account polling passes a label. Resolve the
    // exact full token so both paths share freshness and, critically, backoff.
    if !access_token.is_empty()
        && let Some((label, _)) = accounts
            .into_iter()
            .find(|(_, token)| *token == access_token)
    {
        return format!("label:{label}");
    }
    if let Some(label) = account_label
        .map(str::trim)
        .filter(|label| !label.is_empty() && *label != "default")
    {
        return format!("label:{}", label);
    }

    // Unknown/external credentials have no stable saved label. Never persist
    // credential prefixes, and never merge tokens that share the same prefix.
    use sha2::{Digest, Sha256};
    format!(
        "token:sha256:{}",
        hex::encode(Sha256::digest(access_token.as_bytes()))
    )
}

pub(super) fn openai_usage_cache_key(access_token: &str, account_label: Option<&str>) -> String {
    if let Some(label) = account_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        return format!("label:{}", label);
    }

    let prefix = access_token
        .get(..20)
        .unwrap_or(access_token)
        .trim()
        .to_string();
    format!("token:{}", prefix)
}

pub(super) fn cached_anthropic_usage(cache_key: &str) -> Option<UsageData> {
    let cache = anthropic_usage_cache();
    let map = cache.lock().ok()?;
    let cached = map.get(cache_key)?.clone();
    (!cached.is_stale()).then_some(cached)
}

/// Credential refresh callers must respect the shared backoff too, before
/// attempting any network operation with an expired credential.
pub(super) fn cached_or_shared_anthropic_usage(cache_key: &str) -> Option<UsageData> {
    cached_anthropic_usage(cache_key).or_else(|| {
        let data = super::disk_cache::fresh(cache_key)?;
        store_anthropic_usage(cache_key.to_string(), data.clone());
        Some(data)
    })
}

pub(super) fn store_anthropic_usage(cache_key: String, data: UsageData) {
    if let Ok(mut map) = anthropic_usage_cache().lock() {
        map.insert(cache_key, data);
    }
}

pub(super) fn cached_openai_usage(cache_key: &str) -> Option<OpenAIUsageData> {
    let cache = openai_usage_cache();
    let map = cache.lock().ok()?;
    let cached = map.get(cache_key)?.clone();
    (!cached.is_stale()).then_some(cached)
}

#[cfg(test)]
pub(super) fn store_openai_usage(cache_key: String, data: OpenAIUsageData) {
    store_openai_usage_for_generation(openai_usage_generation(), cache_key, data);
}

pub(super) fn store_openai_usage_for_generation(
    generation: u64,
    cache_key: String,
    data: OpenAIUsageData,
) {
    if let Ok(mut map) = openai_usage_cache().lock() {
        // A request begun before a reset must not reinstate the old exhausted quota.
        if generation != openai_usage_generation() {
            return;
        }
        let previous = map.get(&cache_key).cloned();
        let previous_exhausted = previous
            .as_ref()
            .map(OpenAIUsageData::exhausted)
            .unwrap_or(false);
        let current_exhausted = data.exhausted();
        let previous_hard_limit = previous
            .as_ref()
            .map(|usage| usage.hard_limit_reached)
            .unwrap_or(false);
        if previous.is_none()
            || previous_exhausted != current_exhausted
            || previous_hard_limit != data.hard_limit_reached
        {
            crate::logging::info(&format!(
                "OpenAI limit diag: usage cache update key={} prev_exhausted={} new_exhausted={} prev_hard_limit={} new_hard_limit={} snapshot=({})",
                cache_key,
                previous_exhausted,
                current_exhausted,
                previous_hard_limit,
                data.hard_limit_reached,
                data.diagnostic_fields()
            ));
        }
        map.insert(cache_key, data);
    }
}

pub(super) fn provider_report_from_usage_data(
    display_name: String,
    data: &UsageData,
) -> ProviderUsage {
    if let Some(error) = &data.last_error {
        return ProviderUsage {
            provider_name: display_name,
            error: Some(error.clone()),
            ..Default::default()
        };
    }

    let mut limits = Vec::new();
    limits.push(UsageLimit {
        name: "5-hour window".to_string(),
        usage_percent: data.five_hour * 100.0,
        resets_at: data.five_hour_resets_at.clone(),
    });
    limits.push(UsageLimit {
        name: "7-day window".to_string(),
        usage_percent: data.seven_day * 100.0,
        resets_at: data.seven_day_resets_at.clone(),
    });
    if let Some(opus) = data.seven_day_opus {
        limits.push(UsageLimit {
            name: "7-day Opus window".to_string(),
            usage_percent: opus * 100.0,
            resets_at: data.seven_day_resets_at.clone(),
        });
    }
    for window in &data.model_scoped {
        limits.push(UsageLimit {
            name: format!("7-day {} window", window.model_name),
            usage_percent: window.utilization * 100.0,
            resets_at: window.resets_at.clone(),
        });
    }

    let mut extra_info = Vec::new();
    extra_info.push((
        "Extra usage (long context)".to_string(),
        if data.extra_usage_enabled {
            "enabled".to_string()
        } else {
            "disabled".to_string()
        },
    ));

    ProviderUsage {
        provider_name: display_name,
        limits,
        extra_info,
        hard_limit_reached: false,
        openai_reset_credits: None,
        anthropic_limit_reset: None,
        error: None,
        last_used_unix_secs: None,
    }
}

pub(super) fn usage_data_from_provider_report(report: &ProviderUsage) -> UsageData {
    if let Some(error) = &report.error {
        return UsageData {
            fetched_at: Some(Instant::now()),
            last_error: Some(error.clone()),
            ..Default::default()
        };
    }

    let five_hour = report
        .limits
        .iter()
        .find(|limit| limit.name == "5-hour window");
    let seven_day = report
        .limits
        .iter()
        .find(|limit| limit.name == "7-day window");
    let seven_day_opus = report
        .limits
        .iter()
        .find(|limit| limit.name == "7-day Opus window");
    let model_scoped = report
        .limits
        .iter()
        .filter_map(|limit| {
            let model_name = limit.name.strip_prefix("7-day ")?.strip_suffix(" window")?;
            if model_name == "Opus" {
                return None;
            }
            Some(ModelScopedUsageWindow {
                model_name: model_name.to_string(),
                utilization: usage_percent_to_ratio(limit.usage_percent),
                resets_at: limit.resets_at.clone(),
            })
        })
        .collect();
    let extra_usage_enabled = report.extra_info.iter().find_map(|(key, value)| {
        if key == "Extra usage (long context)" {
            Some(value == "enabled")
        } else {
            None
        }
    });

    UsageData {
        five_hour: five_hour
            .map(|limit| usage_percent_to_ratio(limit.usage_percent))
            .unwrap_or(0.0),
        five_hour_resets_at: five_hour.and_then(|limit| limit.resets_at.clone()),
        seven_day: seven_day
            .map(|limit| usage_percent_to_ratio(limit.usage_percent))
            .unwrap_or(0.0),
        seven_day_resets_at: seven_day.and_then(|limit| limit.resets_at.clone()),
        seven_day_opus: seven_day_opus.map(|limit| usage_percent_to_ratio(limit.usage_percent)),
        model_scoped,
        extra_usage_enabled: extra_usage_enabled.unwrap_or(false),
        fetched_at: Some(Instant::now()),
        // Reports do not carry the original probe timestamp or retry deadline.
        // Only a successful report with actual limits proves last-good quota.
        last_success_at: (report.error.is_none() && !report.limits.is_empty()).then(Instant::now),
        refresh_after: None,
        last_error: report.error.clone(),
    }
}

pub(super) fn openai_usage_data_from_provider_report(report: &ProviderUsage) -> OpenAIUsageData {
    let mut data = classify_openai_limits(&report.limits);
    data.hard_limit_reached = report.hard_limit_reached;
    data.openai_reset_credits = if report.error.is_none() {
        report.openai_reset_credits.clone()
    } else {
        None
    };
    data.fetched_at = Some(Instant::now());
    data.last_error = report.error.clone();
    data
}

pub(super) fn provider_report_from_openai_usage_data(
    display_name: String,
    data: &OpenAIUsageData,
) -> ProviderUsage {
    if let Some(error) = &data.last_error {
        return ProviderUsage {
            provider_name: display_name,
            error: Some(error.clone()),
            ..Default::default()
        };
    }

    let mut limits = Vec::new();
    if let Some(window) = &data.five_hour {
        limits.push(UsageLimit {
            name: window.name.clone(),
            usage_percent: window.usage_ratio * 100.0,
            resets_at: window.resets_at.clone(),
        });
    }
    if let Some(window) = &data.seven_day {
        limits.push(UsageLimit {
            name: window.name.clone(),
            usage_percent: window.usage_ratio * 100.0,
            resets_at: window.resets_at.clone(),
        });
    }
    if let Some(window) = &data.spark {
        limits.push(UsageLimit {
            name: window.name.clone(),
            usage_percent: window.usage_ratio * 100.0,
            resets_at: window.resets_at.clone(),
        });
    }

    ProviderUsage {
        provider_name: display_name,
        limits,
        extra_info: Vec::new(),
        hard_limit_reached: data.hard_limit_reached,
        openai_reset_credits: data.openai_reset_credits.clone(),
        anthropic_limit_reset: None,
        error: None,
        last_used_unix_secs: None,
    }
}

pub(super) fn openai_snapshot_from_usage(
    label: String,
    email: Option<String>,
    usage: &OpenAIUsageData,
) -> AccountUsageSnapshot {
    let five_hour_ratio = usage.five_hour.as_ref().map(|window| window.usage_ratio);
    let seven_day_ratio = usage.seven_day.as_ref().map(|window| window.usage_ratio);
    let exhausted = usage.exhausted();

    AccountUsageSnapshot {
        label,
        email,
        exhausted,
        primary_label: usage
            .five_hour
            .as_ref()
            .map(|window| window.name.trim_end_matches(" window").to_string()),
        five_hour_ratio,
        secondary_label: usage
            .seven_day
            .as_ref()
            .map(|window| window.name.trim_end_matches(" window").to_string()),
        seven_day_ratio,
        resets_at: usage
            .five_hour
            .as_ref()
            .and_then(|window| window.resets_at.clone())
            .or_else(|| {
                usage
                    .seven_day
                    .as_ref()
                    .and_then(|window| window.resets_at.clone())
            }),
        error: usage.last_error.clone(),
    }
}

pub(super) fn anthropic_snapshot_from_usage(
    label: String,
    email: Option<String>,
    usage: &UsageData,
) -> AccountUsageSnapshot {
    // Error timestamps alone are not evidence of known quota: failed first
    // probes contain default zeroes. Only a real last-good sample with a
    // still-open, explicitly dated window is safe to show as stale.
    let ratio = |value: f32, reset: Option<&str>| {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return None;
        }
        let future_reset = reset
            .and_then(|reset| chrono::DateTime::parse_from_rfc3339(reset).ok())
            .is_some_and(|reset| reset > chrono::Utc::now());
        if usage.last_error.is_none() {
            // A success can remain cached across a reset while polling backs
            // off. Never route or display the rolled-over window as current.
            return (reset.is_none() || future_reset).then_some(value);
        }
        (usage.last_success_at.is_some() && usage.fetched_at.is_some() && future_reset)
            .then_some(value)
    };
    let five_hour_ratio = ratio(usage.five_hour, usage.five_hour_resets_at.as_deref());
    let seven_day_ratio = ratio(usage.seven_day, usage.seven_day_resets_at.as_deref());
    AccountUsageSnapshot {
        label,
        email,
        exhausted: usage.last_error.is_none()
            && [five_hour_ratio, seven_day_ratio]
                .into_iter()
                .flatten()
                .any(|ratio| ratio >= 0.99),
        primary_label: Some("5h".to_string()),
        five_hour_ratio,
        secondary_label: Some("7d".to_string()),
        seven_day_ratio,
        resets_at: five_hour_ratio
            .and(usage.five_hour_resets_at.clone())
            .or_else(|| seven_day_ratio.and(usage.seven_day_resets_at.clone())),
        error: usage.last_error.clone(),
    }
}

#[cfg(test)]
mod stale_account_snapshot_tests {
    use super::*;

    fn last_good() -> UsageData {
        UsageData {
            five_hour: 1.0,
            seven_day: 0.42,
            five_hour_resets_at: Some(
                (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            ),
            seven_day_resets_at: Some(
                (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
            ),
            fetched_at: Some(Instant::now()),
            last_success_at: Some(Instant::now()),
            last_error: Some("HTTP 429".into()),
            ..Default::default()
        }
    }

    #[test]
    fn stale_limits_preserve_error_but_never_exhaust_or_route() {
        let data = last_good();
        let snapshot = anthropic_snapshot_from_usage("stale-test".into(), None, &data);
        assert_eq!(snapshot.five_hour_ratio, Some(1.0));
        assert_eq!(snapshot.seven_day_ratio, Some(0.42));
        assert_eq!(snapshot.error.as_deref(), Some("HTTP 429"));
        assert!(!snapshot.exhausted);
    }

    #[test]
    fn stale_limits_require_provenance_and_each_unexpired_valid_window() {
        let snapshot = |data: &UsageData| anthropic_snapshot_from_usage("test".into(), None, data);
        let mut data = last_good();
        data.last_success_at = None;
        assert!(snapshot(&data).five_hour_ratio.is_none());
        assert!(snapshot(&data).seven_day_ratio.is_none());
        data = last_good();
        data.fetched_at = None;
        assert!(snapshot(&data).five_hour_ratio.is_none());
        data = last_good();
        for reset in [
            None,
            Some("invalid".into()),
            Some((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()),
        ] {
            data.five_hour_resets_at = reset;
            assert!(snapshot(&data).five_hour_ratio.is_none());
            assert_eq!(snapshot(&data).seven_day_ratio, Some(0.42));
            assert_eq!(snapshot(&data).resets_at, data.seven_day_resets_at);
        }
        data = last_good();
        for ratio in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            data.five_hour = ratio;
            assert!(snapshot(&data).five_hour_ratio.is_none());
        }
    }

    #[test]
    fn successful_but_rolled_over_window_does_not_exhaust() {
        let mut data = last_good();
        data.last_error = None;
        data.five_hour_resets_at =
            Some((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        let snapshot = anthropic_snapshot_from_usage("test".into(), None, &data);
        assert!(snapshot.five_hour_ratio.is_none());
        assert_eq!(snapshot.seven_day_ratio, Some(0.42));
        assert_eq!(snapshot.resets_at, data.seven_day_resets_at);
        assert!(!snapshot.exhausted);
        assert!(snapshot.error.is_none());
    }
}
