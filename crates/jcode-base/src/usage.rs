//! Subscription usage tracking.
//!
//! Fetches usage information from Anthropic's OAuth usage endpoint and OpenAI's ChatGPT wham/usage endpoint.

use crate::auth;
mod accessors;
mod api_keys;
mod cache;
mod display;
mod model;
mod openai_helpers;
mod provider_fetch;
pub use accessors::*;
use api_keys::enqueue_api_key_usage_tasks;
use cache::*;
pub use jcode_usage_types::{ProviderUsage, ProviderUsageProgress, UsageLimit};
pub use model::*;
use provider_fetch::*;

use anyhow::{Context, Result};
pub use display::{format_reset_time, format_usage_bar};
use display::{format_token_count, humanize_key, provider_usage_cache_is_fresh};
use openai_helpers::{parse_openai_usage_payload, usage_percent_to_ratio};
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

/// Minimum interval between /usage command fetches (per provider).
const PROVIDER_USAGE_CACHE_TTL: Duration = Duration::from_secs(120);

/// Cached provider usage reports (used by /usage command).
/// Keyed by provider display name.
static PROVIDER_USAGE_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, (Instant, ProviderUsage)>>,
> = std::sync::OnceLock::new();

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
            .map(usage_percent_to_ratio)
            .unwrap_or(0.0),
        five_hour_resets_at: data.five_hour.as_ref().and_then(|w| w.resets_at.clone()),
        seven_day: data
            .seven_day
            .as_ref()
            .and_then(|w| w.utilization)
            .map(usage_percent_to_ratio)
            .unwrap_or(0.0),
        seven_day_resets_at: data.seven_day.as_ref().and_then(|w| w.resets_at.clone()),
        seven_day_opus: data
            .seven_day_opus
            .as_ref()
            .and_then(|w| w.utilization)
            .map(usage_percent_to_ratio),
        model_scoped: data
            .limits
            .into_iter()
            .filter(|limit| limit.kind.as_deref() == Some("weekly_scoped"))
            .filter_map(|limit| {
                let model_name = limit.scope?.model?.display_name?;
                Some(ModelScopedUsageWindow {
                    model_name,
                    utilization: usage_percent_to_ratio(limit.percent?),
                    resets_at: limit.resets_at,
                })
            })
            .collect(),
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
        let mut cached = map.values().map(|(_, r)| r.clone()).collect::<Vec<_>>();
        sort_reports_most_recent_first(&mut cached);
        cached
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
    sort_reports_most_recent_first(results);
}

/// Order reports so the most recently used login comes first. Sources jcode
/// has never used sort last, alphabetically for stability.
fn sort_reports_most_recent_first(results: &mut [ProviderUsage]) {
    results.sort_by(|a, b| {
        b.last_used_unix_secs
            .cmp(&a.last_used_unix_secs)
            .then_with(|| a.provider_name.cmp(&b.provider_name))
    });
}

/// Stamp a report with last-used recency from the activity ledger: sets the
/// sort key and appends a human-readable "Last used" detail line.
fn attach_activity(report: &mut ProviderUsage, source_key: &str) {
    if let Some(used) = crate::provider_activity::last_used_unix_secs(source_key) {
        report.last_used_unix_secs = Some(used);
        report.extra_info.push((
            "Last used".to_string(),
            crate::provider_activity::format_relative_age(used),
        ));
    }
}

fn enqueue_provider_usage_tasks(tasks: &mut tokio::task::JoinSet<Option<ProviderUsage>>) -> usize {
    let mut total = 0usize;

    total += enqueue_anthropic_usage_tasks(tasks);
    total += enqueue_openai_usage_tasks(tasks);
    total += enqueue_api_key_usage_tasks(tasks);

    if openrouter_api_key().is_some() {
        tasks.spawn(async {
            fetch_openrouter_usage_report().await.map(|mut report| {
                attach_activity(&mut report, "openrouter");
                report
            })
        });
        total += 1;
    }

    if auth::copilot::has_copilot_credentials() {
        tasks.spawn(async {
            fetch_copilot_usage_report().await.map(|mut report| {
                attach_activity(&mut report, "copilot");
                report
            })
        });
        total += 1;
    }

    if auth::antigravity::has_cached_auth() {
        tasks.spawn(async {
            fetch_antigravity_usage_report().await.map(|mut report| {
                attach_activity(&mut report, "antigravity");
                report
            })
        });
        total += 1;
    }

    if auth::gemini::has_api_key() {
        tasks.spawn(async {
            fetch_gemini_usage_report().await.map(|mut report| {
                attach_activity(&mut report, "gemini");
                report
            })
        });
        total += 1;
    }

    if auth::cursor::has_cursor_api_key() {
        tasks.spawn(async {
            fetch_cursor_usage_report().await.map(|mut report| {
                attach_activity(&mut report, "cursor");
                report
            })
        });
        total += 1;
    }

    total += enqueue_activity_sweeper_task(tasks);

    total
}

