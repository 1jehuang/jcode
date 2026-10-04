//! API-key usage reporting for `/usage`.
//!
//! OAuth subscriptions expose rich usage endpoints, but plain API keys mostly
//! do not, so this module gathers the best available picture per key:
//!   - Key validity (cheap, free endpoint probes such as `GET /v1/models`).
//!   - Real balance / spend APIs where they exist (DeepSeek, Moonshot,
//!     Anthropic/OpenAI admin cost reports when an admin key is configured).
//!   - Locally tracked spend from [`crate::provider_activity`] (jcode prices
//!     every API-key call it makes, so this is a per-machine estimate).
//!   - Last-used recency from the activity ledger.

use super::*;
use crate::provider_activity;

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

fn configured_key(env_key: &str, env_file: &str) -> Option<String> {
    crate::provider_catalog::load_api_key_from_env_or_config(env_key, env_file)
}

/// Append locally tracked spend ("$ today / month / all-time") when present.
fn push_local_spend(extra_info: &mut Vec<(String, String)>, source_key: &str) {
    if let Some(spend) = provider_activity::spend_snapshot(source_key) {
        extra_info.push((
            "Local spend (this machine)".to_string(),
            format!(
                "${:.2} today · ${:.2} this month · ${:.2} all-time",
                spend.day_usd, spend.month_usd, spend.all_time_usd
            ),
        ));
    }
}

fn key_status_from_response(status: reqwest::StatusCode) -> String {
    if status.is_success() {
        "valid".to_string()
    } else if status.as_u16() == 401 || status.as_u16() == 403 {
        format!("invalid or unauthorized ({})", status.as_u16())
    } else if status.as_u16() == 429 {
        "rate limited (429)".to_string()
    } else {
        format!("check failed ({})", status.as_u16())
    }
}

