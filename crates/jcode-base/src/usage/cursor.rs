//! Account-bound Cursor quota reader.
//!
//! Cursor's dashboard APIs are undocumented and can change shape. This module
//! therefore treats every response as untrusted input: only bounded, finite
//! values become quota state, and errors never overwrite a last-good snapshot.

use super::{ProviderUsage, UsageLimit};
use crate::auth;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OnceCell, Semaphore};

const DASHBOARD_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const SUMMARY_URL: &str = "https://api2.cursor.sh/api/usage/summary";
const AUTH_USAGE_URL: &str = "https://api2.cursor.sh/auth/usage";
const WEB_USAGE_URL: &str = "https://cursor.com/api/usage-summary";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_CACHE_ENTRIES: usize = 100;
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_PERCENT: f64 = 100.0;

static REQUEST_LIMIT: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(4)));
static HTTP_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(crate::provider::JCODE_USER_AGENT)
        .build()
        .unwrap_or_else(|_| Client::new())
});
static LAST_GOOD: LazyLock<Mutex<HashMap<String, (Instant, CursorUsageSnapshot)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, (Instant, Arc<OnceCell<CursorFetchResult>>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const IN_FLIGHT_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
struct CursorUsageSnapshot {
    limits: Vec<UsageLimit>,
    quotas: Vec<(String, Option<u16>, Option<String>)>,
    extra_info: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct CursorFetchResult {
    snapshot: Option<CursorUsageSnapshot>,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Dashboard,
    Summary,
    AuthUsage,
    WebSummary,
}

impl Endpoint {
    const fn url(self) -> &'static str {
        match self {
            Self::Dashboard => DASHBOARD_URL,
            Self::Summary => SUMMARY_URL,
            Self::AuthUsage => AUTH_USAGE_URL,
            Self::WebSummary => WEB_USAGE_URL,
        }
    }

    const fn is_post(self) -> bool {
        matches!(self, Self::Dashboard)
    }
}

fn endpoint_host_is_trusted(endpoint: Endpoint, url: &reqwest::Url) -> bool {
    let trusted_host = match endpoint {
        Endpoint::WebSummary => "cursor.com",
        Endpoint::Dashboard | Endpoint::Summary | Endpoint::AuthUsage => "api2.cursor.sh",
    };
    url.scheme() == "https" && url.host_str() == Some(trusted_host)
}

fn finite_number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(string) => string.trim().parse().ok()?,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

fn normalized_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn object_value<'a>(
    object: &'a serde_json::Map<String, Value>,
    aliases: &[&str],
) -> Option<&'a Value> {
    object.iter().find_map(|(key, value)| {
        aliases
            .iter()
            .any(|alias| normalized_key(key) == normalized_key(alias))
            .then_some(value)
    })
}

fn find_value<'a>(root: &'a Value, aliases: &[&str], depth: usize) -> Option<&'a Value> {
    let Value::Object(object) = root else {
        return None;
    };
    if let Some(value) = object_value(object, aliases) {
        return Some(value);
    }
    if depth >= 5 {
        return None;
    }
    object
        .values()
        .find_map(|value| find_value(value, aliases, depth + 1))
}

fn find_object<'a>(
    root: &'a Value,
    aliases: &[&str],
) -> Option<&'a serde_json::Map<String, Value>> {
    match find_value(root, aliases, 0)? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

fn reset_time(root: &Value, object: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    let value = object
        .and_then(|value| object_value(value, &["resetAt", "reset_at", "resetsAt", "reset"]))
        .or_else(|| {
            find_value(
                root,
                &[
                    "billingCycleEnd",
                    "billing_cycle_end",
                    "resetAt",
                    "reset_at",
                    "resetsAt",
                    "endOfMonth",
                ],
                0,
            )
        })?;
    let raw = match value {
        Value::String(string) => string.trim().to_string(),
        Value::Number(_) => value.to_string(),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    if let Ok(epoch) = raw.parse::<i64>() {
        let millis = if epoch.unsigned_abs() < 100_000_000_000 {
            epoch.saturating_mul(1000)
        } else {
            epoch
        };
        return chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis)
            .map(|value| value.to_rfc3339());
    }
    Some(raw)
}

