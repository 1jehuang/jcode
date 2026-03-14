//! Subscription usage tracking
//!
//! Fetches usage information from Anthropic's OAuth usage endpoint
//! and OpenAI's ChatGPT wham/usage endpoint.

use crate::auth;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Usage API endpoint
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// OpenAI ChatGPT usage endpoint
const OPENAI_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// Cache duration (refresh every 5 minutes - usage data is slow-changing)
const CACHE_DURATION: Duration = Duration::from_secs(300);

/// Error backoff duration (wait 5 minutes before retrying after auth/credential errors)
const ERROR_BACKOFF: Duration = Duration::from_secs(300);

/// Rate limit backoff duration (wait 15 minutes before retrying after 429 errors)
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(900);

fn mask_email(email: &str) -> String {
    let trimmed = email.trim();
    let Some((local, domain)) = trimmed.split_once('@') else {
        return trimmed.to_string();
    };

    if local.is_empty() {
        return format!("***@{}", domain);
    }

    let mut chars = local.chars();
    let first = chars.next().unwrap_or('*');
    let last = chars.last().unwrap_or(first);

    let masked_local = if local.chars().count() <= 2 {
        format!("{}*", first)
    } else {
        format!("{}***{}", first, last)
    };

    format!("{}@{}", masked_local, domain)
}

fn openai_provider_display_name(
    label: &str,
    email: Option<&str>,
    account_count: usize,
    is_active: bool,
) -> String {
    let email_suffix = email
        .map(mask_email)
        .map(|masked| format!(" ({})", masked))
        .unwrap_or_default();

    if account_count <= 1 {
        format!("OpenAI (ChatGPT){}", email_suffix)
    } else {
        let active_marker = if is_active { " ✦" } else { "" };
        format!("OpenAI - {}{}{}", label, email_suffix, active_marker)
    }
}

/// Usage data from the API
#[derive(Debug, Clone, Default)]
pub struct UsageData {
    /// Five-hour window utilization (0.0-1.0)
    pub five_hour: f32,
    /// Five-hour reset time (ISO timestamp)
    pub five_hour_resets_at: Option<String>,
    /// Seven-day window utilization (0.0-1.0)
    pub seven_day: f32,
    /// Seven-day reset time (ISO timestamp)
    pub seven_day_resets_at: Option<String>,
    /// Seven-day Opus utilization (0.0-1.0)
    pub seven_day_opus: Option<f32>,
    /// Whether extra usage (long context, etc.) is enabled
    pub extra_usage_enabled: bool,
    /// Last fetch time
    pub fetched_at: Option<Instant>,
    /// Last error (if any)
    pub last_error: Option<String>,
}

impl UsageData {
    /// Check if data is stale and should be refreshed
    pub fn is_stale(&self) -> bool {
        match self.fetched_at {
            Some(t) => {
                let ttl = if self.is_rate_limited() {
                    RATE_LIMIT_BACKOFF
                } else if self.last_error.is_some() {
                    ERROR_BACKOFF
                } else {
                    CACHE_DURATION
                };
                t.elapsed() > ttl
            }
            None => true,
        }
    }

    /// Check if the last error was a rate limit (429)
    fn is_rate_limited(&self) -> bool {
        self.last_error
            .as_ref()
            .map(|e| e.contains("429") || e.contains("rate limit") || e.contains("Rate limited"))
            .unwrap_or(false)
    }

    /// Format five-hour usage as percentage string
    pub fn five_hour_percent(&self) -> String {
        format!("{:.0}%", self.five_hour * 100.0)
    }

    /// Format seven-day usage as percentage string
    pub fn seven_day_percent(&self) -> String {
        format!("{:.0}%", self.seven_day * 100.0)
    }
}

/// API response structures
#[derive(Deserialize, Debug)]
struct UsageResponse {
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
    seven_day_opus: Option<UsageWindow>,
    extra_usage: Option<ExtraUsageResponse>,
}

#[derive(Deserialize, Debug)]
struct UsageWindow {
    utilization: Option<f32>,
    resets_at: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ExtraUsageResponse {
    is_enabled: Option<bool>,
}

// ─── Combined usage for /usage command ───────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ProviderUsage {
    pub provider_name: String,
    pub limits: Vec<UsageLimit>,
    pub extra_info: Vec<(String, String)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UsageLimit {
    pub name: String,
    pub usage_percent: f32,
    pub resets_at: Option<String>,
}

/// Normalized OpenAI/Codex usage window info used by the TUI widget.
#[derive(Debug, Clone, Default)]
pub struct OpenAIUsageWindow {
    pub name: String,
    /// Utilization as a fraction in [0.0, 1.0].
    pub usage_ratio: f32,
    pub resets_at: Option<String>,
}

/// Cached OpenAI/Codex usage snapshot for info widgets.
#[derive(Debug, Clone, Default)]
pub struct OpenAIUsageData {
    pub five_hour: Option<OpenAIUsageWindow>,
    pub seven_day: Option<OpenAIUsageWindow>,
    pub spark: Option<OpenAIUsageWindow>,
    pub fetched_at: Option<Instant>,
    pub last_error: Option<String>,
}

impl OpenAIUsageData {
    pub fn is_stale(&self) -> bool {
        match self.fetched_at {
            Some(t) => {
                let ttl = if self.is_rate_limited() {
                    RATE_LIMIT_BACKOFF
                } else if self.last_error.is_some() {
                    ERROR_BACKOFF
                } else {
                    CACHE_DURATION
                };
                t.elapsed() > ttl
            }
            None => true,
        }
    }

    fn is_rate_limited(&self) -> bool {
        self.last_error
            .as_ref()
            .map(|e| e.contains("429") || e.contains("rate limit") || e.contains("Rate limited"))
            .unwrap_or(false)
    }