/// Free `GET {base}/models` probe used for OpenAI-compatible profiles that do
/// not expose a balance API. Returns a human-readable key status.
async fn probe_openai_compatible_key(api_base: &str, api_key: &str) -> String {
    let base = api_base.trim_end_matches('/');
    if base.is_empty() {
        return "configured (no endpoint to probe)".to_string();
    }
    let client = crate::provider::shared_http_client();
    let request = crate::provider_catalog::apply_openai_compatible_catalog_auth(
        client.get(format!("{}/models", base)),
        base,
        api_key,
    );
    let response = request
        .header("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await;
    match response {
        Ok(response) => key_status_from_response(response.status()),
        Err(e) => format!("check failed ({})", e),
    }
}

fn month_start_utc() -> chrono::DateTime<chrono::Utc> {
    use chrono::Datelike;
    let now = chrono::Utc::now();
    now.date_naive()
        .with_day(1)
        .and_then(|d: chrono::NaiveDate| d.and_hms_opt(0, 0, 0))
        .map(|naive: chrono::NaiveDateTime| naive.and_utc())
        .unwrap_or(now)
}

/// Enqueue one task per configured API key worth reporting on. Returns the
/// number of tasks spawned.
pub(super) fn enqueue_api_key_usage_tasks(
    tasks: &mut tokio::task::JoinSet<Option<ProviderUsage>>,
) -> usize {
    let mut total = 0usize;

    if configured_key("ANTHROPIC_API_KEY", "anthropic.env").is_some() {
        tasks.spawn(async { Some(fetch_anthropic_api_key_report().await) });
        total += 1;
    }

    if configured_key("OPENAI_API_KEY", "openai.env").is_some() {
        tasks.spawn(async { Some(fetch_openai_api_key_report().await) });
        total += 1;
    }

    for profile in crate::provider_catalog::openai_compatible_profiles() {
        if !profile.requires_api_key
            || configured_key(profile.api_key_env, profile.env_file).is_none()
        {
            continue;
        }

        let source_key = format!("openai-compatible:{}", profile.id);
        let has_balance_api = matches!(profile.id, "deepseek" | "moonshotai" | "kimi" | "zai");
        // Only surface profiles jcode has actually used (or that expose a real
        // balance API); listing every configured-but-idle key is noise.
        let used_before = provider_activity::last_used_unix_secs(&source_key).is_some()
            || provider_activity::spend_snapshot(&source_key).is_some();
        if !has_balance_api && !used_before {
            continue;
        }

        let profile = *profile;
        tasks.spawn(async move { Some(fetch_compatible_profile_report(profile).await) });
        total += 1;
    }

    total
}

async fn fetch_anthropic_api_key_report() -> ProviderUsage {
    let source_key = "claude:api-key";
    let display_name = "Anthropic API key".to_string();
    let mut extra_info = Vec::new();

    if let Some(api_key) = configured_key("ANTHROPIC_API_KEY", "anthropic.env") {
        let client = crate::provider::shared_http_client();
        let response = client
            .get("https://api.anthropic.com/v1/models?limit=1")
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .timeout(HTTP_TIMEOUT)
            .send()
            .await;
        let status = match response {
            Ok(response) => key_status_from_response(response.status()),
            Err(e) => format!("check failed ({})", e),
        };
        extra_info.push(("Key status".to_string(), status));

        if let Some(admin_key) = configured_key("ANTHROPIC_ADMIN_API_KEY", "anthropic.env")
            && let Some(cost) = fetch_anthropic_org_cost(&admin_key).await
        {
            extra_info.push((
                "Org cost this month (admin API)".to_string(),
                format!("${:.2}", cost),
            ));
        }
    }

    push_local_spend(&mut extra_info, source_key);

    let mut report = ProviderUsage {
        provider_name: display_name,
        extra_info,
        ..Default::default()
    };
    attach_activity(&mut report, source_key);
    report
}

async fn fetch_openai_api_key_report() -> ProviderUsage {
    let source_key = "openai:api-key";
    let display_name = "OpenAI API key".to_string();
    let mut extra_info = Vec::new();

    if let Some(api_key) = configured_key("OPENAI_API_KEY", "openai.env") {
        let client = crate::provider::shared_http_client();
        let response = client
            .get("https://api.openai.com/v1/models")
            .header("Authorization", format!("Bearer {}", api_key))
            .timeout(HTTP_TIMEOUT)
            .send()
            .await;
        let status = match response {
            Ok(response) => key_status_from_response(response.status()),
            Err(e) => format!("check failed ({})", e),
        };
        extra_info.push(("Key status".to_string(), status));

        if let Some(admin_key) = configured_key("OPENAI_ADMIN_API_KEY", "openai.env")
            && let Some(cost) = fetch_openai_org_cost(&admin_key).await
        {
            extra_info.push((
                "Org cost this month (admin API)".to_string(),
                format!("${:.2}", cost),
            ));
        }
    }

    push_local_spend(&mut extra_info, source_key);

    let mut report = ProviderUsage {
        provider_name: display_name,
        extra_info,
        ..Default::default()
    };
    attach_activity(&mut report, source_key);
    report
}

async fn fetch_compatible_profile_report(
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
) -> ProviderUsage {
    let source_key = format!("openai-compatible:{}", profile.id);
    let mut extra_info = Vec::new();
    let mut limits: Vec<UsageLimit> = Vec::new();

    match profile.id {
        "deepseek" => {
            if let Some(api_key) = configured_key(profile.api_key_env, profile.env_file) {
                match fetch_deepseek_balance(&api_key).await {
                    Ok(lines) => extra_info.extend(lines),
                    Err(e) => {
                        extra_info.push(("Balance".to_string(), format!("unavailable ({})", e)))
                    }
                }
            }
        }
        "moonshotai" => {
            if let Some(api_key) = configured_key(profile.api_key_env, profile.env_file) {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
                match fetch_moonshot_balance(&api_key, &resolved.api_base).await {
                    Ok(lines) => extra_info.extend(lines),
                    Err(e) => {
                        extra_info.push(("Balance".to_string(), format!("unavailable ({})", e)))
                    }
                }
            }
        }
        "kimi" => {
            if let Some(api_key) = configured_key(profile.api_key_env, profile.env_file) {
                match fetch_kimi_usage_limits(&api_key).await {
                    Ok(fetched) => limits.extend(fetched),
                    Err(e) => {
                        extra_info.push(("Usage".to_string(), format!("unavailable ({})", e)))
                    }
                }
            }
            // Kimi for Coding is a flat-rate subscription, so the
            // pay-as-you-go equivalent from the local spend ledger is
            // meaningless here; skip push_local_spend.
        }
        "zai" => {
            if let Some(api_key) = configured_key(profile.api_key_env, profile.env_file) {
                match fetch_zai_coding_plan_limits(&api_key).await {
                    Ok(fetched) if !fetched.is_empty() => limits.extend(fetched),
                    _ => {
                        // Not a Coding Plan key (or quota API unavailable):
                        // fall back to the pay-as-you-go key probe below.
                        let resolved =
                            crate::provider_catalog::resolve_openai_compatible_profile(profile);
                        let status =
                            probe_openai_compatible_key(&resolved.api_base, &api_key).await;
                        extra_info.push(("Key status".to_string(), status));
                    }
                }
            }
        }
        _ => {
            // No provider-specific balance API: do a free `GET /models` probe
            // against the profile's own endpoint so the key status is real
            // rather than just "configured".
            if let Some(api_key) = configured_key(profile.api_key_env, profile.env_file) {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
                let status = probe_openai_compatible_key(&resolved.api_base, &api_key).await;
                extra_info.push(("Key status".to_string(), status));
            }
        }
    }

    // Local pay-as-you-go spend is meaningless next to a flat-rate plan
    // quota (Kimi, or a Z.ai Coding Plan key that returned windows).
    let plan_quota_shown = profile.id == "kimi" || (profile.id == "zai" && !limits.is_empty());
    if !plan_quota_shown {
        push_local_spend(&mut extra_info, &source_key);
    }

    let mut report = ProviderUsage {
        provider_name: format!("{} (API key)", profile.display_name),
        limits,
        extra_info,
        ..Default::default()
    };
    attach_activity(&mut report, &source_key);
    report
}

/// DeepSeek exposes a real balance endpoint for plain API keys.
async fn fetch_deepseek_balance(api_key: &str) -> Result<Vec<(String, String)>> {
    let client = crate::provider::shared_http_client();
    let response = client
        .get("https://api.deepseek.com/user/balance")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .context("balance request failed")?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }
    let json: serde_json::Value = response.json().await.context("invalid balance response")?;

    let mut lines = Vec::new();
    if let Some(available) = json.get("is_available").and_then(|v| v.as_bool()) {
        lines.push((
            "Key status".to_string(),
            if available {
                "valid (balance available)".to_string()
            } else {
                "balance exhausted".to_string()
            },
        ));
    }
    if let Some(infos) = json.get("balance_infos").and_then(|v| v.as_array()) {
        for info in infos {
            let currency = info.get("currency").and_then(|v| v.as_str()).unwrap_or("?");
            let total = info
                .get("total_balance")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let mut detail = format!("{} {}", total, currency);
            let topped_up = info.get("topped_up_balance").and_then(|v| v.as_str());
            let granted = info.get("granted_balance").and_then(|v| v.as_str());
            if let (Some(topped_up), Some(granted)) = (topped_up, granted) {
                detail.push_str(&format!(" ({} paid + {} granted)", topped_up, granted));
            }
            lines.push(("Balance".to_string(), detail));
        }
    }
    if lines.is_empty() {
        anyhow::bail!("no balance info in response");
    }
    Ok(lines)
}