fn percent(value: Option<&Value>) -> Option<f32> {
    let value = finite_number(value?)?;
    if !(0.0..=MAX_PERCENT).contains(&value) {
        return None;
    }
    let value = if value <= 1.0 { value * 100.0 } else { value };
    Some(value.clamp(0.0, MAX_PERCENT) as f32)
}

fn explicit_percent(value: Option<&Value>) -> Option<f32> {
    let value = finite_number(value?)?;
    (0.0..=MAX_PERCENT).contains(&value).then_some(value as f32)
}

fn cents(value: &Value) -> Option<f64> {
    let value = finite_number(value)?;
    (value >= 0.0).then_some(value)
}

fn pair(object: &serde_json::Map<String, Value>) -> Option<(f64, f64)> {
    let used = object_value(
        object,
        &["used", "usage", "numRequests", "requests", "includedSpend"],
    )
    .and_then(cents)?;
    let limit = object_value(
        object,
        &[
            "limit",
            "maxRequestUsage",
            "maxRequests",
            "requestLimit",
            "includedLimit",
            "quota",
        ],
    )
    .and_then(cents)
    .or_else(|| {
        object_value(
            object,
            &["remaining", "remainingRequests", "requestsRemaining"],
        )
        .and_then(cents)
        .map(|remaining| used + remaining)
    })?;
    (limit > 0.0 && used <= limit * 10_000.0).then_some((used, limit))
}

fn percent_from_object(
    root: &Value,
    object: Option<&serde_json::Map<String, Value>>,
    aliases: &[&str],
) -> Option<f32> {
    if let Some(object) = object {
        for alias in aliases {
            if let Some(value) = object_value(object, &[*alias]) {
                let parsed = if normalized_key(alias).contains("percent") {
                    explicit_percent(Some(value))
                } else {
                    percent(Some(value))
                };
                if let Some(value) = parsed {
                    return Some(value);
                }
            }
        }
        if let Some((used, limit)) = pair(object) {
            return Some(((used / limit) * 100.0).clamp(0.0, MAX_PERCENT) as f32);
        }
    }
    aliases.iter().find_map(|alias| {
        let value = find_value(root, &[alias], 0);
        if normalized_key(alias).contains("percent") {
            explicit_percent(value)
        } else {
            percent(value)
        }
    })
}

fn add_window(
    snapshot: &mut CursorUsageSnapshot,
    root: &Value,
    name: &str,
    object: Option<&serde_json::Map<String, Value>>,
    aliases: &[&str],
) {
    let Some(usage_percent) = percent_from_object(root, object, aliases) else {
        return;
    };
    let reset = reset_time(root, object);
    let remaining = ((100.0 - f64::from(usage_percent)) * 10.0).round();
    let remaining = (remaining as i64).clamp(0, 1000) as u16;
    snapshot.limits.push(UsageLimit {
        name: name.to_string(),
        usage_percent,
        resets_at: reset.clone(),
    });
    snapshot
        .quotas
        .push((name.to_ascii_lowercase(), Some(remaining), reset));
}

fn dashboard_snapshot(root: &Value) -> Option<CursorUsageSnapshot> {
    let plan = find_object(root, &["planUsage", "plan_usage"])?;
    let mut snapshot = CursorUsageSnapshot {
        limits: Vec::new(),
        quotas: Vec::new(),
        extra_info: Vec::new(),
    };
    add_window(
        &mut snapshot,
        root,
        "Monthly",
        Some(plan),
        &["totalPercentUsed", "percentUsed", "usagePercent"],
    );
    if snapshot.limits.is_empty() {
        add_window(
            &mut snapshot,
            root,
            "Monthly",
            None,
            &["totalPercentUsed", "percentUsed", "usagePercent"],
        );
    }
    add_window(
        &mut snapshot,
        root,
        "Auto",
        Some(plan),
        &["autoPercentUsed", "autoUsagePercent", "auto"],
    );
    add_window(
        &mut snapshot,
        root,
        "API",
        Some(plan),
        &["apiPercentUsed", "apiUsagePercent", "api"],
    );

    let used = object_value(plan, &["includedSpend", "totalSpend"]).and_then(cents);
    let limit = object_value(plan, &["limit"]).and_then(cents);
    if let (Some(used), Some(limit)) = (used, limit.filter(|value| *value > 0.0)) {
        snapshot.extra_info.push((
            "Plan spend".to_string(),
            format!("{} / {} cents", used, limit),
        ));
    }
    (!snapshot.limits.is_empty()).then_some(snapshot)
}

