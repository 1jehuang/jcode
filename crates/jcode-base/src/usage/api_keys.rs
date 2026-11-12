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
        let has_balance_api = matches!(profile.id, "deepseek" | "moonshotai");
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

    push_local_spend(&mut extra_info, &source_key);

    let mut report = ProviderUsage {
        provider_name: format!("{} (API key)", profile.display_name),
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
