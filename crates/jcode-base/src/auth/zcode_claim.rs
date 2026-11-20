//! ZCode manual plan claiming (weekend / trial plans).
//!
//! Protocol facts verified against the official ZCode client and the
//! independent reference implementation
//! https://github.com/TriDefender/zcode-api (protocol knowledge only — no
//! code copied; that project is CC BY-NC-SA 4.0).
//!
//! - `GET {origin}/api/v1/zcode-plan/billing/preview?app_version=&platform=`
//!   enumerates claimable plans (`Authorization: Bearer {jwt}`, plus a
//!   UUID `X-Device-Mid` — the gateway 3001s without it).
//! - `POST {origin}/api/v1/zcode-plan/billing/claim` body `{plan_id}`
//!   claims one. Biz codes mirror the client: 1001 notFound,
//!   1002 unavailable, 1003 alreadyClaimed, 1004 ineligible,
//!   1005 quotaExhausted, 3001 invalidRequest, 3007 captcha,
//!   401 loginRequired.
//! - A UUID `X-Device-Mid` is generated once per machine and persisted in
//!   the jcode state dir.

use crate::auth::zcode::{load_credentials, DEFAULT_ZCODE_APP_VERSION};
use crate::logging;
use anyhow::Context;
use serde::Deserialize;
use std::time::Duration;

pub const CLAIM_ORIGIN: &str = "https://zcode.z.ai";
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(15);
const CLAIM_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Deserialize)]
pub struct ClaimablePlan {
    #[serde(default, rename = "plan_id")]
    pub plan_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub entitlements: Vec<PlanEntitlement>,
    #[serde(default, rename = "starts_at")]
    pub starts_at: Option<u64>,
    #[serde(default, rename = "ends_at")]
    pub ends_at: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanEntitlement {
    #[serde(default, rename = "show_name")]
    pub show_name: String,
    #[serde(default, rename = "grant_units")]
    pub grant_units: f64,
    #[serde(default, rename = "unit_type")]
    pub unit_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimFailureKind {
    NotFound,
    Unavailable,
    AlreadyClaimed,
    Ineligible,
    QuotaExhausted,
    InvalidRequest,
    Captcha,
    LoginRequired,
    HttpError,
    Unknown,
}

impl std::fmt::Display for ClaimFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
            Self::AlreadyClaimed => "already_claimed",
            Self::Ineligible => "ineligible",
            Self::QuotaExhausted => "quota_exhausted",
            Self::InvalidRequest => "invalid_request",
            Self::Captcha => "captcha",
            Self::LoginRequired => "login_required",
            Self::HttpError => "http_error",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone)]
pub enum ClaimOutcome {
    Ok {
        plan_id: String,
        starts_at: Option<u64>,
        ends_at: Option<u64>,
    },
    Failed {
        plan_id: String,
        kind: ClaimFailureKind,
        code: i64,
        message: String,
    },
}

fn classify_claim_code(code: i64) -> ClaimFailureKind {
    match code {
        1001 => ClaimFailureKind::NotFound,
        1002 => ClaimFailureKind::Unavailable,
        1003 => ClaimFailureKind::AlreadyClaimed,
        1004 => ClaimFailureKind::Ineligible,
        1005 => ClaimFailureKind::QuotaExhausted,
        3001 => ClaimFailureKind::InvalidRequest,
        3007 => ClaimFailureKind::Captcha,
        401 => ClaimFailureKind::LoginRequired,
        _ => ClaimFailureKind::Unknown,
    }
}

/// Stable per-machine device id for the claim/marketing planes.
pub fn device_mid() -> String {
    let dir = match crate::storage::jcode_dir() {
        Ok(d) => d,
        Err(_) => return uuid::Uuid::new_v4().to_string(),
    };
    let path = dir.join("zcode_device_mid");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::write(&path, &id);
    id
}

fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn jwt() -> Option<String> {
    let creds = load_credentials().ok()?;
    let token = creds.token.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

pub async fn list_claimable_plans_default() -> anyhow::Result<Vec<ClaimablePlan>> {
    let client = reqwest::Client::new();
    list_claimable_plans(&client).await
}

pub async fn claim_plan_default(plan_id: &str) -> ClaimOutcome {
    let client = reqwest::Client::new();
    claim_plan(&client, plan_id).await
}

/// Enumerate claimable plans from the billing preview face.
pub async fn list_claimable_plans(client: &reqwest::Client) -> anyhow::Result<Vec<ClaimablePlan>> {
    let Some(jwt) = jwt() else {
        anyhow::bail!("ZCode manual claim requires login (jcode login --provider zcode)");
    };
    let url = format!(
        "{CLAIM_ORIGIN}/api/v1/zcode-plan/billing/preview?app_version={}&platform={}",
        DEFAULT_ZCODE_APP_VERSION,
        platform(),
    );
    let resp = client
        .get(&url)
        .timeout(PREVIEW_TIMEOUT)
        .header("Authorization", format!("Bearer {jwt}"))
        .header("X-Device-Mid", device_mid())
        .send()
        .await
        .context("ZCode billing preview request failed")?;
    let status = resp.status().as_u16();
    if status == 404 {
        // Campaign endpoint not deployed (pre-launch) — empty is correct.
        return Ok(Vec::new());
    }
    let json: serde_json::Value = resp.json().await.context("invalid JSON from billing preview")?;
    let code = json.get("code").and_then(|v| v.as_i64());
    if status < 200 || status >= 300 || (code.is_some() && code != Some(0)) {
        // code 0 with NO data field = no campaigns for this account.
        if code == Some(0) && json.get("data").is_none() {
            return Ok(Vec::new());
        }
        anyhow::bail!(
            "ZCode billing preview failed ({}): {}",
            code.map(|c| c.to_string()).unwrap_or_else(|| status.to_string()),
            json.get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
        );
    }
    let plans = json
        .pointer("/data/plans")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(plans
        .into_iter()
        .filter_map(|p| serde_json::from_value::<ClaimablePlan>(p).ok())
        .collect())
}

/// Claim one plan by id.
pub async fn claim_plan(client: &reqwest::Client, plan_id: &str) -> ClaimOutcome {
    let Some(jwt) = jwt() else {
        return ClaimOutcome::Failed {
            plan_id: plan_id.to_string(),
            kind: ClaimFailureKind::LoginRequired,
            code: 401,
            message: "manual_claim_login_required".to_string(),
        };
    };
    let url = format!("{CLAIM_ORIGIN}/api/v1/zcode-plan/billing/claim");
    let resp = client
        .post(&url)
        .timeout(CLAIM_TIMEOUT)
        .header("Authorization", format!("Bearer {jwt}"))
        .header("Content-Type", "application/json")
        .header("X-ZCode-App-Version", DEFAULT_ZCODE_APP_VERSION)
        .header("X-Platform", platform())
        .header("X-Device-Mid", device_mid())
        .json(&serde_json::json!({ "plan_id": plan_id }))
        .send()
        .await;
    let Ok(resp) = resp else {
        return ClaimOutcome::Failed {
            plan_id: plan_id.to_string(),
            kind: ClaimFailureKind::HttpError,
            code: -1,
            message: "claim request failed (network/timeout)".to_string(),
        };
    };
    let status = resp.status().as_u16();
    let Ok(json) = resp.json::<serde_json::Value>().await else {
        return ClaimOutcome::Failed {
            plan_id: plan_id.to_string(),
            kind: ClaimFailureKind::HttpError,
            code: status as i64,
            message: format!("HTTP {status}"),
        };
    };
    let biz_code = json.get("code").and_then(|v| v.as_i64());
    if (200..300).contains(&status) && biz_code == Some(0) {
        return ClaimOutcome::Ok {
            plan_id: plan_id.to_string(),
            starts_at: json
                .pointer("/data/plan/starts_at")
                .and_then(|v| v.as_u64()),
            ends_at: json.pointer("/data/plan/ends_at").and_then(|v| v.as_u64()),
        };
    }
    let code = biz_code.unwrap_or(status as i64);
    let message = json
        .get("msg")
        .or_else(|| json.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let kind = if biz_code.is_none() && status >= 400 {
        if status == 401 {
            ClaimFailureKind::LoginRequired
        } else {
            ClaimFailureKind::HttpError
        }
    } else {
        classify_claim_code(code)
    };
    logging::info(&format!(
        "ZCode claim {plan_id} failed ({kind}: {code} {message})"
    ));
    ClaimOutcome::Failed {
        plan_id: plan_id.to_string(),
        kind,
        code,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_code_classification() {
        assert_eq!(classify_claim_code(1003), ClaimFailureKind::AlreadyClaimed);
        assert_eq!(classify_claim_code(3007), ClaimFailureKind::Captcha);
        assert_eq!(classify_claim_code(401), ClaimFailureKind::LoginRequired);
        assert_eq!(classify_claim_code(9999), ClaimFailureKind::Unknown);
    }

    #[test]
    fn preview_parse_shape() {
        let json = serde_json::json!({
            "code": 0,
            "data": {
                "plans": [
                    {
                        "plan_id": "zcode-v3-start-plan-wk-1120",
                        "name": "ZCODE WEEKEND BUILD",
                        "entitlements": [
                            {"show_name": "Prompt", "grant_units": 120.0, "unit_type": "token"}
                        ]
                    }
                ]
            }
        });
        let plans = json
            .pointer("/data/plans")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let parsed: Vec<ClaimablePlan> = plans
            .into_iter()
            .filter_map(|p| serde_json::from_value(p).ok())
            .collect();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].plan_id, "zcode-v3-start-plan-wk-1120");
        assert_eq!(parsed[0].entitlements[0].grant_units, 120.0);
    }

    #[test]
    fn device_mid_is_stable_uuid() {
        let a = device_mid();
        let b = device_mid();
        assert_eq!(a, b);
        assert!(uuid::Uuid::parse_str(&a).is_ok());
    }
}