fn fallback_snapshot(root: &Value) -> Option<CursorUsageSnapshot> {
    let mut snapshot = CursorUsageSnapshot {
        limits: Vec::new(),
        quotas: Vec::new(),
        extra_info: Vec::new(),
    };
    let monthly_object = find_object(
        root,
        &["monthly", "monthlyUsage", "usage", "included", "planUsage"],
    );
    add_window(
        &mut snapshot,
        root,
        "Monthly",
        monthly_object,
        &[
            "totalPercentUsed",
            "percentUsed",
            "usagePercent",
            "monthlyPercentUsed",
            "monthly",
        ],
    );
    add_window(
        &mut snapshot,
        root,
        "Auto",
        find_object(root, &["auto", "autoUsage"]),
        &["autoPercentUsed", "percentUsed", "usagePercent", "auto"],
    );
    add_window(
        &mut snapshot,
        root,
        "API",
        find_object(root, &["api", "apiUsage"]),
        &["apiPercentUsed", "percentUsed", "usagePercent", "api"],
    );

    if snapshot.limits.is_empty() {
        collect_first_bucket(root, &mut snapshot, 0);
    }
    (!snapshot.limits.is_empty()).then_some(snapshot)
}

fn collect_first_bucket(root: &Value, snapshot: &mut CursorUsageSnapshot, depth: usize) {
    if depth > 5 || !snapshot.limits.is_empty() {
        return;
    }
    let Value::Object(object) = root else { return };
    if let Some((used, limit)) = pair(object) {
        let usage_percent = ((used / limit) * 100.0).clamp(0.0, MAX_PERCENT) as f32;
        let reset = reset_time(root, Some(object));
        let remaining = ((100.0 - f64::from(usage_percent)) * 10.0)
            .round()
            .clamp(0.0, 1000.0) as u16;
        snapshot.limits.push(UsageLimit {
            name: "Monthly".to_string(),
            usage_percent,
            resets_at: reset.clone(),
        });
        snapshot
            .quotas
            .push(("monthly".to_string(), Some(remaining), reset));
        return;
    }
    for (key, value) in object {
        if matches!(
            normalized_key(key).as_str(),
            "metadata" | "message" | "error" | "user" | "team"
        ) {
            continue;
        }
        collect_first_bucket(value, snapshot, depth + 1);
    }
}

fn parse_snapshot(endpoint: Endpoint, body: &[u8]) -> Option<CursorUsageSnapshot> {
    let root: Value = serde_json::from_slice(body).ok()?;
    match endpoint {
        Endpoint::Dashboard => dashboard_snapshot(&root).or_else(|| fallback_snapshot(&root)),
        Endpoint::Summary | Endpoint::AuthUsage | Endpoint::WebSummary => fallback_snapshot(&root),
    }
}

async fn endpoint_response(
    endpoint: Endpoint,
    token: &str,
) -> Result<(StatusCode, Vec<u8>), &'static str> {
    let url = endpoint.url();
    let parsed = reqwest::Url::parse(url).map_err(|_| "invalid Cursor endpoint")?;
    if !endpoint_host_is_trusted(endpoint, &parsed) {
        return Err("invalid Cursor endpoint");
    }
    let request = if endpoint.is_post() {
        HTTP_CLIENT
            .post(url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .json(&serde_json::json!({}))
    } else {
        HTTP_CLIENT.get(url)
    };
    let response = tokio::time::timeout(
        REQUEST_TIMEOUT,
        request
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send(),
    )
    .await
    .map_err(|_| "Cursor usage request timed out")?
    .map_err(|_| "Cursor usage request failed")?;
    if response.url().scheme() != "https" || response.url().host_str() != parsed.host_str() {
        return Err("Cursor usage redirected to an untrusted host");
    }
    let status = response.status();
    let body = tokio::time::timeout(REQUEST_TIMEOUT, response.bytes())
        .await
        .map_err(|_| "Cursor usage response timed out")?
        .map_err(|_| "Cursor usage response failed")?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err("Cursor usage response was too large");
    }
    Ok((status, body.to_vec()))
}