/// Moonshot exposes `GET /v1/users/me/balance` for plain API keys.
async fn fetch_moonshot_balance(api_key: &str, api_base: &str) -> Result<Vec<(String, String)>> {
    let base = api_base.trim_end_matches('/');
    let currency = if base.contains("moonshot.cn") {
        "CNY"
    } else {
        "USD"
    };
    let client = crate::provider::shared_http_client();
    let response = client
        .get(format!("{}/users/me/balance", base))
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .context("balance request failed")?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }
    let json: serde_json::Value = response.json().await.context("invalid balance response")?;
    let data = json
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("no balance data in response"))?;

    let mut lines = Vec::new();
    if let Some(available) = data.get("available_balance").and_then(|v| v.as_f64()) {
        lines.push((
            "Balance".to_string(),
            format!("{:.2} {} available", available, currency),
        ));
        lines.push((
            "Key status".to_string(),
            if available > 0.0 {
                "valid (balance available)".to_string()
            } else {
                "balance exhausted".to_string()
            },
        ));
    }
    if let (Some(cash), Some(voucher)) = (
        data.get("cash_balance").and_then(|v| v.as_f64()),
        data.get("voucher_balance").and_then(|v| v.as_f64()),
    ) {
        lines.push((
            "Balance breakdown".to_string(),
            format!("{:.2} cash + {:.2} voucher {}", cash, voucher, currency),
        ));
    }
    if lines.is_empty() {
        anyhow::bail!("no balance fields in response");
    }
    Ok(lines)
}