    pub fn has_limits(&self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some() || self.spark.is_some()
    }
}

/// Cached provider usage reports (used by /usage command).
/// Keyed by provider display name.
static PROVIDER_USAGE_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, (Instant, ProviderUsage)>>,
> = std::sync::OnceLock::new();

/// Shared Anthropic usage cache used by the info widget, `/usage`, and
/// multi-account fallback logic so they don't hammer the same endpoint through
/// separate code paths.
static ANTHROPIC_USAGE_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, UsageData>>> =
    std::sync::OnceLock::new();

/// Minimum interval between /usage command fetches (per provider).
const PROVIDER_USAGE_CACHE_TTL: Duration = Duration::from_secs(120);

fn anthropic_usage_cache() -> &'static std::sync::Mutex<HashMap<String, UsageData>> {
    ANTHROPIC_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn anthropic_usage_cache_key(access_token: &str, account_label: Option<&str>) -> String {
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

fn cached_anthropic_usage(cache_key: &str) -> Option<UsageData> {
    let cache = anthropic_usage_cache();
    let map = cache.lock().ok()?;
    let cached = map.get(cache_key)?.clone();
    (!cached.is_stale()).then_some(cached)
}

fn store_anthropic_usage(cache_key: String, data: UsageData) {
    if let Ok(mut map) = anthropic_usage_cache().lock() {
        map.insert(cache_key, data);
    }
}

fn anthropic_usage_error(err_msg: String) -> UsageData {
    UsageData {
        fetched_at: Some(Instant::now()),
        last_error: Some(err_msg),
        ..Default::default()
    }
}

fn provider_report_from_usage_data(display_name: String, data: &UsageData) -> ProviderUsage {
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
        error: None,
    }
}

async fn fetch_anthropic_usage_data(access_token: String, cache_key: String) -> Result<UsageData> {
    if let Some(cached) = cached_anthropic_usage(&cache_key) {
        return Ok(cached);
    }

    let client = crate::provider::shared_http_client();
    let response = client
        .get(USAGE_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", "claude-cli/1.0.0")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("anthropic-beta", "oauth-2025-04-20,claude-code-20250219")
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(e) => {
            let err = anthropic_usage_error(format!("Failed to fetch usage data: {}", e));
            store_anthropic_usage(cache_key, err.clone());
            anyhow::bail!(
                err.last_error
                    .unwrap_or_else(|| "Failed to fetch usage data".into())
            );
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        let err = anthropic_usage_error(format!("Usage API error ({}): {}", status, error_text));
        store_anthropic_usage(cache_key, err.clone());
        anyhow::bail!(err.last_error.unwrap_or_else(|| "Usage API error".into()));
    }

    let data: UsageResponse = response
        .json()
        .await
        .context("Failed to parse usage response")?;

    let usage = UsageData {
        five_hour: data
            .five_hour
            .as_ref()
            .and_then(|w| w.utilization)
            .map(|u| u / 100.0)
            .unwrap_or(0.0),
        five_hour_resets_at: data.five_hour.as_ref().and_then(|w| w.resets_at.clone()),
        seven_day: data
            .seven_day
            .as_ref()
            .and_then(|w| w.utilization)
            .map(|u| u / 100.0)
            .unwrap_or(0.0),
        seven_day_resets_at: data.seven_day.as_ref().and_then(|w| w.resets_at.clone()),
        seven_day_opus: data
            .seven_day_opus
            .as_ref()
            .and_then(|w| w.utilization)
            .map(|u| u / 100.0),
        extra_usage_enabled: data
            .extra_usage
            .as_ref()
            .and_then(|e| e.is_enabled)
            .unwrap_or(false),
        fetched_at: Some(Instant::now()),
        last_error: None,
    };

    store_anthropic_usage(cache_key, usage.clone());
    Ok(usage)
}

/// Fetch usage from all connected providers with OAuth credentials.
/// Returns a list of ProviderUsage, one per provider that has credentials.
/// Results are cached for 2 minutes to avoid hitting rate limits.
pub async fn fetch_all_provider_usage() -> Vec<ProviderUsage> {
    let cache = PROVIDER_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));

    let now = Instant::now();
    let all_fresh = if let Ok(map) = cache.lock() {
        !map.is_empty()
            && map.values().all(|(fetched_at, report)| {
                let ttl = if report
                    .error
                    .as_ref()
                    .map(|e| {
                        e.contains("429") || e.contains("rate limit") || e.contains("Rate limited")
                    })
                    .unwrap_or(false)
                {
                    RATE_LIMIT_BACKOFF
                } else {
                    PROVIDER_USAGE_CACHE_TTL
                };
                now.duration_since(*fetched_at) < ttl
            })
    } else {
        false
    };

    if all_fresh {
        if let Ok(map) = cache.lock() {
            return map.values().map(|(_, r)| r.clone()).collect();
        }
    }

    let mut results = Vec::new();

    let (anthropic_results, openai_results, openrouter, copilot) = tokio::join!(
        fetch_all_anthropic_usage_reports(),
        fetch_all_openai_usage_reports(),
        fetch_openrouter_usage_report(),
        fetch_copilot_usage_report(),
    );

    results.extend(anthropic_results);
    results.extend(openai_results);
    if let Some(r) = openrouter {
        results.push(r);
    }
    if let Some(r) = copilot {
        results.push(r);
    }

    sync_cached_usage_from_reports(&results).await;

    if let Ok(mut map) = cache.lock() {
        map.clear();
        let now = Instant::now();
        for r in &results {
            map.insert(r.provider_name.clone(), (now, r.clone()));
        }
    }

    results
}

async fn sync_cached_usage_from_reports(results: &[ProviderUsage]) {
    sync_active_anthropic_usage_from_reports(results).await;
    sync_openai_usage_from_reports(results).await;
}

async fn sync_active_anthropic_usage_from_reports(results: &[ProviderUsage]) {
    let report = active_anthropic_usage_report(results);
    let usage = get_usage().await;
    let mut cached = usage.write().await;

    match report {
        Some(report) => {
            let usage_data = usage_data_from_provider_report(report);
            if let Ok(creds) = auth::claude::load_credentials() {
                let cache_key = anthropic_usage_cache_key(
                    &creds.access_token,
                    auth::claude::active_account_label().as_deref(),
                );
                store_anthropic_usage(cache_key, usage_data.clone());
            }
            *cached = usage_data;
            if report.error.is_none() {
                crate::provider::clear_provider_unavailable_for_account("claude");
            }
        }
        None => {
            *cached = UsageData {
                fetched_at: Some(Instant::now()),
                last_error: Some("No Anthropic OAuth credentials found".to_string()),
                ..Default::default()
            };
        }
    }
}

