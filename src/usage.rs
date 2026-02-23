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
            Some(t) => t.elapsed() > CACHE_DURATION,
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

/// Fetch usage from all connected providers with OAuth credentials.
/// Returns a list of ProviderUsage, one per provider that has credentials.
pub async fn fetch_all_provider_usage() -> Vec<ProviderUsage> {
    let mut results = Vec::new();

    let (anthropic, openai) =
        tokio::join!(fetch_anthropic_usage_report(), fetch_openai_usage_report());

    if let Some(r) = anthropic {
        results.push(r);
    }
    if let Some(r) = openai {
        results.push(r);
    }

    results
}

async fn fetch_anthropic_usage_report() -> Option<ProviderUsage> {
    let creds = auth::claude::load_credentials().ok()?;
    if creds.access_token.is_empty() {
        return None;
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    if creds.expires_at < now_ms {
        return Some(ProviderUsage {
            provider_name: "Anthropic (Claude)".to_string(),
            error: Some("OAuth token expired - use `/login claude` to re-authenticate".to_string()),
            ..Default::default()
        });
    }

    let client = Client::new();
    let response = client
        .get(USAGE_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", "claude-cli/1.0.0")
        .header("Authorization", format!("Bearer {}", creds.access_token))
        .header("anthropic-beta", "oauth-2025-04-20,claude-code-20250219")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return Some(ProviderUsage {
                provider_name: "Anthropic (Claude)".to_string(),
                error: Some(format!("Failed to fetch: {}", e)),
                ..Default::default()
            });
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Some(ProviderUsage {
            provider_name: "Anthropic (Claude)".to_string(),
            error: Some(format!("API error ({}): {}", status, body)),
            ..Default::default()
        });
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

            Some(ProviderUsage {
                provider_name: "Anthropic (Claude)".to_string(),
                limits,
                extra_info,
                error: None,
            })
        }
        Err(e) => Some(ProviderUsage {
            provider_name: "Anthropic (Claude)".to_string(),
            error: Some(format!("Failed to parse response: {}", e)),
            ..Default::default()
        }),
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

    if let Some(rate_limits) = json.get("rate_limits").and_then(|v| v.as_array()) {
        for entry in rate_limits {
            let name = entry
                .get("name")
                .or_else(|| entry.get("label"))
                .or_else(|| entry.get("display_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let usage = entry
                .get("usage")
                .or_else(|| entry.get("utilization"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let resets_at = entry
                .get("resets_at")
                .or_else(|| entry.get("reset_at"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            limits.push(UsageLimit {
                name,
                usage_percent: usage,
                resets_at,
            });
        }
    }

    if limits.is_empty() {
        if let Some(obj) = json.as_object() {
            for (key, value) in obj {
                if key == "rate_limits" {
                    continue;
                }
                if let Some(inner) = value.as_object() {
                    let usage = inner
                        .get("usage")
                        .or_else(|| inner.get("utilization"))
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32);
                    let resets_at = inner
                        .get("resets_at")
                        .or_else(|| inner.get("reset_at"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if let Some(u) = usage {
                        limits.push(UsageLimit {
                            name: humanize_key(key),
                            usage_percent: u,
                            resets_at,
                        });
                    }
                } else if let Some(s) = value.as_str() {
                    extra_info.push((humanize_key(key), s.to_string()));
                } else if let Some(b) = value.as_bool() {
                    extra_info.push((humanize_key(key), if b { "yes" } else { "no" }.to_string()));
                }
            }
        }
    }

    if let Some(plan) = json
        .get("plan")
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

    let client = Client::new();
    let response = client
        .get(USAGE_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", "claude-cli/1.0.0")
        .header("Authorization", format!("Bearer {}", creds.access_token))
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
            let mut data = usage.write().await;
            data.last_error = Some(e.to_string());
            data.fetched_at = Some(Instant::now()); // Prevent spam retries
            crate::logging::error(&format!("Usage fetch error: {}", e));
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
}