/// Kimi Code exposes per-window quota through the coding usage endpoint.
async fn fetch_kimi_usage_limits(api_key: &str) -> Result<Vec<UsageLimit>> {
    let client = crate::provider::shared_http_client();
    let response = client
        .get("https://api.kimi.com/coding/v1/usages")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .context("usage request failed")?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }
    let json: serde_json::Value = response.json().await.context("invalid usage response")?;
    Ok(parse_kimi_usage_limits(&json))
}

/// Pure parse of the Kimi usage payload: a `usage` summary plus `limits[]`
/// (each with a `window` descriptor) or a legacy `usages` map (`limit_5h` /
/// `limit_7d` / `limit_30d`). Every documented shape is accepted because the
/// backend has shipped all of them at different times.
pub(super) fn parse_kimi_usage_limits(json: &serde_json::Value) -> Vec<UsageLimit> {
    let mut limits: Vec<UsageLimit> = Vec::new();

    if let Some(usage) = json.get("usage")
        && let Some(limit) = kimi_parse_row(usage, "Weekly")
    {
        limits.push(limit);
    }

    if let Some(raw_limits) = json.get("limits").and_then(|v| v.as_array()) {
        for raw in raw_limits {
            let window = raw
                .get("window")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let duration = window.get("duration").and_then(|v| v.as_f64());
            let unit = window
                .get("timeUnit")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_uppercase();
            let minutes = duration.map(|d| {
                if unit.contains("HOUR") {
                    d * 60.0
                } else if unit.contains("DAY") {
                    d * 1440.0
                } else {
                    d
                }
            });
            let fallback = format!("Limit {}", limits.len() + 1);
            let label = kimi_window_label(minutes, &fallback);
            let detail = raw.get("detail").unwrap_or(raw);
            if let Some(limit) = kimi_parse_row(detail, &label) {
                limits.push(limit);
            }
        }
    }

    if let Some(usages) = json.get("usages").and_then(|v| v.as_object()) {
        for (key, raw) in usages {
            let minutes = match key.as_str() {
                "limit_5h" => Some(300.0),
                "limit_7d" => Some(10080.0),
                "limit_30d" => Some(43200.0),
                _ => None,
            };
            let label = kimi_window_label(minutes, key);
            let usage_percent = raw
                .get("used_ratio")
                .and_then(|v| v.as_f64())
                .map(|ratio| (ratio * 100.0).clamp(0.0, 100.0) as f32);
            let resets_at = ["reset_time", "resetTime"]
                .iter()
                .find_map(|k| raw.get(k).and_then(reset_timestamp_from_json_value));
            if let Some(usage_percent) = usage_percent {
                limits.push(UsageLimit {
                    name: label,
                    usage_percent,
                    resets_at,
                });
            }
        }
    }

    // The summary/legacy shapes can repeat a window the limits[] shape already
    // reported; the first occurrence wins.
    let mut seen = std::collections::HashSet::new();
    limits.retain(|limit| seen.insert(limit.name.clone()));

    let rank = |name: &str| match name {
        "5-hour" => 0,
        "Weekly" => 1,
        "Monthly" => 2,
        _ => 3,
    };
    limits.sort_by_key(|limit| rank(&limit.name));
    limits
}