async fn sync_openai_usage_from_reports(results: &[ProviderUsage]) {
    let report = active_openai_usage_report(results);
    let usage = get_openai_usage_cell().await;
    let mut cached = usage.write().await;

    match report {
        Some(report) => {
            *cached = openai_usage_data_from_provider_report(report);
            if report.error.is_none() {
                crate::provider::clear_provider_unavailable_for_account("openai");
            }
        }
        None => {
            *cached = OpenAIUsageData {
                fetched_at: Some(Instant::now()),
                last_error: Some("No OpenAI/Codex OAuth credentials found".to_string()),
                ..Default::default()
            };
        }
    }
}

fn active_anthropic_usage_report(results: &[ProviderUsage]) -> Option<&ProviderUsage> {
    let mut anthropic_reports = results
        .iter()
        .filter(|report| report.provider_name.starts_with("Anthropic"));

    let first = anthropic_reports.next()?;
    if !first.provider_name.contains(" - ") {
        return Some(first);
    }

    results
        .iter()
        .find(|report| {
            report.provider_name.starts_with("Anthropic") && report.provider_name.contains(" ✦")
        })
        .or(Some(first))
}

fn active_openai_usage_report(results: &[ProviderUsage]) -> Option<&ProviderUsage> {
    let accounts = auth::codex::list_accounts().unwrap_or_default();
    if accounts.is_empty() {
        return results
            .iter()
            .find(|report| report.provider_name.starts_with("OpenAI (ChatGPT)"));
    }

    let active_label = auth::codex::active_account_label();
    let active_account = active_label.as_deref().and_then(|label| {
        accounts
            .iter()
            .find(|account| account.label == label)
            .or_else(|| accounts.first())
    });

    let expected_name = active_account.map(|account| {
        openai_provider_display_name(
            &account.label,
            account.email.as_deref(),
            accounts.len(),
            accounts.len() > 1,
        )
    });

    expected_name
        .as_deref()
        .and_then(|name| results.iter().find(|report| report.provider_name == name))
        .or_else(|| {
            results
                .iter()
                .find(|report| report.provider_name.starts_with("OpenAI"))
        })
}

