//! Subscription usage tracking
//!
//! Fetches usage information from Anthropic's OAuth usage endpoint.

use crate::auth;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Usage API endpoint
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

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
}

#[derive(Deserialize, Debug)]
struct UsageWindow {
    utilization: Option<f32>,
    resets_at: Option<String>,
}

/// Global usage tracker
static USAGE: tokio::sync::OnceCell<Arc<RwLock<UsageData>>> = tokio::sync::OnceCell::const_new();

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
        .header("User-Agent", "jcode/0.1")
        .header("Authorization", format!("Bearer {}", creds.access_token))
        .header("anthropic-beta", "oauth-2025-04-20")
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

    Ok(UsageData {
        five_hour: data
            .five_hour
            .as_ref()
            .and_then(|w| w.utilization)
            .unwrap_or(0.0),
        five_hour_resets_at: data.five_hour.as_ref().and_then(|w| w.resets_at.clone()),
        seven_day: data
            .seven_day
            .as_ref()
            .and_then(|w| w.utilization)
            .unwrap_or(0.0),
        seven_day_resets_at: data.seven_day.as_ref().and_then(|w| w.resets_at.clone()),
        seven_day_opus: data.seven_day_opus.as_ref().and_then(|w| w.utilization),
        fetched_at: Some(Instant::now()),
        last_error: None,
    })
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
        // Spawn a refresh task if not already running
        let usage_clone = usage.clone();
        tokio::spawn(async move {
            match fetch_usage().await {
                Ok(new_data) => {
                    *usage_clone.write().await = new_data;
                }
                Err(e) => {
                    let mut data = usage_clone.write().await;
                    data.last_error = Some(e.to_string());
                    data.fetched_at = Some(Instant::now()); // Prevent spam retries
                    crate::logging::error(&format!("Usage fetch error: {}", e));
                }
            }
        });
    }

    current_data
}

/// Get usage data synchronously (returns cached data, triggers refresh if stale)
pub fn get_sync() -> UsageData {
    // Try to get cached data
    if let Some(usage) = USAGE.get() {
        // Spawn async refresh if stale
        let usage_clone = usage.clone();
        tokio::spawn(async move {
            let should_refresh = {
                let data = usage_clone.read().await;
                data.is_stale()
            };

            if should_refresh {
                if let Ok(new_data) = fetch_usage().await {
                    *usage_clone.write().await = new_data;
                }
            }
        });

        // Return current cached value (blocking read)
        if let Ok(data) = usage.try_read() {
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
}