fn kimi_parse_row(raw: &serde_json::Value, label: &str) -> Option<UsageLimit> {
    let limit = raw.get("limit").and_then(|v| v.as_f64());
    let used = raw.get("used").and_then(|v| v.as_f64()).or_else(|| {
        match (limit, raw.get("remaining").and_then(|v| v.as_f64())) {
            (Some(limit), Some(remaining)) => Some((limit - remaining).max(0.0)),
            _ => None,
        }
    });
    if used.is_none() && limit.is_none() {
        return None;
    }
    let usage_percent = raw
        .get("percentage")
        .and_then(|v| v.as_f64())
        .map(|pct| pct.clamp(0.0, 100.0) as f32)
        .or_else(|| match (used, limit) {
            (Some(used), Some(limit)) => Some(usage_percent_from_used_limit(used, limit)),
            _ => None,
        })?;
    let resets_at = ["resetTime", "resetAt", "reset_time"]
        .iter()
        .find_map(|key| raw.get(key).and_then(reset_timestamp_from_json_value));
    Some(UsageLimit {
        name: label.to_string(),
        usage_percent,
        resets_at,
    })
}

fn kimi_window_label(minutes: Option<f64>, fallback: &str) -> String {
    match minutes {
        Some(300.0) => "5-hour".to_string(),
        Some(10080.0) => "Weekly".to_string(),
        Some(m) if (43200.0..=44640.0).contains(&m) => "Monthly".to_string(),
        Some(m) if m > 0.0 && m % 1440.0 == 0.0 => format!("{}d limit", kimi_fmt_num(m / 1440.0)),
        Some(m) if m > 0.0 && m % 60.0 == 0.0 => format!("{}h limit", kimi_fmt_num(m / 60.0)),
        _ => fallback.to_string(),
    }
}

fn kimi_fmt_num(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{}", value)
    }
}

/// Z.ai GLM Coding Plan exposes plan quota windows (5-hour, weekly, and MCP
/// monthly) through the monitor quota endpoint. Pay-as-you-go keys are
/// rejected, which the caller uses as the fallback signal.
async fn fetch_zai_coding_plan_limits(api_key: &str) -> Result<Vec<UsageLimit>> {
    let client = crate::provider::shared_http_client();
    let response = client
        .get("https://open.bigmodel.cn/api/monitor/usage/quota/limit")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .context("usage request failed")?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }
    let json: serde_json::Value = response.json().await.context("invalid usage response")?;
    Ok(parse_zai_coding_plan_limits(&json))
}