fn usage_data_from_provider_report(report: &ProviderUsage) -> UsageData {
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

fn openai_usage_data_from_provider_report(report: &ProviderUsage) -> OpenAIUsageData {
    let mut data = classify_openai_limits(&report.limits);
    data.fetched_at = Some(Instant::now());
    data.last_error = report.error.clone();
    data
}

fn normalize_ratio(raw: f32) -> f32 {
    if !raw.is_finite() {
        return 0.0;
    }
    if raw > 1.0 {
        (raw / 100.0).clamp(0.0, 1.0)
    } else {
        raw.clamp(0.0, 1.0)
    }
}

fn normalize_percent(raw: f32) -> f32 {
    normalize_ratio(raw) * 100.0
}

fn normalize_limit_key(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn limit_mentions_five_hour(key: &str) -> bool {
    key.contains("5 hour")
        || key.contains("5hr")
        || key.contains("5 h")
        || key.contains("five hour")
}

fn limit_mentions_weekly(key: &str) -> bool {
    key.contains("weekly")
        || key.contains("1 week")
        || key.contains("1w")
        || key.contains("7 day")
        || key.contains("seven day")
}

fn limit_mentions_spark(key: &str) -> bool {
    key.contains("spark")
}

fn to_openai_window(limit: &UsageLimit) -> OpenAIUsageWindow {
    OpenAIUsageWindow {
        name: limit.name.clone(),
        usage_ratio: normalize_ratio(limit.usage_percent),
        resets_at: limit.resets_at.clone(),
    }
}

fn classify_openai_limits(limits: &[UsageLimit]) -> OpenAIUsageData {
    let mut five_hour: Option<OpenAIUsageWindow> = None;
    let mut seven_day: Option<OpenAIUsageWindow> = None;
    let mut spark: Option<OpenAIUsageWindow> = None;
    let mut generic_non_spark: Vec<OpenAIUsageWindow> = Vec::new();

    for limit in limits {
        let key = normalize_limit_key(&limit.name);
        let window = to_openai_window(limit);
        let is_spark = limit_mentions_spark(&key);

        if is_spark && spark.is_none() {
            spark = Some(window.clone());
        }

        if !is_spark {
            if limit_mentions_five_hour(&key) && five_hour.is_none() {
                five_hour = Some(window.clone());
            }
            if limit_mentions_weekly(&key) && seven_day.is_none() {
                seven_day = Some(window.clone());
            }
            generic_non_spark.push(window);
        }
    }

    if five_hour.is_none() {
        five_hour = generic_non_spark.first().cloned();
    }
    if seven_day.is_none() {
        seven_day = generic_non_spark
            .iter()
            .find(|w| {
                five_hour
                    .as_ref()
                    .map(|f| f.name != w.name || f.resets_at != w.resets_at)
                    .unwrap_or(true)
            })
            .cloned();
    }

    OpenAIUsageData {
        five_hour,
        seven_day,
        spark,
        ..Default::default()
    }
}

fn parse_f32_value(value: &serde_json::Value) -> Option<f32> {
    if let Some(n) = value.as_f64() {
        return Some(n as f32);
    }
    value.as_str().and_then(|s| s.trim().parse::<f32>().ok())
}

fn parse_usage_percent_from_obj(obj: &serde_json::Map<String, serde_json::Value>) -> Option<f32> {
    for key in [
        "usage",
        "utilization",
        "usage_percent",
        "used_percent",
        "percent_used",
        "usage_ratio",
        "used_ratio",
    ] {
        if let Some(value) = obj.get(key).and_then(parse_f32_value) {
            return Some(normalize_percent(value));
        }
    }

    let used = obj.get("used").and_then(parse_f32_value);
    let remaining = obj.get("remaining").and_then(parse_f32_value);
    let limit = obj
        .get("limit")
        .or_else(|| obj.get("max"))
        .and_then(parse_f32_value);

    if let (Some(used), Some(limit)) = (used, limit) {
        if limit > 0.0 {
            return Some(((used / limit) * 100.0).clamp(0.0, 100.0));
        }
    }

    if let (Some(remaining), Some(limit)) = (remaining, limit) {
        if limit > 0.0 {
            let used = (limit - remaining).max(0.0);
            return Some(((used / limit) * 100.0).clamp(0.0, 100.0));
        }
    }

    None
}

fn parse_resets_at_from_obj(obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for key in [
        "resets_at",
        "reset_at",
        "resetsAt",
        "resetAt",
        "reset_time",
        "resetTime",
    ] {
        if let Some(value) = obj.get(key).and_then(|v| v.as_str()) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn parse_limit_name(entry: &serde_json::Value, fallback: &str) -> String {
    entry
        .get("name")
        .or_else(|| entry.get("label"))
        .or_else(|| entry.get("display_name"))
        .or_else(|| entry.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or(fallback)
        .to_string()
}

async fn fetch_all_anthropic_usage_reports() -> Vec<ProviderUsage> {
    let accounts = match auth::claude::list_accounts() {
        Ok(a) if !a.is_empty() => a,
        _ => match auth::claude::load_credentials() {
            Ok(creds) if !creds.access_token.is_empty() => {
                return vec![
                    fetch_anthropic_usage_for_token(
                        "Anthropic (Claude)".to_string(),
                        creds.access_token.clone(),
                        creds.refresh_token.clone(),
                        "default".to_string(),
                        creds.expires_at,
                    )
                    .await,
                ];
            }
            _ => return Vec::new(),
        },
    };

    let active_label = auth::claude::active_account_label();
    let mut futures = Vec::new();
    for account in &accounts {
        let label = if accounts.len() > 1 {
            let active_marker = if active_label.as_deref() == Some(&account.label) {
                " ✦"
            } else {
                ""
            };
            let email_suffix = account
                .email
                .as_deref()
                .map(mask_email)
                .map(|m| format!(" ({})", m))
                .unwrap_or_default();
            format!(
                "Anthropic - {}{}{}",
                account.label, email_suffix, active_marker
            )
        } else {
            let email_suffix = account
                .email
                .as_deref()
                .map(mask_email)
                .map(|m| format!(" ({})", m))
                .unwrap_or_default();
            format!("Anthropic (Claude){}", email_suffix)
        };
        let access = account.access.clone();
        let refresh = account.refresh.clone();
        let account_label = account.label.clone();
        let expires = account.expires;
        futures.push(fetch_anthropic_usage_for_token(
            label,
            access,
            refresh,
            account_label,
            expires,
        ));
    }

    let mut results = Vec::new();
    for fut in futures {
        results.push(fut.await);
    }
    results
}

async fn fetch_anthropic_usage_for_token(
    display_name: String,
    access_token: String,
    refresh_token: String,
    account_label: String,
    expires_at: i64,
) -> ProviderUsage {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let access_token = if expires_at < now_ms + 300_000 && !refresh_token.is_empty() {
        match crate::auth::oauth::refresh_claude_tokens_for_account(&refresh_token, &account_label)
            .await
        {
            Ok(refreshed) => refreshed.access_token,
            Err(_) => {
                if expires_at < now_ms {
                    return ProviderUsage {
                        provider_name: display_name,
                        error: Some(
                            "OAuth token expired - use `/login claude` to re-authenticate"
                                .to_string(),
                        ),
                        ..Default::default()
                    };
                }
                access_token
            }
        }
    } else {
        access_token
    };

    let cache_key = anthropic_usage_cache_key(&access_token, Some(&account_label));
    match fetch_anthropic_usage_data(access_token, cache_key).await {
        Ok(data) => provider_report_from_usage_data(display_name, &data),
        Err(e) => ProviderUsage {
            provider_name: display_name,
            error: Some(e.to_string()),
            ..Default::default()
        },
    }
}

async fn fetch_all_openai_usage_reports() -> Vec<ProviderUsage> {
    let accounts = auth::codex::list_accounts().unwrap_or_default();
    if !accounts.is_empty() {
        let active_label = auth::codex::active_account_label();
        let mut reports = Vec::with_capacity(accounts.len());
        for account in &accounts {
            let display_name = openai_provider_display_name(
                &account.label,
                account.email.as_deref(),
                accounts.len(),
                active_label.as_deref() == Some(&account.label),
            );
            reports.push(
                fetch_openai_usage_for_account(
                    display_name,
                    auth::codex::CodexCredentials {
                        access_token: account.access_token.clone(),
                        refresh_token: account.refresh_token.clone(),
                        id_token: account.id_token.clone(),
                        account_id: account.account_id.clone(),
                        expires_at: account.expires_at,
                    },
                    Some(account.label.as_str()),
                )
                .await,
            );
        }
        return reports;
    }

    let creds = match auth::codex::load_credentials() {
        Ok(creds) => creds,
        Err(_) => return Vec::new(),
    };
    let is_chatgpt = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    if !is_chatgpt || creds.access_token.is_empty() {
        return Vec::new();
    }

    vec![
        fetch_openai_usage_for_account(
            openai_provider_display_name("default", None, 1, true),
            creds,
            None,
        )
        .await,
    ]
}

async fn fetch_openai_usage_report() -> Option<ProviderUsage> {
    let reports = fetch_all_openai_usage_reports().await;
    active_openai_usage_report(&reports)
        .cloned()
        .or_else(|| reports.into_iter().next())
}

async fn fetch_openai_usage_for_account(
    display_name: String,
    mut creds: auth::codex::CodexCredentials,
    account_label: Option<&str>,
) -> ProviderUsage {
    let is_chatgpt = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    if creds.access_token.is_empty() || !is_chatgpt {
        return ProviderUsage {
            provider_name: display_name,
            error: Some("No OpenAI/Codex OAuth credentials found".to_string()),
            ..Default::default()
        };
    }

    if let Some(expires_at) = creds.expires_at {
        let now = chrono::Utc::now().timestamp_millis();
        if expires_at < now + 300_000 && !creds.refresh_token.is_empty() {
            let refreshed = match account_label {
                Some(label) => {
                    crate::auth::oauth::refresh_openai_tokens_for_account(
                        &creds.refresh_token,
                        label,
                    )
                    .await
                }
                None => crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await,
            };
            match refreshed {
                Ok(refreshed) => {
                    creds.access_token = refreshed.access_token;
                    creds.refresh_token = refreshed.refresh_token;
                    creds.id_token = refreshed.id_token.or(creds.id_token);
                    creds.account_id = creds.account_id.clone().or_else(|| {
                        creds
                            .id_token
                            .as_deref()
                            .and_then(auth::codex::extract_account_id)
                    });
                    creds.expires_at = Some(refreshed.expires_at);
                }
                Err(e) => {
                    return ProviderUsage {
                        provider_name: display_name,
                        error: Some(format!(
                            "Token refresh failed: {} - use `/login openai` to re-authenticate",
                            e
                        )),
                        ..Default::default()
                    };
                }
            }
        }
    }

    let client = crate::provider::shared_http_client();
    let mut builder = client
        .get(OPENAI_USAGE_URL)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", creds.access_token));

    if let Some(ref account_id) = creds.account_id {
        builder = builder.header("chatgpt-account-id", account_id);
    }

    let response = match builder.send().await {
        Ok(response) => response,
        Err(e) => {
            return ProviderUsage {
                provider_name: display_name,
                error: Some(format!("Failed to fetch: {}", e)),
                ..Default::default()
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return ProviderUsage {
            provider_name: display_name,
            error: Some(format!("API error ({}): {}", status, body)),
            ..Default::default()
        };
    }

    let body_text = match response.text().await {
        Ok(text) => text,
        Err(e) => {
            return ProviderUsage {
                provider_name: display_name,
                error: Some(format!("Failed to read response: {}", e)),
                ..Default::default()
            };
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&body_text) {
        Ok(value) => value,
        Err(e) => {
            return ProviderUsage {
                provider_name: display_name,
                error: Some(format!("Failed to parse response: {}", e)),
                ..Default::default()
            };
        }
    };

    let mut limits = Vec::new();
    let mut extra_info = Vec::new();

    fn parse_wham_window(window: &serde_json::Value, name: &str) -> Option<UsageLimit> {
        let obj = window.as_object()?;
        let used_percent = obj.get("used_percent").and_then(parse_f32_value)?;
        let resets_at = obj.get("reset_at").and_then(parse_f32_value).map(|ts| {
            chrono::DateTime::from_timestamp(ts as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| format!("{}", ts as i64))
        });
        Some(UsageLimit {
            name: name.to_string(),
            usage_percent: used_percent,
            resets_at,
        })
    }

    fn parse_wham_rate_limit(
        rl: &serde_json::Value,
        primary_name: &str,
        secondary_name: &str,
    ) -> Vec<UsageLimit> {
        let mut out = Vec::new();
        if let Some(pw) = rl.get("primary_window") {
            if let Some(limit) = parse_wham_window(pw, primary_name) {
                out.push(limit);
            }
        }
        if let Some(sw) = rl.get("secondary_window") {
            if !sw.is_null() {
                if let Some(limit) = parse_wham_window(sw, secondary_name) {
                    out.push(limit);
                }
            }
        }
        out
    }

    if let Some(rl) = json.get("rate_limit") {
        limits.extend(parse_wham_rate_limit(rl, "5-hour window", "7-day window"));
    }

    if let Some(additional) = json
        .get("additional_rate_limits")
        .and_then(|v| v.as_array())
    {
        for entry in additional {
            let limit_name = entry
                .get("limit_name")
                .and_then(|v| v.as_str())
                .unwrap_or("Additional");
            if let Some(rl) = entry.get("rate_limit") {
                let primary = format!("{} (5h)", limit_name);
                let secondary = format!("{} (7d)", limit_name);
                limits.extend(parse_wham_rate_limit(rl, &primary, &secondary));
            }
        }
    }

    if limits.is_empty()
        && let Some(rate_limits) = json.get("rate_limits").and_then(|v| v.as_array())
    {
        for entry in rate_limits {
            if let Some(obj) = entry.as_object()
                && let Some(usage_percent) = parse_usage_percent_from_obj(obj)
            {
                limits.push(UsageLimit {
                    name: parse_limit_name(entry, "unknown"),
                    usage_percent,
                    resets_at: parse_resets_at_from_obj(obj),
                });
            }
        }
    }

    if limits.is_empty()
        && let Some(obj) = json.as_object()
    {
        for (key, value) in obj {
            if key == "rate_limits" || key == "rate_limit" || key == "additional_rate_limits" {
                continue;
            }

            if let Some(inner) = value.as_object() {
                if let Some(usage_percent) = parse_usage_percent_from_obj(inner) {
                    limits.push(UsageLimit {
                        name: humanize_key(key),
                        usage_percent,
                        resets_at: parse_resets_at_from_obj(inner),
                    });
                    continue;
                }

                if let Some(windows) = inner.get("rate_limits").and_then(|v| v.as_array()) {
                    for entry in windows {
                        if let Some(entry_obj) = entry.as_object()
                            && let Some(usage_percent) = parse_usage_percent_from_obj(entry_obj)
                        {
                            limits.push(UsageLimit {
                                name: parse_limit_name(entry, key),
                                usage_percent,
                                resets_at: parse_resets_at_from_obj(entry_obj),
                            });
                        }
                    }
                }
            }
        }
    }

    if let Some(plan) = json
        .get("plan_type")
        .or_else(|| json.get("plan"))
        .or_else(|| json.get("subscription_type"))
        .and_then(|v| v.as_str())
    {
        extra_info.insert(0, ("Plan".to_string(), plan.to_string()));
    }

    ProviderUsage {
        provider_name: display_name,
        limits,
        extra_info,
        error: None,
    }
}

async fn fetch_openrouter_usage_report() -> Option<ProviderUsage> {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .or_else(|| {
            let config_path = dirs::config_dir()?.join("jcode").join("openrouter.env");
            crate::storage::harden_secret_file_permissions(&config_path);
            let content = std::fs::read_to_string(config_path).ok()?;
            content
                .lines()
                .find_map(|line| line.strip_prefix("OPENROUTER_API_KEY="))
                .map(|k| k.trim().to_string())
        })
        .filter(|k| !k.is_empty())?;

    let client = crate::provider::shared_http_client();

    let (key_resp, credits_resp) = tokio::join!(
        client
            .get("https://openrouter.ai/api/v1/key")
            .header("Authorization", format!("Bearer {}", api_key))
            .send(),
        client
            .get("https://openrouter.ai/api/v1/credits")
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
    );

    let mut limits = Vec::new();
    let mut extra_info = Vec::new();

    if let Ok(resp) = credits_resp {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(data) = json.get("data") {
                    let total_credits = data
                        .get("total_credits")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let total_usage = data
                        .get("total_usage")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let balance = total_credits - total_usage;

                    if total_credits > 0.0 {
                        let usage_pct = (total_usage / total_credits * 100.0) as f32;
                        limits.push(UsageLimit {
                            name: "Credits".to_string(),
                            usage_percent: usage_pct,
                            resets_at: None,
                        });
                    }

                    extra_info.push((
                        "Balance".to_string(),
                        format!("${:.2} / ${:.2}", balance, total_credits),
                    ));
                }
            }
        }
    }

    if let Ok(resp) = key_resp {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(data) = json.get("data") {
                    let usage_daily = data
                        .get("usage_daily")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let usage_weekly = data
                        .get("usage_weekly")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let usage_monthly = data
                        .get("usage_monthly")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);

                    extra_info.push(("Today".to_string(), format!("${:.2}", usage_daily)));
                    extra_info.push(("This week".to_string(), format!("${:.2}", usage_weekly)));
                    extra_info.push(("This month".to_string(), format!("${:.2}", usage_monthly)));

                    if let Some(limit) = data.get("limit").and_then(|v| v.as_f64()) {
                        let remaining = data
                            .get("limit_remaining")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        let used = limit - remaining;
                        let pct = if limit > 0.0 {
                            (used / limit * 100.0) as f32
                        } else {
                            0.0
                        };
                        limits.push(UsageLimit {
                            name: "Key limit".to_string(),
                            usage_percent: pct,
                            resets_at: None,
                        });
                        extra_info.push((
                            "Key limit".to_string(),
                            format!("${:.2} / ${:.2}", remaining, limit),
                        ));
                    }
                }
            }
        }
    }

    if limits.is_empty() && extra_info.is_empty() {
        return None;
    }

    Some(ProviderUsage {
        provider_name: "OpenRouter".to_string(),
        limits,
        extra_info,
        error: None,
    })
}

async fn fetch_copilot_usage_report() -> Option<ProviderUsage> {
    if !auth::copilot::has_copilot_credentials() {
        return None;
    }

    let github_token = auth::copilot::load_github_token().ok()?;

    let mut limits = Vec::new();
    let mut extra_info = Vec::new();

    // Fetch plan/quota info from the token endpoint
    let client = crate::provider::shared_http_client();
    let api_result = client
        .get(auth::copilot::COPILOT_TOKEN_URL)
        .header("Authorization", format!("token {}", github_token))
        .header("User-Agent", auth::copilot::EDITOR_VERSION)
        .header("Editor-Version", auth::copilot::EDITOR_VERSION)
        .header(
            "Editor-Plugin-Version",
            auth::copilot::EDITOR_PLUGIN_VERSION,
        )
        .header("Accept", "application/json")
        .send()
        .await;

    if let Ok(resp) = api_result {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(sku) = json.get("sku").and_then(|v| v.as_str()) {
                    extra_info.push(("Plan".to_string(), sku.to_string()));
                }

                let reset_date = json
                    .get("limited_user_reset_date")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                if let Some(quotas) = json.get("limited_user_quotas").and_then(|v| v.as_object()) {
                    for (name, value) in quotas {
                        if let Some(obj) = value.as_object() {
                            let used = obj.get("used").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            let limit = obj.get("limit").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            if limit > 0.0 {
                                let pct = (used / limit * 100.0) as f32;
                                limits.push(UsageLimit {
                                    name: format!("{} (remote)", humanize_key(name)),
                                    usage_percent: pct,
                                    resets_at: reset_date.clone(),
                                });
                                extra_info.push((
                                    humanize_key(name),
                                    format!("{} / {} used", used as u64, limit as u64),
                                ));
                            }
                        }
                    }
                }

                if let Some(ref rd) = reset_date {
                    let relative = crate::usage::format_reset_time(rd);
                    extra_info.push(("Resets in".to_string(), relative));
                }
            }
        }
    }

    // Local usage tracking
    let usage = crate::copilot_usage::get_usage();

    extra_info.push((
        "Today".to_string(),
        format!(
            "{} premium + {} agent = {} total ({} in + {} out)",
            usage.today.premium_requests,
            usage
                .today
                .requests
                .saturating_sub(usage.today.premium_requests),
            usage.today.requests,
            format_token_count(usage.today.input_tokens),
            format_token_count(usage.today.output_tokens),
        ),
    ));
    extra_info.push((
        "This month".to_string(),
        format!(
            "{} premium + {} agent = {} total ({} in + {} out)",
            usage.month.premium_requests,
            usage
                .month
                .requests
                .saturating_sub(usage.month.premium_requests),
            usage.month.requests,
            format_token_count(usage.month.input_tokens),
            format_token_count(usage.month.output_tokens),
        ),
    ));
    extra_info.push((
        "All time".to_string(),
        format!(
            "{} premium + {} agent = {} total ({} in + {} out)",
            usage.all_time.premium_requests,
            usage
                .all_time
                .requests
                .saturating_sub(usage.all_time.premium_requests),
            usage.all_time.requests,
            format_token_count(usage.all_time.input_tokens),
            format_token_count(usage.all_time.output_tokens),
        ),
    ));

    Some(ProviderUsage {
        provider_name: "GitHub Copilot".to_string(),
        limits,
        extra_info,
        error: None,
    })
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

fn humanize_key(key: &str) -> String {
    key.replace('_', " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let mut s = c.to_uppercase().to_string();
                    s.push_str(&chars.as_str().to_lowercase());
                    s
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Format a reset timestamp into a human-readable relative time
pub fn format_reset_time(timestamp: &str) -> String {
    if let Ok(reset) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        let now = chrono::Utc::now();
        let duration = reset.signed_duration_since(now);
        if duration.num_seconds() <= 0 {
            return "now".to_string();
        }
        let hours = duration.num_hours();
        let minutes = duration.num_minutes() % 60;
        if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    } else if let Ok(reset) =
        chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.fZ")
    {
        let reset_utc = reset.and_utc();
        let now = chrono::Utc::now();
        let duration = reset_utc.signed_duration_since(now);
        if duration.num_seconds() <= 0 {
            return "now".to_string();
        }
        let hours = duration.num_hours();
        let minutes = duration.num_minutes() % 60;
        if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    } else {
        timestamp.to_string()
    }
}

/// Format a usage bar (e.g. "███░░░░░░░ 42%")
pub fn format_usage_bar(percent: f32, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f32).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    let bar: String = "█".repeat(filled) + &"░".repeat(empty);
    format!("{} {:.0}%", bar, percent)
}

// ─── Existing global tracker (Anthropic only) ────────────────────────────────

/// Global usage tracker
static USAGE: tokio::sync::OnceCell<Arc<RwLock<UsageData>>> = tokio::sync::OnceCell::const_new();
static REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Initialize or get the global usage tracker
async fn get_usage() -> Arc<RwLock<UsageData>> {
    USAGE
        .get_or_init(|| async { Arc::new(RwLock::new(UsageData::default())) })
        .await
        .clone()
}

/// Fetch usage data from the API
async fn fetch_usage() -> Result<UsageData> {
    let creds = auth::claude::load_credentials().context("Failed to load Claude credentials")?;

    let now = chrono::Utc::now().timestamp_millis();
    let active_label =
        auth::claude::active_account_label().unwrap_or_else(|| "default".to_string());
    let access_token = if creds.expires_at < now + 300_000 && !creds.refresh_token.is_empty() {
        match auth::oauth::refresh_claude_tokens_for_account(&creds.refresh_token, &active_label)
            .await
        {
            Ok(refreshed) => refreshed.access_token,
            Err(_) => creds.access_token,
        }
    } else {
        creds.access_token
    };

    let cache_key = anthropic_usage_cache_key(&access_token, Some(&active_label));
    fetch_anthropic_usage_data(access_token, cache_key).await
}

async fn refresh_usage(usage: Arc<RwLock<UsageData>>) {
    match fetch_usage().await {
        Ok(new_data) => {
            *usage.write().await = new_data;
        }
        Err(e) => {
            let err_msg = e.to_string();
            let mut data = usage.write().await;
            let is_new_error = data.last_error.as_deref() != Some(&err_msg);
            data.last_error = Some(err_msg.clone());
            data.fetched_at = Some(Instant::now());
            if is_new_error {
                crate::logging::error(&format!("Usage fetch error: {}", err_msg));
            }
        }
    }
}

fn try_spawn_refresh(usage: Arc<RwLock<UsageData>>) {
    if REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(async move {
        refresh_usage(usage).await;
        REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
    });
}

/// Get current usage data, refreshing if stale
pub async fn get() -> UsageData {
    let usage = get_usage().await;

    // Check if we need to refresh
    let (should_refresh, current_data) = {
        let data = usage.read().await;
        (data.is_stale(), data.clone())
    };

    if should_refresh {
        try_spawn_refresh(usage.clone());
    }

    current_data
}

// ─── OpenAI usage tracker (Codex/ChatGPT OAuth) ───────────────────────────────

static OPENAI_USAGE: tokio::sync::OnceCell<Arc<RwLock<OpenAIUsageData>>> =
    tokio::sync::OnceCell::const_new();
static OPENAI_REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

async fn get_openai_usage_cell() -> Arc<RwLock<OpenAIUsageData>> {
    OPENAI_USAGE
        .get_or_init(|| async { Arc::new(RwLock::new(OpenAIUsageData::default())) })
        .await
        .clone()
}

async fn fetch_openai_usage_data() -> OpenAIUsageData {
    match fetch_openai_usage_report().await {
        Some(report) => {
            let mut data = classify_openai_limits(&report.limits);
            data.fetched_at = Some(Instant::now());
            data.last_error = report.error;
            data
        }
        None => OpenAIUsageData {
            fetched_at: Some(Instant::now()),
            last_error: Some("No OpenAI/Codex OAuth credentials found".to_string()),
            ..Default::default()
        },
    }
}

async fn refresh_openai_usage(usage: Arc<RwLock<OpenAIUsageData>>) {
    let new_data = fetch_openai_usage_data().await;
    *usage.write().await = new_data;
}

fn try_spawn_openai_refresh(usage: Arc<RwLock<OpenAIUsageData>>) {
    if OPENAI_REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(async move {
        refresh_openai_usage(usage).await;
        OPENAI_REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
    });
}

pub async fn get_openai_usage() -> OpenAIUsageData {
    let usage = get_openai_usage_cell().await;

    let (should_refresh, current_data) = {
        let data = usage.read().await;
        (data.is_stale(), data.clone())
    };

    if should_refresh {
        try_spawn_openai_refresh(usage.clone());
    }

    current_data
}

pub fn get_openai_usage_sync() -> OpenAIUsageData {
    if let Some(usage) = OPENAI_USAGE.get() {
        if let Ok(data) = usage.try_read() {
            if data.is_stale() {
                try_spawn_openai_refresh(usage.clone());
            }
            return data.clone();
        }
    }

    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(async {
            let _ = get_openai_usage().await;
        });
    }

    OpenAIUsageData::default()
}

/// Check if extra usage (1M context, etc.) is enabled for the account.
/// Returns false if unknown/not yet fetched.
pub fn has_extra_usage() -> bool {
    if let Some(usage) = USAGE.get() {
        if let Ok(data) = usage.try_read() {
            return data.extra_usage_enabled;
        }
    }
    false
}

/// Fetch usage data for a specific Anthropic account token (blocking).
/// Used for account rotation - checks if a particular account is exhausted.
/// Returns an error if the fetch fails (network, auth, etc.).
/// Results are cached per-account to avoid hammering the API.
pub fn fetch_usage_for_account_sync(
    access_token: &str,
    refresh_token: &str,
    expires_at: i64,
) -> Result<UsageData> {
    let cache_key = anthropic_usage_cache_key(access_token, None);

    if let Some(cached) = cached_anthropic_usage(&cache_key) {
        return Ok(cached);
    }

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(fetch_usage_for_account(
            access_token.to_string(),
            refresh_token.to_string(),
            expires_at,
        ))
    });

    if let Ok(ref data) = result {
        store_anthropic_usage(cache_key, data.clone());
    }

    result
}