fn fallback_error(statuses: &[StatusCode]) -> String {
    if statuses
        .iter()
        .any(|status| *status == StatusCode::TOO_MANY_REQUESTS)
    {
        "Cursor usage unavailable (rate limited; quota is unknown)".to_string()
    } else if statuses
        .iter()
        .any(|status| *status == StatusCode::UNAUTHORIZED)
    {
        "Cursor usage unavailable (authentication required; quota is unknown)".to_string()
    } else {
        "Cursor usage unavailable (quota is unknown)".to_string()
    }
}

async fn fetch_live(token: &str, allow_web_summary: bool) -> CursorFetchResult {
    let _permit = REQUEST_LIMIT.acquire().await;
    let endpoints = if allow_web_summary {
        vec![
            Endpoint::Dashboard,
            Endpoint::Summary,
            Endpoint::AuthUsage,
            Endpoint::WebSummary,
        ]
    } else {
        vec![Endpoint::Dashboard, Endpoint::Summary, Endpoint::AuthUsage]
    };
    let mut statuses = Vec::new();
    for endpoint in endpoints {
        match endpoint_response(endpoint, token).await {
            Ok((status, body)) => {
                statuses.push(status);
                if status.is_success() {
                    if let Some(snapshot) = parse_snapshot(endpoint, &body) {
                        return CursorFetchResult {
                            snapshot: Some(snapshot),
                            error: None,
                        };
                    }
                }
            }
            Err(_) => {}
        }
    }
    CursorFetchResult {
        snapshot: None,
        error: Some(fallback_error(&statuses)),
    }
}

fn cached_snapshot(key: &str) -> Option<CursorUsageSnapshot> {
    LAST_GOOD.lock().ok().and_then(|cache| {
        cache
            .get(key)
            .and_then(|(at, snapshot)| (at.elapsed() <= CACHE_TTL).then_some(snapshot.clone()))
    })
}