/// Pure parse of the Coding Plan quota payload: `limits[]` rows are either
/// `TOKENS_LIMIT` windows (`unit` 6 is the weekly bucket, anything else the
/// 5-hour bucket) or a `TIME_LIMIT` row for MCP monthly usage.
pub(super) fn parse_zai_coding_plan_limits(json: &serde_json::Value) -> Vec<UsageLimit> {
    let root = json.get("data").filter(|v| v.is_object()).unwrap_or(json);
    let Some(raw_limits) = root.get("limits").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut limits = Vec::new();
    for raw in raw_limits {
        let resets_at = ["nextResetTime", "resetAt", "reset_time"]
            .iter()
            .find_map(|key| raw.get(key).and_then(reset_timestamp_from_json_value));
        let percent = raw
            .get("percentage")
            .and_then(|v| v.as_f64())
            .map(|pct| pct.clamp(0.0, 100.0) as f32);
        let (name, usage_percent) = match raw.get("type").and_then(|v| v.as_str()) {
            Some("TOKENS_LIMIT") => {
                let unit = raw.get("unit").and_then(|v| v.as_f64());
                let name = if unit == Some(6.0) {
                    "Weekly"
                } else {
                    "5-hour"
                };
                let Some(usage_percent) = percent else {
                    continue;
                };
                (name, usage_percent)
            }
            Some("TIME_LIMIT") => {
                let usage_percent = percent.or_else(|| {
                    match (
                        raw.get("currentValue").and_then(|v| v.as_f64()),
                        raw.get("usage").and_then(|v| v.as_f64()),
                    ) {
                        (Some(current), Some(limit)) if limit > 0.0 => {
                            Some(((current / limit) * 100.0).clamp(0.0, 100.0) as f32)
                        }
                        _ => None,
                    }
                });
                let Some(usage_percent) = usage_percent else {
                    continue;
                };
                ("MCP monthly", usage_percent)
            }
            _ => continue,
        };
        limits.push(UsageLimit {
            name: name.to_string(),
            usage_percent,
            resets_at,
        });
    }

    let rank = |name: &str| match name {
        "5-hour" => 0,
        "Weekly" => 1,
        _ => 2,
    };
    limits.sort_by_key(|limit| rank(&limit.name));
    limits
}

/// Anthropic org-wide cost for the current month. Requires an *admin* API key
/// (`sk-ant-admin...`); regular keys cannot read cost reports.
async fn fetch_anthropic_org_cost(admin_key: &str) -> Option<f64> {
    let starting_at = month_start_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let client = crate::provider::shared_http_client();
    let response = client
        .get(format!(
            "https://api.anthropic.com/v1/organizations/cost_report?starting_at={}&limit=31",
            starting_at
        ))
        .header("x-api-key", admin_key)
        .header("anthropic-version", "2023-06-01")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let json: serde_json::Value = response.json().await.ok()?;
    let mut total = 0.0_f64;
    let mut saw_any = false;
    for bucket in json.get("data")?.as_array()? {
        let Some(results) = bucket.get("results").and_then(|v| v.as_array()) else {
            continue;
        };
        for result in results {
            let amount = result.get("amount");
            let value = amount
                .and_then(|v| v.as_f64())
                .or_else(|| amount.and_then(|v| v.as_str()).and_then(|s| s.parse().ok()));
            if let Some(value) = value {
                total += value;
                saw_any = true;
            }
        }
    }
    saw_any.then_some(total)
}

/// OpenAI org-wide cost for the current month. Requires an admin API key.
async fn fetch_openai_org_cost(admin_key: &str) -> Option<f64> {
    let start_time = month_start_utc().timestamp();
    let client = crate::provider::shared_http_client();
    let response = client
        .get(format!(
            "https://api.openai.com/v1/organization/costs?start_time={}&limit=31",
            start_time
        ))
        .header("Authorization", format!("Bearer {}", admin_key))
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let json: serde_json::Value = response.json().await.ok()?;
    let mut total = 0.0_f64;
    let mut saw_any = false;
    for bucket in json.get("data")?.as_array()? {
        let Some(results) = bucket.get("results").and_then(|v| v.as_array()) else {
            continue;
        };
        for result in results {
            if let Some(value) = result
                .get("amount")
                .and_then(|amount| amount.get("value"))
                .and_then(|v| v.as_f64())
            {
                total += value;
                saw_any = true;
            }
        }
    }
    saw_any.then_some(total)
}
