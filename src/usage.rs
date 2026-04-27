//! Subscription usage tracking.
//!
//! Fetches usage information from Anthropic's OAuth usage endpoint and OpenAI's ChatGPT wham/usage endpoint.

use crate::auth;
mod accessors;
mod display;
mod openai_helpers;
mod provider_fetch;
pub use accessors::*;
use provider_fetch::*;

use anyhow::{Context, Result};
pub use display::{format_reset_time, format_usage_bar};
use display::{
    format_token_count, humanize_key, provider_usage_cache_is_fresh, usage_reset_passed,
};
use openai_helpers::{classify_openai_limits, normalize_ratio, parse_openai_usage_payload};
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
        if usage_reset_passed([
            self.five_hour_resets_at.as_deref(),
            self.seven_day_resets_at.as_deref(),
        ]) {
            return true;
        }

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
    pub hard_limit_reached: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UsageLimit {
    pub name: String,
    pub usage_percent: f32,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderUsageProgress {
    pub results: Vec<ProviderUsage>,
    pub completed: usize,
    pub total: usize,
    pub done: bool,
    pub from_cache: bool,
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
    pub hard_limit_reached: bool,
    pub fetched_at: Option<Instant>,
    pub last_error: Option<String>,
}

impl OpenAIUsageData {
    pub fn age_ms(&self) -> Option<u128> {
        self.fetched_at.map(|t| t.elapsed().as_millis())
    }

    pub fn freshness_state(&self) -> &'static str {
        if self.fetched_at.is_none() {
            "unknown"
        } else if self.is_stale() {
            "stale"
        } else {
            "fresh"
        }
    }

    pub fn exhausted(&self) -> bool {
        if self.hard_limit_reached {
            return true;
        }

        if !self.has_limits() {
            return false;
        }

        let five_hour_exhausted = self
            .five_hour
            .as_ref()
            .map(|w| w.usage_ratio >= 0.99)
            .unwrap_or(false);
        let seven_day_exhausted = self
            .seven_day
            .as_ref()
            .map(|w| w.usage_ratio >= 0.99)
            .unwrap_or(false);

        five_hour_exhausted && seven_day_exhausted
    }

    pub fn diagnostic_fields(&self) -> String {
        let fmt_ratio = |window: Option<&OpenAIUsageWindow>| {
            window
                .map(|w| format!("{:.1}%", w.usage_ratio * 100.0))
                .unwrap_or_else(|| "unknown".to_string())
        };

        format!(
            "freshness={} age_ms={} exhausted={} hard_limit_reached={} has_limits={} five_hour={} seven_day={} spark={} last_error={}",
            self.freshness_state(),
            self.age_ms()
                .map(|age| age.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            self.exhausted(),
            self.hard_limit_reached,
            self.has_limits(),
            fmt_ratio(self.five_hour.as_ref()),
            fmt_ratio(self.seven_day.as_ref()),
            fmt_ratio(self.spark.as_ref()),
            self.last_error.as_deref().unwrap_or("none")
        )
    }

    pub fn is_stale(&self) -> bool {
        if usage_reset_passed([
            self.five_hour.as_ref().and_then(|w| w.resets_at.as_deref()),
            self.seven_day.as_ref().and_then(|w| w.resets_at.as_deref()),
            self.spark.as_ref().and_then(|w| w.resets_at.as_deref()),
        ]) {
            return true;
        }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiAccountProviderKind {
    Anthropic,
    OpenAI,
}

impl MultiAccountProviderKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAI => "OpenAI",
        }
    }

    pub fn switch_command(self, label: &str) -> String {
        match self {
            Self::Anthropic => format!("/account switch {}", label),
            Self::OpenAI => format!("/account openai switch {}", label),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsageSnapshot {
    pub label: String,
    pub email: Option<String>,
    pub exhausted: bool,
    pub five_hour_ratio: Option<f32>,
    pub seven_day_ratio: Option<f32>,
    pub resets_at: Option<String>,
    pub error: Option<String>,
}

impl AccountUsageSnapshot {
    pub fn summary(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }

        let mut parts = Vec::new();
        if let Some(ratio) = self.five_hour_ratio {
            parts.push(format!("5h {:.0}%", ratio * 100.0));
        }
        if let Some(ratio) = self.seven_day_ratio {
            parts.push(format!("7d {:.0}%", ratio * 100.0));
        }
        if let Some(reset) = &self.resets_at {
            parts.push(format!("resets {}", format_reset_time(reset)));
        }

        if parts.is_empty() {
            "limits unknown".to_string()
        } else {
            parts.join(", ")
        }
    }

    fn preference_score(&self) -> f32 {
        if self.error.is_some() {
            return f32::INFINITY;
        }
        self.five_hour_ratio
            .unwrap_or(0.0)
            .max(self.seven_day_ratio.unwrap_or(0.0))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsageProbe {
    pub provider: MultiAccountProviderKind,
    pub current_label: String,
    pub accounts: Vec<AccountUsageSnapshot>,
}

impl AccountUsageProbe {
    pub fn current_account(&self) -> Option<&AccountUsageSnapshot> {
        self.accounts
            .iter()
            .find(|account| account.label == self.current_label)
    }

    pub fn current_exhausted(&self) -> bool {
        self.current_account()
            .map(|account| account.exhausted)
            .unwrap_or(false)
    }

    pub fn has_multiple_accounts(&self) -> bool {
        self.accounts.len() > 1
    }

    pub fn best_available_alternative(&self) -> Option<&AccountUsageSnapshot> {
        if !self.current_exhausted() {
            return None;
        }

        self.accounts
            .iter()
            .filter(|account| account.label != self.current_label)
            .filter(|account| !account.exhausted && account.error.is_none())
            .min_by(|a, b| a.preference_score().total_cmp(&b.preference_score()))
    }

    pub fn all_accounts_exhausted(&self) -> bool {
        self.has_multiple_accounts()
            && self
                .accounts
                .iter()
                .filter(|account| account.error.is_none())
                .all(|account| account.exhausted)
    }

    pub fn switch_guidance(&self) -> Option<String> {
        let alternative = self.best_available_alternative()?;
        Some(format!(
            "Another {} account has headroom: `{}` ({}). Use `{}`.",
            self.provider.display_name(),
            alternative.label,
            alternative.summary(),
            self.provider.switch_command(&alternative.label)
        ))
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

/// Shared OpenAI usage cache keyed by account label/token prefix.
static OPENAI_ACCOUNT_USAGE_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, OpenAIUsageData>>,
> = std::sync::OnceLock::new();

/// Minimum interval between /usage command fetches (per provider).
const PROVIDER_USAGE_CACHE_TTL: Duration = Duration::from_secs(120);

fn anthropic_usage_cache() -> &'static std::sync::Mutex<HashMap<String, UsageData>> {
    ANTHROPIC_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn openai_usage_cache() -> &'static std::sync::Mutex<HashMap<String, OpenAIUsageData>> {
    OPENAI_ACCOUNT_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
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

fn openai_usage_cache_key(access_token: &str, account_label: Option<&str>) -> String {
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

fn cached_openai_usage(cache_key: &str) -> Option<OpenAIUsageData> {
    let cache = openai_usage_cache();
    let map = cache.lock().ok()?;
    let cached = map.get(cache_key)?.clone();
    (!cached.is_stale()).then_some(cached)
}

fn store_openai_usage(cache_key: String, data: OpenAIUsageData) {
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
        hard_limit_reached: false,
        error: None,
    }
}

async fn fetch_anthropic_usage_data(access_token: String, cache_key: String) -> Result<UsageData> {
    if let Some(cached) = cached_anthropic_usage(&cache_key) {
        return Ok(cached);
    }

    let client = crate::provider::shared_http_client();
    let response = crate::provider::anthropic::apply_oauth_attribution_headers(
        client
            .get(USAGE_URL)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                crate::provider::anthropic::CLAUDE_CLI_USER_AGENT,
            )
            .header("Authorization", format!("Bearer {}", access_token))
            .header("anthropic-beta", "oauth-2025-04-20,claude-code-20250219"),
        &crate::provider::anthropic::new_oauth_request_id(),
    )
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
    fetch_all_provider_usage_progressive(|_| {}).await
}

/// Fetch usage from all connected providers and report incremental progress as
/// each provider/account finishes. Cached data is emitted immediately when
/// available so the UI can show useful stale/fresh context while live refreshes
/// are still in flight.
pub async fn fetch_all_provider_usage_progressive<F>(mut on_update: F) -> Vec<ProviderUsage>
where
    F: FnMut(ProviderUsageProgress) + Send,
{
    let cache = PROVIDER_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));

    let now = Instant::now();
    let cached_results = if let Ok(map) = cache.lock() {
        map.values().map(|(_, r)| r.clone()).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let all_fresh = if let Ok(map) = cache.lock() {
        !map.is_empty()
            && map
                .values()
                .all(|(fetched_at, report)| provider_usage_cache_is_fresh(now, *fetched_at, report))
    } else {
        false
    };

    if all_fresh {
        on_update(ProviderUsageProgress {
            completed: cached_results.len(),
            total: cached_results.len(),
            done: true,
            from_cache: true,
            results: cached_results.clone(),
        });
        return cached_results;
    }

    let mut results = cached_results.clone();
    if !cached_results.is_empty() {
        on_update(ProviderUsageProgress {
            results: cached_results,
            completed: 0,
            total: 0,
            done: false,
            from_cache: true,
        });
    }

    let mut tasks = tokio::task::JoinSet::<Option<ProviderUsage>>::new();
    let total = enqueue_provider_usage_tasks(&mut tasks);

    if total == 0 {
        sync_cached_usage_from_reports(&results).await;
        if let Ok(mut map) = cache.lock() {
            map.clear();
        }
        on_update(ProviderUsageProgress {
            results: results.clone(),
            completed: 0,
            total: 0,
            done: true,
            from_cache: false,
        });
        return results;
    }

    let mut completed = 0usize;
    while let Some(joined) = tasks.join_next().await {
        completed += 1;
        if let Ok(Some(report)) = joined {
            upsert_provider_usage(&mut results, report);
        }

        on_update(ProviderUsageProgress {
            results: results.clone(),
            completed,
            total,
            done: false,
            from_cache: false,
        });
    }

    sync_cached_usage_from_reports(&results).await;

    if let Ok(mut map) = cache.lock() {
        map.clear();
        let now = Instant::now();
        for r in &results {
            map.insert(r.provider_name.clone(), (now, r.clone()));
        }
    }

    on_update(ProviderUsageProgress {
        results: results.clone(),
        completed: total,
        total,
        done: true,
        from_cache: false,
    });

    results
}

fn upsert_provider_usage(results: &mut Vec<ProviderUsage>, report: ProviderUsage) {
    if let Some(existing) = results
        .iter_mut()
        .find(|existing| existing.provider_name == report.provider_name)
    {
        *existing = report;
    } else {
        results.push(report);
    }
}

fn enqueue_provider_usage_tasks(tasks: &mut tokio::task::JoinSet<Option<ProviderUsage>>) -> usize {
    let mut total = 0usize;

    total += enqueue_anthropic_usage_tasks(tasks);
    total += enqueue_openai_usage_tasks(tasks);

    if openrouter_api_key().is_some() {
        tasks.spawn(async { fetch_openrouter_usage_report().await });
        total += 1;
    }

    if auth::copilot::has_copilot_credentials() {
        tasks.spawn(async { fetch_copilot_usage_report().await });
        total += 1;
    }

    total
}

fn enqueue_anthropic_usage_tasks(tasks: &mut tokio::task::JoinSet<Option<ProviderUsage>>) -> usize {
    let accounts = match auth::claude::list_accounts() {
        Ok(a) if !a.is_empty() => a,
        _ => match auth::claude::load_credentials() {
            Ok(creds) if !creds.access_token.is_empty() => {
                tasks.spawn(async move {
                    Some(
                        fetch_anthropic_usage_for_token(
                            "Anthropic (Claude)".to_string(),
                            creds.access_token,
                            creds.refresh_token,
                            "default".to_string(),
                            creds.expires_at,
                        )
                        .await,
                    )
                });
                return 1;
            }
            _ => return 0,
        },
    };

    let active_label = auth::claude::active_account_label();
    let account_count = accounts.len();
    for account in accounts {
        let label = if account_count > 1 {
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

        tasks.spawn(async move {
            Some(
                fetch_anthropic_usage_for_token(
                    label,
                    account.access,
                    account.refresh,
                    account.label,
                    account.expires,
                )
                .await,
            )
        });
    }

    account_count
}

fn enqueue_openai_usage_tasks(tasks: &mut tokio::task::JoinSet<Option<ProviderUsage>>) -> usize {
    let accounts = auth::codex::list_accounts().unwrap_or_default();
    if !accounts.is_empty() {
        let active_label = auth::codex::active_account_label();
        let account_count = accounts.len();
        for account in accounts {
            let display_name = openai_provider_display_name(
                &account.label,
                account.email.as_deref(),
                account_count,
                active_label.as_deref() == Some(&account.label),
            );
            let account_label = account.label;
            let creds = auth::codex::CodexCredentials {
                access_token: account.access_token,
                refresh_token: account.refresh_token,
                id_token: account.id_token,
                account_id: account.account_id,
                expires_at: account.expires_at,
            };
            tasks.spawn(async move {
                Some(
                    fetch_openai_usage_for_account(display_name, creds, Some(&account_label)).await,
                )
            });
        }
        return account_count;
    }

    let creds = match auth::codex::load_credentials() {
        Ok(creds) => creds,
        Err(_) => return 0,
    };
    let is_chatgpt = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    if !is_chatgpt || creds.access_token.is_empty() {
        return 0;
    }

    tasks.spawn(async move {
        Some(
            fetch_openai_usage_for_account(
                openai_provider_display_name("default", None, 1, true),
                creds,
                None,
            )
            .await,
        )
    });
    1
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
    data.hard_limit_reached = report.hard_limit_reached;
    data.fetched_at = Some(Instant::now());
    data.last_error = report.error.clone();
    data
}

fn provider_report_from_openai_usage_data(
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

fn openai_snapshot_from_usage(
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

fn anthropic_snapshot_from_usage(
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

#[cfg(test)]
mod tests;
