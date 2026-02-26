//! Subscription usage tracking
//!
//! Fetches usage information from Anthropic's OAuth usage endpoint
//! and OpenAI's ChatGPT wham/usage endpoint.

use crate::auth;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Usage API endpoint
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// OpenAI ChatGPT usage endpoint
const OPENAI_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// Cache duration (refresh every 60 seconds)
const CACHE_DURATION: Duration = Duration::from_secs(60);

/// Error backoff duration (wait 5 minutes before retrying after auth/credential errors)
const ERROR_BACKOFF: Duration = Duration::from_secs(300);

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
                let ttl = if self.last_error.is_some() {
                    ERROR_BACKOFF
                } else {
                    CACHE_DURATION
                };
                t.elapsed() > ttl
            }
            None => true,
        }
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
                let ttl = if self.last_error.is_some() {
                    ERROR_BACKOFF
                } else {
                    CACHE_DURATION
                };
                t.elapsed() > ttl
            }
            None => true,
        }
    }

    pub fn has_limits(&self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some() || self.spark.is_some()
    }
}

/// Fetch usage from all connected providers with OAuth credentials.
/// Returns a list of ProviderUsage, one per provider that has credentials.
pub async fn fetch_all_provider_usage() -> Vec<ProviderUsage> {
    let mut results = Vec::new();

    let (anthropic_results, openai) =
        tokio::join!(fetch_all_anthropic_usage_reports(), fetch_openai_usage_report());

    results.extend(anthropic_results);
    if let Some(r) = openai {
        results.push(r);
    }

    results
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
        _ => {
            match auth::claude::load_credentials() {
                Ok(creds) if !creds.access_token.is_empty() => {
                    return vec![fetch_anthropic_usage_for_token(
                        "Anthropic (Claude)".to_string(),
                        creds.access_token.clone(),
                        creds.expires_at,
                    )
                    .await];
                }
                _ => return Vec::new(),
            }
        }
    };

    let active_label = auth::claude::active_account_label();
    let mut futures = Vec::new();
    for account in &accounts {
        let label = if accounts.len() > 1 {
            let active_marker = if active_label.as_deref() == Some(&account.label) {
                " *"
            } else {
                ""
            };
            format!("Anthropic - {}{}", account.label, active_marker)
        } else {
            "Anthropic (Claude)".to_string()
        };
        let access = account.access.clone();
        let expires = account.expires;
        futures.push(fetch_anthropic_usage_for_token(
            label,
            access,
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
    expires_at: i64,
) -> ProviderUsage {
    let now_ms = chrono::Utc::now().timestamp_millis();
    if expires_at < now_ms {
        return ProviderUsage {
            provider_name: display_name.to_string(),
            error: Some("OAuth token expired - use `/login claude` to re-authenticate".to_string()),
            ..Default::default()
        };
    }

    let client = Client::new();
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
        Ok(r) => r,
        Err(e) => {
            return ProviderUsage {
                provider_name: display_name.to_string(),
                error: Some(format!("Failed to fetch: {}", e)),
                ..Default::default()
            };
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return ProviderUsage {
            provider_name: display_name.to_string(),
            error: Some(format!("API error ({}): {}", status, body)),
            ..Default::default()
        };
    }

    match response.json::<UsageResponse>().await {
        Ok(data) => {
            let mut limits = Vec::new();
            if let Some(ref w) = data.five_hour {
                limits.push(UsageLimit {
                    name: "5-hour window".to_string(),
                    usage_percent: w.utilization.unwrap_or(0.0),
                    resets_at: w.resets_at.clone(),
                });
            }
            if let Some(ref w) = data.seven_day {
                limits.push(UsageLimit {
                    name: "7-day window".to_string(),
                    usage_percent: w.utilization.unwrap_or(0.0),
                    resets_at: w.resets_at.clone(),
                });
            }
            if let Some(ref w) = data.seven_day_opus {
                if let Some(u) = w.utilization {
                    limits.push(UsageLimit {
                        name: "7-day Opus window".to_string(),
                        usage_percent: u,
                        resets_at: w.resets_at.clone(),
                    });
                }
            }

            let mut extra_info = Vec::new();
            if let Some(ref eu) = data.extra_usage {
                extra_info.push((
                    "Extra usage (long context)".to_string(),
                    if eu.is_enabled.unwrap_or(false) {
                        "enabled".to_string()
                    } else {
                        "disabled".to_string()
                    },
                ));
            }

            ProviderUsage {
                provider_name: display_name.to_string(),
                limits,
                extra_info,
                error: None,
            }
        }
        Err(e) => ProviderUsage {
            provider_name: display_name.to_string(),
            error: Some(format!("Failed to parse response: {}", e)),
            ..Default::default()
        },
    }
}

async fn fetch_openai_usage_report() -> Option<ProviderUsage> {
    let creds = auth::codex::load_credentials().ok()?;
    if creds.access_token.is_empty() {
        return None;
    }

    let is_chatgpt = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    if !is_chatgpt {
        return None;
    }

    let access_token = if let Some(expires_at) = creds.expires_at {
        let now = chrono::Utc::now().timestamp_millis();
        if expires_at < now + 300_000 && !creds.refresh_token.is_empty() {
            match crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await {
                Ok(refreshed) => refreshed.access_token,
                Err(e) => {
                    return Some(ProviderUsage {
                        provider_name: "OpenAI (ChatGPT)".to_string(),
                        error: Some(format!(
                            "Token refresh failed: {} - use `/login openai` to re-authenticate",
                            e
                        )),
                        ..Default::default()
                    });
                }
            }
        } else {
            creds.access_token.clone()
        }
    } else {
        creds.access_token.clone()
    };

    let client = Client::new();
    let mut builder = client
        .get(OPENAI_USAGE_URL)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", access_token));

    if let Some(ref account_id) = creds.account_id {
        builder = builder.header("chatgpt-account-id", account_id);
    }

    let response = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            return Some(ProviderUsage {
                provider_name: "OpenAI (ChatGPT)".to_string(),
                error: Some(format!("Failed to fetch: {}", e)),
                ..Default::default()
            });
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Some(ProviderUsage {
            provider_name: "OpenAI (ChatGPT)".to_string(),
            error: Some(format!("API error ({}): {}", status, body)),
            ..Default::default()
        });
    }

    let body_text = match response.text().await {
        Ok(t) => t,
        Err(e) => {
            return Some(ProviderUsage {
                provider_name: "OpenAI (ChatGPT)".to_string(),
                error: Some(format!("Failed to read response: {}", e)),
                ..Default::default()
            });
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&body_text) {
        Ok(v) => v,
        Err(e) => {
            return Some(ProviderUsage {
                provider_name: "OpenAI (ChatGPT)".to_string(),
                error: Some(format!("Failed to parse response: {}", e)),
                ..Default::default()
            });
        }
    };

    let mut limits = Vec::new();
    let mut extra_info = Vec::new();

    fn parse_wham_window(window: &serde_json::Value, name: &str) -> Option<UsageLimit> {
        let obj = window.as_object()?;
        let used_percent = obj.get("used_percent").and_then(parse_f32_value)?;
        let resets_at = obj
            .get("reset_at")
            .and_then(parse_f32_value)
            .map(|ts| {
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

    fn parse_wham_rate_limit(rl: &serde_json::Value, primary_name: &str, secondary_name: &str) -> Vec<UsageLimit> {
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

    if let Some(additional) = json.get("additional_rate_limits").and_then(|v| v.as_array()) {
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

    if limits.is_empty() {
        if let Some(rate_limits) = json.get("rate_limits").and_then(|v| v.as_array()) {
            for entry in rate_limits {
                if let Some(obj) = entry.as_object() {
                    if let Some(usage_percent) = parse_usage_percent_from_obj(obj) {
                        limits.push(UsageLimit {
                            name: parse_limit_name(entry, "unknown"),
                            usage_percent,
                            resets_at: parse_resets_at_from_obj(obj),
                        });
                    }
                }
            }
        }
    }

    if limits.is_empty() {
        if let Some(obj) = json.as_object() {
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
                            if let Some(entry_obj) = entry.as_object() {
                                if let Some(usage_percent) = parse_usage_percent_from_obj(entry_obj)
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

    Some(ProviderUsage {
        provider_name: "OpenAI (ChatGPT)".to_string(),
        limits,
        extra_info,
        error: None,
    })
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
    let access_token = if creds.expires_at < now + 300_000 && !creds.refresh_token.is_empty() {
        let active_label = auth::claude::active_account_label()
            .unwrap_or_else(|| "default".to_string());
        match auth::oauth::refresh_claude_tokens_for_account(
            &creds.refresh_token,
            &active_label,
        )
        .await
        {
            Ok(refreshed) => refreshed.access_token,
            Err(_) => creds.access_token,
        }
    } else {
        creds.access_token
    };

    let client = Client::new();
    let response = client
        .get(USAGE_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", "claude-cli/1.0.0")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("anthropic-beta", "oauth-2025-04-20,claude-code-20250219")
        .send()
        .await
        .context("Failed to fetch usage data")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Usage API error ({}): {}", status, error_text);
    }

    let data: UsageResponse = response
        .json()
        .await
        .context("Failed to parse usage response")?;

    // API returns percentages (0-100), convert to fractions (0.0-1.0)
    Ok(UsageData {
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
    })
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

    tokio::spawn(async {
        let _ = get_openai_usage().await;
    });

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
    tokio::spawn(async {
        let _ = get().await;
    });

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
}