/// Source-key prefixes that already get a dedicated `/usage` report; the
/// sweeper skips these so used-but-unreported logins (Cursor, Bedrock,
/// Azure, jcode subscription, ...) still show recency + local spend without
/// double-listing covered providers.
fn activity_source_has_dedicated_report(source_key: &str) -> bool {
    // Dual-auth and OAuth surfaces always reported above.
    if source_key.starts_with("claude:") || source_key.starts_with("openai:") {
        return true;
    }
    match source_key {
        "openrouter" => openrouter_api_key().is_some(),
        "copilot" => auth::copilot::has_copilot_credentials(),
        "antigravity" => auth::antigravity::has_cached_auth(),
        "gemini" => auth::gemini::has_api_key(),
        "cursor" => auth::cursor::has_cursor_api_key(),
        _ => {
            // Direct OpenAI-compatible profiles are reported by the API-key
            // module whenever their key is configured.
            source_key
                .strip_prefix("openai-compatible:")
                .and_then(crate::provider_catalog::openai_compatible_profile_by_id)
                .map(|profile| {
                    crate::provider_catalog::load_api_key_from_env_or_config(
                        profile.api_key_env,
                        profile.env_file,
                    )
                    .is_some()
                })
                .unwrap_or(false)
        }
    }
}

/// One catch-all task that reports every ledger entry without a dedicated
/// fetcher: last-used recency plus locally tracked spend.
fn enqueue_activity_sweeper_task(tasks: &mut tokio::task::JoinSet<Option<ProviderUsage>>) -> usize {
    let leftover: Vec<(String, crate::provider_activity::ProviderActivityEntry)> =
        crate::provider_activity::all_entries()
            .into_iter()
            .filter(|(key, _)| !activity_source_has_dedicated_report(key))
            .collect();
    if leftover.is_empty() {
        return 0;
    }

    let count = leftover.len();
    for (source_key, entry) in leftover {
        tasks.spawn(async move {
            let mut extra_info = Vec::new();
            if let Some(spend) = entry.spend {
                extra_info.push((
                    "Local spend (this machine)".to_string(),
                    format!(
                        "${:.2} today · ${:.2} this month · ${:.2} all-time",
                        spend.day_usd, spend.month_usd, spend.all_time_usd
                    ),
                ));
            }
            let mut report = ProviderUsage {
                provider_name: crate::provider_activity::display_name_for_source_key(&source_key),
                extra_info,
                ..Default::default()
            };
            attach_activity(&mut report, &source_key);
            Some(report)
        });
    }
    count
}

fn enqueue_anthropic_usage_tasks(tasks: &mut tokio::task::JoinSet<Option<ProviderUsage>>) -> usize {
    let accounts = match auth::claude::list_accounts() {
        Ok(a) if !a.is_empty() => a,
        _ => match auth::claude::load_credentials() {
            Ok(creds) if !creds.access_token.is_empty() => {
                tasks.spawn(async move {
                    let mut report = fetch_anthropic_usage_for_token(
                        "Anthropic (Claude)".to_string(),
                        creds.access_token,
                        creds.refresh_token,
                        "default".to_string(),
                        creds.expires_at,
                    )
                    .await;
                    attach_activity(&mut report, "claude:oauth:default");
                    Some(report)
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
            let source_key = format!("claude:oauth:{}", account.label);
            let mut report = fetch_anthropic_usage_for_token(
                label,
                account.access,
                account.refresh,
                account.label,
                account.expires,
            )
            .await;
            attach_activity(&mut report, &source_key);
            Some(report)
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
                let source_key = format!("openai:oauth:{}", account_label);
                let mut report =
                    fetch_openai_usage_for_account(display_name, creds, Some(&account_label)).await;
                attach_activity(&mut report, &source_key);
                Some(report)
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
        let mut report = fetch_openai_usage_for_account(
            openai_provider_display_name("default", None, 1, true),
            creds,
            None,
        )
        .await;
        attach_activity(&mut report, "openai:oauth:default");
        Some(report)
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

#[cfg(test)]
mod tests;