fn remember_snapshot(key: String, snapshot: CursorUsageSnapshot) {
    if let Ok(mut cache) = LAST_GOOD.lock() {
        if cache.len() >= MAX_CACHE_ENTRIES && !cache.contains_key(&key) {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(key, (Instant::now(), snapshot));
    }
}

async fn fetch_deduplicated(
    key: String,
    token: &str,
    allow_web_summary: bool,
) -> CursorFetchResult {
    let cell = {
        let mut cells = IN_FLIGHT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cells
            .get(&key)
            .is_some_and(|(created, _)| created.elapsed() > IN_FLIGHT_TTL)
        {
            cells.remove(&key);
        }
        if cells.len() >= MAX_CACHE_ENTRIES && !cells.contains_key(&key) {
            if let Some(oldest) = cells.keys().next().cloned() {
                cells.remove(&oldest);
            }
        }
        cells
            .entry(key)
            .or_insert_with(|| (Instant::now(), Arc::new(OnceCell::new())))
            .1
            .clone()
    };
    cell.get_or_init(|| async { fetch_live(token, allow_web_summary).await })
        .await
        .clone()
}

pub(super) async fn fetch_cursor_usage_for_account(
    account: crate::auth::provider_pool::ManagedProviderAccount,
) -> ProviderUsage {
    let display_name = format!("Cursor - {}", account.label);
    let key = format!("{}:{}", account.id, account.label);
    let client = crate::provider::shared_http_client();
    let tokens = match auth::cursor::resolve_direct_tokens_for_account(&client, &account).await {
        Ok(tokens) => tokens,
        Err(_) => {
            let snapshot = cached_snapshot(&key).or_else(|| quota_snapshot(&account.label));
            return report_from_result(
                display_name,
                snapshot,
                "Cursor usage unavailable (authentication required; quota is unknown)",
                false,
            );
        }
    };
    let allow_web = auth::cursor::token_is_bound_to_account(&tokens.access_token, &account);
    let result = fetch_deduplicated(key.clone(), &tokens.access_token, allow_web).await;
    if let Some(snapshot) = result.snapshot.clone() {
        remember_snapshot(key, snapshot.clone());
        report_from_result(display_name, Some(snapshot), "", true)
    } else {
        let snapshot = cached_snapshot(&key).or_else(|| quota_snapshot(&account.label));
        report_from_result(
            display_name,
            snapshot,
            result
                .error
                .as_deref()
                .unwrap_or("Cursor usage unavailable (quota is unknown)"),
            false,
        )
    }
}

fn quota_snapshot(label: &str) -> Option<CursorUsageSnapshot> {
    let snapshots = auth::provider_pool::account_quota_snapshots("cursor", label);
    if snapshots.is_empty() {
        return None;
    }
    let mut result = CursorUsageSnapshot {
        limits: Vec::new(),
        quotas: Vec::new(),
        extra_info: Vec::new(),
    };
    for (model, snapshot) in snapshots {
        let Some(remaining) = snapshot.remaining_fraction_milli else {
            continue;
        };
        let name = match model.as_str() {
            "monthly" => "Monthly",
            "auto" => "Auto",
            "api" => "API",
            _ => continue,
        };
        result.limits.push(UsageLimit {
            name: name.to_string(),
            usage_percent: (1000_u16.saturating_sub(remaining) as f32) / 10.0,
            resets_at: snapshot.reset_time.clone(),
        });
        result
            .quotas
            .push((model, Some(remaining), snapshot.reset_time));
    }
    (!result.limits.is_empty()).then_some(result)
}

fn report_from_result(
    display_name: String,
    snapshot: Option<CursorUsageSnapshot>,
    error: &str,
    record_quota: bool,
) -> ProviderUsage {
    let mut report = ProviderUsage {
        provider_name: display_name,
        ..Default::default()
    };
    if let Some(snapshot) = snapshot {
        if record_quota {
            for (model, remaining, reset) in snapshot.quotas {
                auth::provider_pool::record_account_quota(
                    "cursor",
                    report
                        .provider_name
                        .strip_prefix("Cursor - ")
                        .unwrap_or("cursor"),
                    &model,
                    remaining,
                    reset,
                );
            }
        }
        report.limits = snapshot.limits;
        report.extra_info = snapshot.extra_info;
        report.extra_info.push((
            "Usage API".to_string(),
            "unofficial Cursor dashboard API (reverse-engineered)".to_string(),
        ));
    } else {
        report.extra_info.push((
            "Usage API".to_string(),
            "unofficial Cursor dashboard API (reverse-engineered)".to_string(),
        ));
    }
    if !error.is_empty() {
        report.error = Some(error.to_string());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(value: &str) -> Vec<u8> {
        value.as_bytes().to_vec()
    }

    #[test]
    fn dashboard_parses_percent_cents_and_reset_timestamp() {
        let snapshot = parse_snapshot(Endpoint::Dashboard, &json(r#"{
            "billingCycleEnd":"1735689600000",
            "planUsage":{"includedSpend":"23222","limit":"40000","totalPercentUsed":58.055,"autoPercentUsed":25,"apiPercentUsed":"12.5"}
        }"#)).expect("dashboard should parse");
        assert_eq!(snapshot.limits[0].name, "Monthly");
        assert!((snapshot.limits[0].usage_percent - 58.055).abs() < 0.01);
        assert_eq!(snapshot.limits[1].name, "Auto");
        assert!((snapshot.limits[1].usage_percent - 25.0).abs() < 0.01);
        assert_eq!(snapshot.quotas[0].1, Some(419));
        assert!(
            snapshot.limits[0]
                .resets_at
                .as_deref()
                .unwrap_or_default()
                .contains("2025")
        );
        assert!(snapshot.extra_info[0].1.contains("23222"));
    }

    #[test]
    fn fallback_parses_auth_request_buckets_and_reset() {
        let snapshot = parse_snapshot(
            Endpoint::AuthUsage,
            &json(
                r#"{
            "startOfMonth":"2026-09-01T00:00:00Z",
            "gpt-4":{"numRequests":"40","maxRequestUsage":"460","resetAt":"2026-10-01T00:00:00Z"}
        }"#,
            ),
        )
        .expect("auth fallback should parse");
        assert_eq!(snapshot.limits[0].name, "Monthly");
        assert!((snapshot.limits[0].usage_percent - 8.695).abs() < 0.01);
        assert_eq!(
            snapshot.limits[0].resets_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
    }

    #[test]
    fn invalid_values_are_unknown_not_exhausted() {
        assert!(
            parse_snapshot(
                Endpoint::Dashboard,
                &json(r#"{"planUsage":{"limit":0,"includedSpend":10}}"#)
            )
            .is_none()
        );
        assert!(parse_snapshot(Endpoint::Summary, &json("[]")).is_none());
    }

    #[test]
    fn endpoint_fallback_order_and_web_binding_gate_are_explicit() {
        let dashboard = parse_snapshot(Endpoint::Dashboard, &json("{}"));
        let summary = parse_snapshot(Endpoint::Summary, &json("{}"));
        let auth = parse_snapshot(
            Endpoint::AuthUsage,
            &json(r#"{"monthly":{"used":1,"limit":10}}"#),
        );
        assert!(dashboard.is_none());
        assert!(summary.is_none());
        assert!(auth.is_some());
        assert_eq!(Endpoint::Dashboard.url(), DASHBOARD_URL);
        assert!(Endpoint::Dashboard.is_post());
        assert_eq!(Endpoint::WebSummary.url(), WEB_USAGE_URL);
    }

    #[test]
    fn web_summary_endpoint_is_allowlisted_without_trusting_api2_as_web_host() {
        let web = reqwest::Url::parse(Endpoint::WebSummary.url()).expect("web URL");
        let api2 = reqwest::Url::parse(Endpoint::Dashboard.url()).expect("api2 URL");
        assert!(endpoint_host_is_trusted(Endpoint::WebSummary, &web));
        assert!(!endpoint_host_is_trusted(Endpoint::WebSummary, &api2));
        assert!(endpoint_host_is_trusted(Endpoint::Dashboard, &api2));
    }

    #[test]
    fn web_summary_requires_and_accepts_matching_account_identity() {
        let token = "header.eyJzdWIiOiJvdGhlciJ9.signature";
        let account = crate::auth::provider_pool::ManagedProviderAccount {
            id: "expected".into(),
            label: "cursor-test".into(),
            access_token: token.into(),
            refresh_token: "refresh".into(),
            expires_at: 0,
            email: None,
            project_id: None,
        };
        assert!(!auth::cursor::token_is_bound_to_account(token, &account));

        let matching = "header.eyJzdWIiOiJleHBlY3RlZCJ9.signature";
        assert!(auth::cursor::token_is_bound_to_account(matching, &account));
    }

    #[test]
    fn last_good_failure_keeps_limits_and_never_includes_secret() {
        let snapshot = CursorUsageSnapshot {
            limits: vec![UsageLimit {
                name: "Monthly".into(),
                usage_percent: 10.0,
                resets_at: None,
            }],
            quotas: vec![("monthly".into(), Some(900), None)],
            extra_info: Vec::new(),
        };
        let report = report_from_result(
            "Cursor - safe".into(),
            Some(snapshot),
            "Cursor usage unavailable (rate limited; quota is unknown)",
            false,
        );
        assert!(
            report
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("rate limited")
        );
        assert_eq!(report.limits.len(), 1);
        assert!(!format!("{report:?}").contains("refresh"));
    }
}
