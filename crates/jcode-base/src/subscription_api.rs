//! Typed client for the Jcode account and subscription API.
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
    #[serde(default)]
    pub budget_usd: f64,
    /// RFC 3339 timestamp for when the usage window resets.
    #[serde(default)]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubscriptionMe {
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
}

impl SubscriptionMe {
    pub fn parsed_tier(&self) -> Option<JcodeTier> {
        JcodeTier::parse(&self.tier)
    }

    pub fn has_active_paid_plan(&self) -> bool {
        self.status.eq_ignore_ascii_case("active") && self.parsed_tier().is_some()
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
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// (status, extra headers, body) for one canned HTTP response.
    type CannedResponse = (u16, Vec<(&'static str, &'static str)>, String);

    fn spawn_server(responses: Vec<CannedResponse>) -> String {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut request = [0u8; 8192];
                let _ = stream.read(&mut request);
                let extra = headers
                    .into_iter()
                    .map(|(name, value)| format!("{name}: {value}\r\n"))
                    .collect::<String>();
                let response = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        format!("http://{addr}/v1")
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client")
    }

    #[test]
    fn subscription_me_parses_expected_shape() {
        let json = r#"{
            "account_id": "acct_123", "email": "dev@example.com",
            "tier": "flagship", "status": "active",
            "usage": {"used_usd": 12.5, "budget_usd": 3000.0},
            "manage_url": "https://jcode.sh/account"
        }"#;
        let me: SubscriptionMe = serde_json::from_str(json).expect("parse");
        assert_eq!(me.parsed_tier(), Some(JcodeTier::Flagship));
        assert!(me.has_active_paid_plan());
        assert_eq!(me.manage_url.as_deref(), Some("https://jcode.sh/account"));
    }

    #[test]
    fn polling_backoff_is_deterministic_and_bounded() {
        let mut state = PollingBackoff::new(Duration::from_secs(3));
        assert_eq!(state.delay(), Duration::from_secs(3));
        state.on_slow_down(None);
        assert_eq!(state.delay(), Duration::from_secs(8));
        state.on_slow_down(Some(Duration::from_secs(12)));
        assert_eq!(state.delay(), Duration::from_secs(12));
        state.on_offline_error();
        assert_eq!(state.delay(), Duration::from_secs(24));
        state.on_offline_error();
        assert_eq!(state.delay(), Duration::from_secs(30));
        state.on_pending();
        assert_eq!(state.delay(), Duration::from_secs(3));
    }

    #[tokio::test]
    async fn device_request_uses_new_contract_and_parses_public_urls() {
        let base = spawn_server(vec![(
            200,
            vec![],
            r#"{"device_code":"secret","flow_id":"public-flow","verification_uri":"https://jcode.sh/account","verification_uri_complete":"https://jcode.sh/account?flow=public-flow","verify_url":"https://jcode.sh/account?flow=public-flow","expires_in":600,"interval":3}"#.to_string(),
        )]);
        let result = request_device_authorization(&client(), &base, Some(JcodeTier::Pro))
            .await
            .expect("device auth");
        assert_eq!(result.device_code, "secret");
        assert_eq!(result.flow_id, "public-flow");
        assert!(!result.verification_uri_complete.contains("secret"));
    }

    #[tokio::test]
    async fn legacy_device_response_is_explained_without_echoing_body() {
        let base = spawn_server(vec![(
            200,
            vec![],
            r#"{"device_code":"do-not-echo","verify_url":"https://old.example/login"}"#.to_string(),
        )]);
        let error = request_device_authorization(&client(), &base, Some(JcodeTier::Pro))
            .await
            .expect_err("legacy response rejected");
        assert_eq!(error, AccountApiError::LegacyBackend);
        assert!(!error.to_string().contains("do-not-echo"));
    }

    #[tokio::test]
    async fn token_poll_handles_pending_slow_down_success_denied_and_replay() {
        let base = spawn_server(vec![
            (428, vec![], r#"{"error":"authorization_pending"}"#.to_string()),
            (429, vec![("Retry-After", "9")], r#"{"error":"slow_down"}"#.to_string()),
            (200, vec![], r#"{"api_key":"jck_live_test","account_id":"acct","email":"user@example.com","tier":"none","status":"active"}"#.to_string()),
            (400, vec![], r#"{"error":"access_denied"}"#.to_string()),
            (400, vec![], r#"{"error":"expired_token"}"#.to_string()),
        ]);
        let client = client();
        assert_eq!(
            poll_device_token_once(&client, &base, "secret")
                .await
                .unwrap(),
            TokenPollOutcome::Pending
        );
        assert_eq!(
            poll_device_token_once(&client, &base, "secret")
                .await
                .unwrap(),
            TokenPollOutcome::SlowDown {
                retry_after: Some(Duration::from_secs(9))
            }
        );
        assert!(matches!(
            poll_device_token_once(&client, &base, "secret")
                .await
                .unwrap(),
            TokenPollOutcome::Approved(_)
        ));
        assert_eq!(
            poll_device_token_once(&client, &base, "secret")
                .await
                .unwrap(),
            TokenPollOutcome::Denied
        );
        assert_eq!(
            poll_device_token_once(&client, &base, "secret")
                .await
                .unwrap(),
            TokenPollOutcome::Expired
        );
    }

    #[tokio::test]
    async fn me_and_revoke_classify_revoked_keys_without_leaking_them() {
        let base = spawn_server(vec![
            (
                401,
                vec![],
                r#"{"error":"invalid_key","message":"jck_live_do-not-log"}"#.to_string(),
            ),
            (401, vec![], r#"{"error":"invalid_key"}"#.to_string()),
        ]);
        let client = client();
        let me_error = fetch_subscription_me_with(&client, &base, "jck_live_secret")
            .await
            .expect_err("revoked");
        assert_eq!(me_error, AccountApiError::Unauthorized);
        assert!(!me_error.to_string().contains("jck_live"));
        let revoke_error = revoke_current_key(&client, &base, "jck_live_secret")
            .await
            .expect_err("already revoked");
        assert_eq!(revoke_error, AccountApiError::Unauthorized);
    }
}
