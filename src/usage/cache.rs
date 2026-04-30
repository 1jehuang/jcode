use super::openai_helpers::{classify_openai_limits, normalize_ratio};
use super::{AccountUsageSnapshot, OpenAIUsageData, ProviderUsage, UsageData, UsageLimit};
use std::collections::HashMap;
use std::time::Instant;

/// Shared Anthropic usage cache used by the info widget, `/usage`, and
/// multi-account fallback logic so they don't hammer the same endpoint through
/// separate code paths.
static ANTHROPIC_USAGE_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, UsageData>>> =
    std::sync::OnceLock::new();

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

pub(super) fn anthropic_usage_cache_key(access_token: &str, account_label: Option<&str>) -> String {
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

pub(super) fn store_openai_usage(cache_key: String, data: OpenAIUsageData) {
    if let Ok(mut map) = openai_usage_cache().lock() {
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
        error: None,
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
    let extra_usage_enabled = report.extra_info.iter().find_map(|(key, value)| {
        if key == "Extra usage (long context)" {
            Some(value == "enabled")
        } else {
            None
        }
    });

    UsageData {
        five_hour: five_hour
            .map(|limit| normalize_ratio(limit.usage_percent))
            .unwrap_or(0.0),
        five_hour_resets_at: five_hour.and_then(|limit| limit.resets_at.clone()),
        seven_day: seven_day
            .map(|limit| normalize_ratio(limit.usage_percent))
            .unwrap_or(0.0),
        seven_day_resets_at: seven_day.and_then(|limit| limit.resets_at.clone()),
        seven_day_opus: seven_day_opus.map(|limit| normalize_ratio(limit.usage_percent)),
        extra_usage_enabled: extra_usage_enabled.unwrap_or(false),
        fetched_at: Some(Instant::now()),
        last_error: None,
    }
}

pub(super) fn openai_usage_data_from_provider_report(report: &ProviderUsage) -> OpenAIUsageData {
    let mut data = classify_openai_limits(&report.limits);
    data.hard_limit_reached = report.hard_limit_reached;
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
        error: None,
    }
}

pub(super) fn openai_snapshot_from_usage(
    label: String,
    email: Option<String>,
    usage: &OpenAIUsageData,
) -> AccountUsageSnapshot {
    let five_hour_ratio = usage.five_hour.as_ref().map(|window| window.usage_ratio);
    let seven_day_ratio = usage.seven_day.as_ref().map(|window| window.usage_ratio);
    let exhausted = usage.has_limits()
        && five_hour_ratio.map(|ratio| ratio >= 0.99).unwrap_or(false)
        && seven_day_ratio.map(|ratio| ratio >= 0.99).unwrap_or(false);

    AccountUsageSnapshot {
        label,
        email,
        exhausted,
        five_hour_ratio,
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
        five_hour_ratio: Some(usage.five_hour),
        seven_day_ratio: Some(usage.seven_day),
        resets_at: usage
            .five_hour_resets_at
            .clone()
            .or_else(|| usage.seven_day_resets_at.clone()),
        error: usage.last_error.clone(),
    }
}
