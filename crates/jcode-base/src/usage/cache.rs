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
    if let Ok(mut map) = anthropic_usage_cache().lock() {
        match account_label
            .map(str::trim)
            .filter(|label| !label.is_empty())
        {
            Some(label) => {
                let prefix = anthropic_label_key_prefix(label);
                map.retain(|key, _| !key.starts_with(&prefix));
                super::disk_cache::invalidate(|key| key.starts_with(&prefix));
            }
            None => {
                let default_prefix = anthropic_label_key_prefix("default");
                map.retain(|key, _| {
                    !key.starts_with("token:") && !key.starts_with(&default_prefix)
                });
                super::disk_cache::invalidate(|key| {
                    key.starts_with("token:") || key.starts_with(&default_prefix)
                });
            }
        }
    }
    if let Some(cache) = super::PROVIDER_USAGE_CACHE.get()
        && let Ok(mut map) = cache.lock()
    {
        // As for OpenAI: a partially cleared aggregate would skip the refetch.
        map.clear();
    }
}

/// Key prefix shared by every credential stored under one Claude label.
fn anthropic_label_key_prefix(label: &str) -> String {
    format!("label:{label}:")
}

/// Short, non-reversible fingerprint of an access token.
fn anthropic_token_fingerprint(access_token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(access_token.trim().as_bytes());
    format!("{:x}", digest)[..16].to_string()
}

/// Usage is keyed by the label *and* the credential. A relogin that reuses a
/// label (often "default") for a different Claude account must not inherit
/// the old account's exhausted limits or its shared 429 backoff, while the
/// same credential keeps one key so the cross-process backoff still applies.
pub(super) fn anthropic_usage_cache_key(access_token: &str, account_label: Option<&str>) -> String {
    let fingerprint = anthropic_token_fingerprint(access_token);
    match account_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        Some(label) => format!("{}{fingerprint}", anthropic_label_key_prefix(label)),
        None => format!("token:{fingerprint}"),
    }
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

/// Fresh cached usage for a Claude account label (any credential under it).
pub(super) fn cached_anthropic_usage_for_label(label: &str) -> Option<UsageData> {
    let prefix = anthropic_label_key_prefix(label);
    let map = anthropic_usage_cache().lock().ok()?;
    map.iter()
        .filter(|(key, data)| key.starts_with(&prefix) && !data.is_stale())
        .max_by_key(|(_, data)| data.fetched_at)
        .map(|(_, data)| data.clone())
}

/// Fresh cached usage for an OpenAI account label.
pub(super) fn cached_openai_usage_for_label(label: &str) -> Option<OpenAIUsageData> {
    cached_openai_usage(&openai_usage_cache_key("", Some(label)))
}

/// Forget one OpenAI label's cached usage so the next read refetches it.
pub(super) fn forget_openai_usage_for_label(label: &str) {
    if let Ok(mut map) = openai_usage_cache().lock() {
        map.remove(&openai_usage_cache_key("", Some(label)));
    }
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
    // A label whose fresh usage confirms the limit is open again. Cleared
    // after the cache lock is released to keep lock order flat.
    let mut reset_label: Option<String> = None;
    if let Ok(mut map) = openai_usage_cache().lock() {
        // A request begun before a reset must not reinstate the old exhausted quota.
        if generation != openai_usage_generation() {
            return;
        }
        if data.confirms_usage_available()
            && let Some(label) = cache_key.strip_prefix("label:")
        {
            reset_label = Some(label.to_string());
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
    if let Some(label) = reset_label {
        // Fresh usage says this account is open again (an early or banked
        // reset, or the window rolled over). Drop the usage-limit mark and
        // the provider cooldown so the next resume is actually sent.
        crate::provider::clear_openai_provider_unavailability_for_account_label(Some(&label));
    }
}

pub(super) fn anthropic_usage_error(err_msg: String) -> UsageData {
    UsageData {
        fetched_at: Some(Instant::now()),
        last_error: Some(err_msg),
        ..Default::default()
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
        last_error: None,
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
    AccountUsageSnapshot {
        label,
        email,
        exhausted: usage.five_hour >= 0.99 && usage.seven_day >= 0.99,
        primary_label: Some("5h".to_string()),
        five_hour_ratio: Some(usage.five_hour),
        secondary_label: Some("7d".to_string()),
        seven_day_ratio: Some(usage.seven_day),
        resets_at: usage
            .five_hour_resets_at
            .clone()
            .or_else(|| usage.seven_day_resets_at.clone()),
        error: usage.last_error.clone(),
    }
}
