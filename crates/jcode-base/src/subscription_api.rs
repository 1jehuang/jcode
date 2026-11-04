//! Typed client for the Jcode account and hosted-model billing API.
//!
//! All bearer credentials are sent in authorization headers or JSON response
//! bodies. They are never placed in URLs, redirects, or diagnostic messages.

use crate::subscription_catalog::{self, JcodeTier};
use anyhow::{Context, Result};
use reqwest::{StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// Timeout for short account API requests used by the CLI and TUI.
pub const ME_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubscriptionUsage {
    #[serde(default)]
    pub used_usd: f64,
    /// User-selected monthly spending limit. `budget_usd` remains accepted for
    /// compatibility with account servers deployed before metered billing.
    #[serde(default, alias = "spend_limit_usd", alias = "limit_usd")]
    pub budget_usd: f64,
    /// Amount already collected in billing tranches this month.
    #[serde(default)]
    pub billed_usd: f64,
    /// Next usage amount at which a tranche will be collected, when supplied.
    #[serde(default)]
    pub next_charge_at_usd: Option<f64>,
    /// RFC 3339 timestamp for when the usage window resets.
    #[serde(default)]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SubscriptionCapabilities {
    #[serde(default)]
    pub voice_transcription: bool,
    /// Server-verified entitlement for mobile transcript sync.
    #[serde(default)]
    pub mobile_sync: bool,
    /// Server-verified entitlement and configured isolated build service.
    #[serde(default)]
    pub remote_compile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubscriptionMe {
    /// Older servers advertise no optional capabilities.
    #[serde(default)]
    pub capabilities: SubscriptionCapabilities,
    pub account_id: String,
    pub email: String,
    /// Stable wire tier value: "none", "plus", "pro", "max", "ultra", or "flagship".
    pub tier: String,
    pub status: String,
    #[serde(default)]
    pub usage: SubscriptionUsage,
    /// Optional stable public account-management URL. It must never contain a secret.
    #[serde(default)]
    pub manage_url: Option<String>,
    /// Daily included-feature allowances (memory recall, browser automation).
    /// Absent on older gateways or when the account is not entitled.
    #[serde(default)]
    pub jev_usage: Option<JevUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct JevFeatureUsage {
    #[serde(default)]
    pub used: u64,
    #[serde(default)]
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct JevUsage {
    #[serde(default)]
    pub memory: JevFeatureUsage,
    #[serde(default)]
    pub browser: JevFeatureUsage,
    #[serde(default)]
    pub resets_at: Option<String>,
    #[serde(default)]
    pub upgrade_tier: Option<String>,
    #[serde(default)]
    pub upgrade_url: Option<String>,
}

impl SubscriptionMe {
    pub fn parsed_tier(&self) -> Option<JcodeTier> {
        JcodeTier::parse(&self.tier)
    }

    pub fn has_active_paid_plan(&self) -> bool {
        self.status.eq_ignore_ascii_case("active")
            && (self.usage.budget_usd > 0.0 || self.parsed_tier().is_some())
    }

    pub fn checkout_was_canceled(&self) -> bool {
        self.status.eq_ignore_ascii_case("canceled")
            || self.status.eq_ignore_ascii_case("cancelled")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthorization {
    /// Secret used only in the token exchange request body.
    pub device_code: String,
    /// Public correlation identifier.
    pub flow_id: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedAccountKey {
    pub api_key: String,
    pub account_id: String,
    pub email: String,
    pub tier: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenPollOutcome {
    Pending,
    SlowDown { retry_after: Option<Duration> },
    Approved(ApprovedAccountKey),
    Expired,
    Denied,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActivationOutcome {
    Active(SubscriptionMe),
    Canceled(SubscriptionMe),
    TimedOut { last_error_was_offline: bool },
    Revoked,
    Denied,
}

/// Redacted API error. Response bodies and bearer values are never retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountApiError {
    Offline(String),
    Unauthorized,
    Forbidden,
    LegacyBackend,
    Http { status: u16, code: Option<String> },
    InvalidResponse(&'static str),
}

impl AccountApiError {
    pub fn is_temporary(&self) -> bool {
        matches!(self, Self::Offline(_))
            || matches!(self, Self::Http { status, .. } if *status >= 500)
    }
}

impl fmt::Display for AccountApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Offline(reason) => write!(f, "temporarily offline: {reason}"),
            Self::Unauthorized => write!(f, "the Jcode account key is revoked or expired"),
            Self::Forbidden => write!(f, "the Jcode account request was denied"),
            Self::LegacyBackend => write!(
                f,
                "the configured Jcode API uses the legacy email-based login contract; update the backend or use the current https://api.jcode.sh/v1 endpoint"
            ),
            Self::Http { status, code } => match code {
                Some(code) => write!(f, "Jcode account API returned HTTP {status} ({code})"),
                None => write!(f, "Jcode account API returned HTTP {status}"),
            },
            Self::InvalidResponse(detail) => {
                write!(
                    f,
                    "Jcode account API returned an invalid response: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for AccountApiError {}

#[derive(Debug, Deserialize, Default)]
struct ErrorEnvelope {
    #[serde(default)]
    error: Option<ErrorField>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ErrorField {
    Code(String),
    Object { code: Option<String> },
}

impl ErrorEnvelope {
    fn code(&self) -> Option<String> {
        match &self.error {
            Some(ErrorField::Code(code)) => Some(code.clone()),
            Some(ErrorField::Object { code }) => code.clone(),
            None => self.status.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorizationWire {
    #[serde(default)]
    device_code: Option<String>,
    #[serde(default)]
    flow_id: Option<String>,
    #[serde(default)]
    verification_uri: Option<String>,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
    /// Legacy field. Its presence gives a specific compatibility error.
    #[serde(default)]
    verify_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApprovedAccountKeyWire {
    api_key: String,
    account_id: String,
    email: String,
    tier: String,
    status: String,
}

pub fn configured_api_base() -> String {
    subscription_catalog::configured_api_base()
        .unwrap_or_else(|| subscription_catalog::DEFAULT_JCODE_API_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn endpoint_url(api_base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        api_base.trim().trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// The `/v1/me` endpoint URL for the configured (or default) API base.
pub fn me_endpoint_url() -> String {
    endpoint_url(&configured_api_base(), "me")
}

fn offline(error: reqwest::Error) -> AccountApiError {
    // reqwest's display contains only the public endpoint URL here. Request and
    // response bodies, including device codes and API keys, are not included.
    AccountApiError::Offline(error.to_string())
}

fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .and_then(|error| error.code())
        .map(|code| code.chars().take(80).collect())
}

pub async fn request_device_authorization(
    client: &reqwest::Client,
    api_base: &str,
    requested_tier: Option<JcodeTier>,
) -> std::result::Result<DeviceAuthorization, AccountApiError> {
    let url = endpoint_url(api_base, "auth/device");
    let mut payload = serde_json::json!({ "client_name": "jcode-cli" });
    if let Some(tier) = requested_tier {
        payload["requested_tier"] = serde_json::Value::String(tier.as_str().to_string());
    }
    let response = client
        .post(url)
        .json(&payload)
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let status = response.status();
    let body = response.text().await.map_err(offline)?;
    if !status.is_success() {
        return Err(match status {
            StatusCode::UNAUTHORIZED => AccountApiError::Unauthorized,
            StatusCode::FORBIDDEN => AccountApiError::Forbidden,
            StatusCode::NOT_FOUND => AccountApiError::LegacyBackend,
            _ => AccountApiError::Http {
                status: status.as_u16(),
                code: error_code(&body),
            },
        });
    }

    let wire: DeviceAuthorizationWire = serde_json::from_str(&body)
        .map_err(|_| AccountApiError::InvalidResponse("malformed device authorization JSON"))?;
    if wire.verify_url.is_some() && wire.verification_uri_complete.is_none() {
        return Err(AccountApiError::LegacyBackend);
    }
    let required = |value: Option<String>, detail| {
        value
            .filter(|value| !value.trim().is_empty())
            .ok_or(AccountApiError::InvalidResponse(detail))
    };
    Ok(DeviceAuthorization {
        device_code: required(wire.device_code, "missing device_code")?,
        flow_id: required(wire.flow_id, "missing flow_id")?,
        verification_uri: required(wire.verification_uri, "missing verification_uri")?,
        verification_uri_complete: required(
            wire.verification_uri_complete,
            "missing verification_uri_complete",
        )?,
        expires_in: wire.expires_in.unwrap_or(600).clamp(1, 3600),
        interval: wire.interval.unwrap_or(3).clamp(1, 60),
    })
}

pub async fn poll_device_token_once(
    client: &reqwest::Client,
    api_base: &str,
    device_code: &str,
) -> std::result::Result<TokenPollOutcome, AccountApiError> {
    let url = endpoint_url(api_base, "auth/token");
    let response = client
        .post(url)
        .json(&serde_json::json!({ "device_code": device_code }))
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let status = response.status();
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response.text().await.map_err(offline)?;

    if status == StatusCode::TOO_MANY_REQUESTS {
        return Ok(TokenPollOutcome::SlowDown { retry_after });
    }
    if status.as_u16() == 428 || status == StatusCode::ACCEPTED {
        return Ok(TokenPollOutcome::Pending);
    }
    if status.is_success() {
        if matches!(
            error_code(&body).as_deref(),
            Some("authorization_pending" | "pending")
        ) {
            return Ok(TokenPollOutcome::Pending);
        }
        let approved: ApprovedAccountKeyWire = serde_json::from_str(&body)
            .map_err(|_| AccountApiError::InvalidResponse("malformed approved token JSON"))?;
        if approved.api_key.trim().is_empty() {
            return Err(AccountApiError::InvalidResponse("empty api_key"));
        }
        return Ok(TokenPollOutcome::Approved(ApprovedAccountKey {
            api_key: approved.api_key,
            account_id: approved.account_id,
            email: approved.email,
            tier: approved.tier,
            status: approved.status,
        }));
    }

    let code = error_code(&body);
    match code.as_deref() {
        Some("authorization_pending" | "pending") => Ok(TokenPollOutcome::Pending),
        Some("slow_down") => Ok(TokenPollOutcome::SlowDown { retry_after }),
        Some("expired_token" | "expired" | "expired_device_code") => Ok(TokenPollOutcome::Expired),
        Some("access_denied" | "denied") => Ok(TokenPollOutcome::Denied),
        _ if status == StatusCode::UNAUTHORIZED => Err(AccountApiError::Unauthorized),
        _ if status == StatusCode::FORBIDDEN => Err(AccountApiError::Forbidden),
        _ if status == StatusCode::NOT_FOUND => Err(AccountApiError::LegacyBackend),
        _ => Err(AccountApiError::Http {
            status: status.as_u16(),
            code,
        }),
    }
}

/// Outcome of starting a native email-code sign-in.
#[derive(Clone)]
pub struct EmailCodeStart {
    pub login_token: String,
    pub expires_in: u64,
    pub code_length: usize,
}

#[derive(Deserialize)]
struct EmailCodeStartWire {
    login_token: Option<String>,
    expires_in: Option<u64>,
    code_length: Option<usize>,
}

/// Outcome of verifying an emailed code.
#[derive(Debug, Clone)]
pub enum EmailCodeVerifyOutcome {
    Approved(ApprovedAccountKey),
    /// Wrong code. The same login may be retried.
    Incorrect {
        attempts_remaining: Option<u32>,
    },
    /// Expired, used, or locked. Start again.
    Expired,
}

/// Email the address a six-digit code. Nothing opens in a browser.
pub async fn request_email_code(
    client: &reqwest::Client,
    api_base: &str,
    email: &str,
) -> std::result::Result<EmailCodeStart, AccountApiError> {
    let response = client
        .post(endpoint_url(api_base, "auth/email/start"))
        .json(&serde_json::json!({ "client_name": "jcode-cli", "email": email.trim() }))
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let status = response.status();
    let body = response.text().await.map_err(offline)?;
    if !status.is_success() {
        return Err(match status {
            StatusCode::NOT_FOUND => AccountApiError::LegacyBackend,
            _ => AccountApiError::Http {
                status: status.as_u16(),
                code: error_code(&body),
            },
        });
    }
    let wire: EmailCodeStartWire = serde_json::from_str(&body)
        .map_err(|_| AccountApiError::InvalidResponse("malformed email code JSON"))?;
    let login_token = wire
        .login_token
        .filter(|token| !token.trim().is_empty())
        .ok_or(AccountApiError::InvalidResponse("missing login_token"))?;
    Ok(EmailCodeStart {
        login_token,
        expires_in: wire.expires_in.unwrap_or(900).clamp(1, 3600),
        code_length: wire.code_length.unwrap_or(6).clamp(4, 12),
    })
}

pub async fn verify_email_code(
    client: &reqwest::Client,
    api_base: &str,
    login_token: &str,
    code: &str,
) -> std::result::Result<EmailCodeVerifyOutcome, AccountApiError> {
    let response = client
        .post(endpoint_url(api_base, "auth/email/verify"))
        .json(&serde_json::json!({ "login_token": login_token, "code": code.trim() }))
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let status = response.status();
    let body = response.text().await.map_err(offline)?;
    if status.is_success() {
        let approved: ApprovedAccountKeyWire = serde_json::from_str(&body)
            .map_err(|_| AccountApiError::InvalidResponse("malformed approved token JSON"))?;
        if approved.api_key.trim().is_empty() {
            return Err(AccountApiError::InvalidResponse("empty api_key"));
        }
        return Ok(EmailCodeVerifyOutcome::Approved(ApprovedAccountKey {
            api_key: approved.api_key,
            account_id: approved.account_id,
            email: approved.email,
            tier: approved.tier,
            status: approved.status,
        }));
    }
    match error_code(&body).as_deref() {
        Some("invalid_code") => Ok(EmailCodeVerifyOutcome::Incorrect {
            attempts_remaining: serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| value["error"]["attempts_remaining"].as_u64())
                .map(|n| n.min(u32::MAX as u64) as u32),
        }),
        Some("expired_code") => Ok(EmailCodeVerifyOutcome::Expired),
        code => Err(match status {
            StatusCode::NOT_FOUND => AccountApiError::LegacyBackend,
            _ => AccountApiError::Http {
                status: status.as_u16(),
                code: code.map(str::to_owned),
            },
        }),
    }
}

pub async fn fetch_subscription_me_with(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
) -> std::result::Result<SubscriptionMe, AccountApiError> {
    let response = client
        .get(endpoint_url(api_base, "me"))
        .bearer_auth(api_key)
        .timeout(ME_FETCH_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let status = response.status();
    let body = response.text().await.map_err(offline)?;
    if !status.is_success() {
        return Err(match status {
            StatusCode::UNAUTHORIZED => AccountApiError::Unauthorized,
            StatusCode::FORBIDDEN => AccountApiError::Forbidden,
            _ => AccountApiError::Http {
                status: status.as_u16(),
                code: error_code(&body),
            },
        });
    }
    let me: SubscriptionMe = serde_json::from_str(&body)
        .map_err(|_| AccountApiError::InvalidResponse("malformed /v1/me JSON"))?;
    if let Some(tier) = me.parsed_tier() {
        let _ = subscription_catalog::store_cached_tier(Some(tier));
    }
    Ok(me)
}

/// Fetch account status using the configured local credential.
pub async fn fetch_subscription_me() -> Result<SubscriptionMe> {
    let api_key = subscription_catalog::configured_api_key()
        .context("no Jcode account credential configured (run `jcode account login`)")?;
    fetch_subscription_me_with(
        &crate::provider::shared_http_client(),
        &configured_api_base(),
        &api_key,
    )
    .await
    .map_err(anyhow::Error::new)
}

pub async fn revoke_current_key(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
) -> std::result::Result<(), AccountApiError> {
    let response = client
        .delete(endpoint_url(api_base, "keys/current"))
        .bearer_auth(api_key)
        .timeout(ME_FETCH_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.map_err(offline)?;
    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND => AccountApiError::Unauthorized,
        StatusCode::FORBIDDEN => AccountApiError::Forbidden,
        _ => AccountApiError::Http {
            status: status.as_u16(),
            code: error_code(&body),
        },
    })
}

/// Usage-billing state for the current period, from `GET /v1/billing`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BillingStatus {
    #[serde(default)]
    pub monthly_hard_cap_cents: u64,
    /// Monthly plan in dollars, present only for active subscribers.
    #[serde(default)]
    pub plan_usd: Option<u64>,
    #[serde(default)]
    pub activation: BillingActivation,
    #[serde(default)]
    pub promotional_credit: BillingCredit,
    #[serde(default)]
    pub period: BillingPeriod,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BillingActivation {
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BillingCredit {
    #[serde(default)]
    pub original_microusd: u64,
    #[serde(default)]
    pub remaining_microusd: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BillingPeriod {
    #[serde(default)]
    pub resets_at: Option<String>,
    #[serde(default)]
    pub billable_microusd: u64,
    #[serde(default)]
    pub remaining_cap_microusd: u64,
}

impl BillingStatus {
    pub fn monthly_hard_cap_usd(&self) -> f64 {
        self.monthly_hard_cap_cents as f64 / 100.0
    }

    pub fn billable_usd(&self) -> f64 {
        self.period.billable_microusd as f64 / 1_000_000.0
    }

    pub fn promotional_credit_usd(&self) -> f64 {
        self.promotional_credit.remaining_microusd as f64 / 1_000_000.0
    }
}

#[derive(Deserialize)]
struct BillingEnvelope {
    billing: BillingStatus,
}

#[derive(Deserialize)]
struct PortalWire {
    url: String,
}

async fn account_response_body(
    response: reqwest::Response,
) -> std::result::Result<String, AccountApiError> {
    let status = response.status();
    let body = response.text().await.map_err(offline)?;
    if status.is_success() {
        return Ok(body);
    }
    Err(match status {
        StatusCode::UNAUTHORIZED => AccountApiError::Unauthorized,
        StatusCode::FORBIDDEN => AccountApiError::Forbidden,
        _ => AccountApiError::Http {
            status: status.as_u16(),
            code: error_code(&body),
        },
    })
}

/// Fetch usage billing (monthly hard cap, period spend, credit).
pub async fn fetch_billing_with(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
) -> std::result::Result<BillingStatus, AccountApiError> {
    let response = client
        .get(endpoint_url(api_base, "billing"))
        .bearer_auth(api_key)
        .timeout(ME_FETCH_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let body = account_response_body(response).await?;
    serde_json::from_str::<BillingEnvelope>(&body)
        .map(|envelope| envelope.billing)
        .map_err(|_| AccountApiError::InvalidResponse("malformed /v1/billing JSON"))
}

/// Set the monthly spending hard cap, in whole cents. Returns the new state.
pub async fn set_monthly_hard_cap_with(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    cents: u64,
) -> std::result::Result<BillingStatus, AccountApiError> {
    let response = client
        .put(endpoint_url(api_base, "billing/hard-cap"))
        .bearer_auth(api_key)
        .json(&serde_json::json!({ "monthly_hard_cap_cents": cents }))
        .timeout(ME_FETCH_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let body = account_response_body(response).await?;
    serde_json::from_str::<BillingEnvelope>(&body)
        .map(|envelope| envelope.billing)
        .map_err(|_| AccountApiError::InvalidResponse("malformed /v1/billing/hard-cap JSON"))
}

/// Create a short-lived Stripe billing portal session (payment method,
/// invoices, cancellation). Only `https://` URLs are returned.
pub async fn billing_portal_url_with(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
) -> std::result::Result<String, AccountApiError> {
    let response = client
        .post(endpoint_url(api_base, "billing/portal"))
        .bearer_auth(api_key)
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let body = account_response_body(response).await?;
    let url = serde_json::from_str::<PortalWire>(&body)
        .map_err(|_| AccountApiError::InvalidResponse("malformed billing portal JSON"))?
        .url;
    if !url.starts_with("https://") {
        return Err(AccountApiError::InvalidResponse(
            "billing portal URL was not https",
        ));
    }
    Ok(url)
}

/// Open Stripe Checkout for a new subscription at `plan_usd` dollars a month
/// (a multiple of $10). Stripe Checkout offers Link and saved cards, so most
/// people never retype card details. Only `checkout.stripe.com` URLs are
/// returned.
pub async fn start_subscription_checkout_with(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    plan_usd: u64,
) -> std::result::Result<String, AccountApiError> {
    let response = client
        .post(endpoint_url(api_base, "billing/subscribe"))
        .bearer_auth(api_key)
        .json(&serde_json::json!({ "plan_usd": plan_usd }))
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let body = account_response_body(response).await?;
    let url = serde_json::from_str::<PortalWire>(&body)
        .map_err(|_| AccountApiError::InvalidResponse("malformed checkout JSON"))?
        .url;
    if !url.starts_with("https://checkout.stripe.com/") {
        return Err(AccountApiError::InvalidResponse(
            "checkout URL was not Stripe Checkout",
        ));
    }
    Ok(url)
}

/// Result of an in-place plan switch.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PlanChange {
    pub plan_usd: u64,
    pub monthly_limit_usd: f64,
    /// When the new price applies, e.g. "next_invoice".
    #[serde(default)]
    pub effective: String,
}

/// Switch an active subscription to `plan_usd` dollars a month. Uses the
/// payment method already on the subscription, so nothing is entered.
pub async fn change_plan_with(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    plan_usd: u64,
) -> std::result::Result<PlanChange, AccountApiError> {
    let response = client
        .post(endpoint_url(api_base, "billing/plan"))
        .bearer_auth(api_key)
        .json(&serde_json::json!({ "plan_usd": plan_usd }))
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(offline)?;
    let body = account_response_body(response).await?;
    serde_json::from_str(&body)
        .map_err(|_| AccountApiError::InvalidResponse("malformed plan change JSON"))
}

/// How a sign-out finished. Local credentials are always cleared first-class;
/// remote revocation is best effort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignOutOutcome {
    Revoked,
    AlreadyRevoked,
    NoCredential,
    /// Local credentials were cleared but the key could not be revoked.
    LocalOnly(String),
}

/// Revoke the current key (best effort) and clear local credentials.
pub async fn sign_out_current_account(client: &reqwest::Client) -> Result<SignOutOutcome> {
    let api_key = subscription_catalog::configured_api_key();
    let remote = match api_key.as_deref() {
        Some(key) => Some(revoke_current_key(client, &configured_api_base(), key).await),
        None => None,
    };
    subscription_catalog::clear_account_credentials()?;
    let _ = subscription_catalog::store_cached_tier(None);
    Ok(match remote {
        None => SignOutOutcome::NoCredential,
        Some(Ok(())) => SignOutOutcome::Revoked,
        Some(Err(AccountApiError::Unauthorized)) => SignOutOutcome::AlreadyRevoked,
        Some(Err(error)) => SignOutOutcome::LocalOnly(error.to_string()),
    })
}

/// Poll `/v1/me` after a successful token exchange until a paid plan becomes
/// active or a clear terminal/recovery state is reached.
pub async fn poll_for_paid_activation(
    client: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    timeout: Duration,
    interval: Duration,
) -> ActivationOutcome {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut backoff = PollingBackoff::new(interval.max(Duration::from_secs(1)));
    let mut last_error_was_offline;

    loop {
        match fetch_subscription_me_with(client, api_base, api_key).await {
            Ok(me) if me.has_active_paid_plan() => return ActivationOutcome::Active(me),
            Ok(me) if me.checkout_was_canceled() => return ActivationOutcome::Canceled(me),
            Ok(_) => {
                last_error_was_offline = false;
                backoff.on_successful_poll();
            }
            Err(AccountApiError::Unauthorized) => return ActivationOutcome::Revoked,
            Err(AccountApiError::Forbidden) => return ActivationOutcome::Denied,
            Err(error) if error.is_temporary() => {
                last_error_was_offline = true;
                backoff.on_offline_error();
            }
            Err(_) => {
                last_error_was_offline = false;
                backoff.on_server_error();
            }
        }

        let delay = backoff.delay();
        if tokio::time::Instant::now() + delay >= deadline {
            return ActivationOutcome::TimedOut {
                last_error_was_offline,
            };
        }
        tokio::time::sleep(delay).await;
    }
}

/// Deterministic retry policy shared by device-token and activation polling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollingBackoff {
    base: Duration,
    delay: Duration,
}

impl PollingBackoff {
    pub fn new(base: Duration) -> Self {
        let base = base.max(Duration::from_secs(1));
        Self { base, delay: base }
    }

    pub fn delay(&self) -> Duration {
        self.delay
    }

    pub fn on_pending(&mut self) {
        self.delay = self.base;
    }

    pub fn on_slow_down(&mut self, retry_after: Option<Duration>) {
        self.delay = retry_after
            .unwrap_or(self.delay + Duration::from_secs(5))
            .max(self.base)
            .min(Duration::from_secs(60));
    }

    pub fn on_offline_error(&mut self) {
        self.delay = (self.delay * 2).min(Duration::from_secs(30));
    }

    pub fn on_server_error(&mut self) {
        self.on_offline_error();
    }

    pub fn on_successful_poll(&mut self) {
        self.delay = self.base;
    }
}

#[cfg(test)]
#[path = "subscription_api_tests.rs"]
mod tests;