async fn fetch_usage_for_account(
    access_token: String,
    _refresh_token: String,
    expires_at: i64,
) -> Result<UsageData> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    if expires_at < now_ms {
        anyhow::bail!("OAuth token expired");
    }

    let cache_key = anthropic_usage_cache_key(&access_token, None);
    fetch_anthropic_usage_data(access_token, cache_key).await
}

/// Get usage data synchronously (returns cached data, triggers refresh if stale)
pub fn get_sync() -> UsageData {
    // Try to get cached data
    if let Some(usage) = USAGE.get() {
        // Return current cached value (blocking read)
        if let Ok(data) = usage.try_read() {
            if data.is_stale() {
                try_spawn_refresh(usage.clone());
            }
            return data.clone();
        }
    }

    // Not initialized yet - trigger initialization
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(async {
            let _ = get().await;
        });
    }

    UsageData::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_data_default() {
        let data = UsageData::default();
        assert!(data.is_stale());
        assert_eq!(data.five_hour_percent(), "0%");
        assert_eq!(data.seven_day_percent(), "0%");
    }

    #[test]
    fn test_usage_percent_format() {
        let data = UsageData {
            five_hour: 0.42,
            seven_day: 0.156,
            ..Default::default()
        };
        assert_eq!(data.five_hour_percent(), "42%");
        assert_eq!(data.seven_day_percent(), "16%");
    }

    #[test]
    fn test_humanize_key() {
        assert_eq!(humanize_key("five_hour"), "Five Hour");
        assert_eq!(humanize_key("seven_day_opus"), "Seven Day Opus");
        assert_eq!(humanize_key("plan"), "Plan");
    }

    #[test]
    fn test_get_sync_without_runtime_does_not_panic() {
        let result = std::panic::catch_unwind(get_sync);
        assert!(
            result.is_ok(),
            "get_sync should not require a Tokio runtime"
        );
    }

    #[test]
    fn test_get_openai_usage_sync_without_runtime_does_not_panic() {
        let result = std::panic::catch_unwind(get_openai_usage_sync);
        assert!(
            result.is_ok(),
            "get_openai_usage_sync should not require a Tokio runtime"
        );
    }

    #[test]
    fn test_mask_email_censors_local_part() {
        assert_eq!(mask_email("jeremyh1@uw.edu"), "j***1@uw.edu");
        assert_eq!(mask_email("ab@example.com"), "a*@example.com");
    }

    #[test]
    fn test_format_usage_bar() {
        let bar = format_usage_bar(50.0, 10);
        assert!(bar.contains("█████░░░░░"));
        assert!(bar.contains("50%"));

        let bar = format_usage_bar(0.0, 10);
        assert!(bar.contains("░░░░░░░░░░"));
        assert!(bar.contains("0%"));

        let bar = format_usage_bar(100.0, 10);
        assert!(bar.contains("██████████"));
        assert!(bar.contains("100%"));
    }

    #[test]
    fn test_format_reset_time_past() {
        assert_eq!(format_reset_time("2020-01-01T00:00:00Z"), "now");
    }

    #[test]
    fn test_classify_openai_limits_recognizes_five_weekly_and_spark() {
        let limits = vec![
            UsageLimit {
                name: "Codex 5h".to_string(),
                usage_percent: 25.0,
                resets_at: Some("2026-01-01T00:00:00Z".to_string()),
            },
            UsageLimit {
                name: "Codex 1w".to_string(),
                usage_percent: 50.0,
                resets_at: Some("2026-01-07T00:00:00Z".to_string()),
            },
            UsageLimit {
                name: "Codex Spark".to_string(),
                usage_percent: 75.0,
                resets_at: Some("2026-01-02T00:00:00Z".to_string()),
            },
        ];

        let classified = classify_openai_limits(&limits);

        assert_eq!(
            classified.five_hour.as_ref().map(|w| w.usage_ratio),
            Some(0.25)
        );
        assert_eq!(
            classified.seven_day.as_ref().map(|w| w.usage_ratio),
            Some(0.5)
        );
        assert_eq!(classified.spark.as_ref().map(|w| w.usage_ratio), Some(0.75));
    }

    #[test]
    fn test_parse_usage_percent_supports_used_limit_shape() {
        let mut obj = serde_json::Map::new();
        obj.insert("used".to_string(), serde_json::json!(20));
        obj.insert("limit".to_string(), serde_json::json!(80));

        let percent = parse_usage_percent_from_obj(&obj);
        assert_eq!(percent, Some(25.0));
    }

    #[test]
    fn test_parse_usage_percent_supports_remaining_limit_shape() {
        let mut obj = serde_json::Map::new();
        obj.insert("remaining".to_string(), serde_json::json!(60));
        obj.insert("limit".to_string(), serde_json::json!(80));

        let percent = parse_usage_percent_from_obj(&obj);
        assert_eq!(percent, Some(25.0));
    }

    #[test]
    fn test_active_anthropic_usage_report_prefers_marked_account() {
        let results = vec![
            ProviderUsage {
                provider_name: "Anthropic - work".to_string(),
                ..Default::default()
            },
            ProviderUsage {
                provider_name: "Anthropic - personal ✦".to_string(),
                ..Default::default()
            },
        ];

        let active = active_anthropic_usage_report(&results)
            .expect("expected active anthropic report to be selected");
        assert_eq!(active.provider_name, "Anthropic - personal ✦");
    }

    #[test]
    fn test_usage_data_from_provider_report_maps_limits_and_extra_usage() {
        let report = ProviderUsage {
            provider_name: "Anthropic (Claude)".to_string(),
            limits: vec![
                UsageLimit {
                    name: "5-hour window".to_string(),
                    usage_percent: 25.0,
                    resets_at: Some("2026-01-01T00:00:00Z".to_string()),
                },
                UsageLimit {
                    name: "7-day window".to_string(),
                    usage_percent: 50.0,
                    resets_at: Some("2026-01-07T00:00:00Z".to_string()),
                },
                UsageLimit {
                    name: "7-day Opus window".to_string(),
                    usage_percent: 75.0,
                    resets_at: Some("2026-01-08T00:00:00Z".to_string()),
                },
            ],
            extra_info: vec![(
                "Extra usage (long context)".to_string(),
                "enabled".to_string(),
            )],
            error: None,
        };

        let usage = usage_data_from_provider_report(&report);

        assert_eq!(usage.five_hour, 0.25);
        assert_eq!(usage.seven_day, 0.5);
        assert_eq!(usage.seven_day_opus, Some(0.75));
        assert!(usage.extra_usage_enabled);
        assert_eq!(
            usage.five_hour_resets_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }

    #[test]
    fn test_openai_usage_data_from_provider_report_preserves_error() {
        let report = ProviderUsage {
            provider_name: "OpenAI (ChatGPT)".to_string(),
            error: Some("API error (401 Unauthorized)".to_string()),
            ..Default::default()
        };

        let usage = openai_usage_data_from_provider_report(&report);

        assert_eq!(
            usage.last_error.as_deref(),
            Some("API error (401 Unauthorized)")
        );
        assert!(usage.five_hour.is_none());
        assert!(usage.seven_day.is_none());
    }
}
