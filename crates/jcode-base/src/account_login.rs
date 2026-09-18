//! Account-only browser login for Desktop and other async clients.
//!
//! Start opens no browser and sends no email. The caller opens `auth_url()` and
//! schedules one `poll()` at a time, respecting `interval()` and `SlowDown`.
//! Cancel by dropping the flow/future. Polling never persists credentials. Only
//! call `save()` after confirming that the approval belongs to the UI's current
//! flow. Canceling an in-flight exchange can consume the server's single-use
//! device code, so restarting requires a new flow.
//!
//! This module never chooses a provider, activates billing, or waits for a paid
//! plan. Browser sign-in and optional subscription checkout are separate actions.

use crate::subscription_api::{self, AccountApiError, SubscriptionMe, TokenPollOutcome};
use crate::subscription_catalog;
use std::fmt;
use std::time::{Duration, Instant};

/// A redacted error safe for UI display and diagnostic logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountLoginError {
    Offline,
    Unauthorized,
    Denied,
    UnsupportedBackend,
    Http { status: u16 },
    InvalidResponse,
    Storage,
}

impl AccountLoginError {
    pub fn is_temporary(&self) -> bool {
        matches!(
            self,
            Self::Offline
                | Self::Http {
                    status: 429 | 500..=599
                }
        )
    }
}

impl fmt::Display for AccountLoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Offline => f.write_str("Unable to reach the Jcode account service. Try again."),
            Self::Unauthorized => {
                f.write_str("The Jcode account credential has expired or was revoked.")
            }
            Self::Denied => f.write_str("Jcode account sign-in was denied."),
            Self::UnsupportedBackend => {
                f.write_str("This Jcode account service does not support browser device sign-in.")
            }
            Self::Http { status } => write!(f, "Jcode account service returned HTTP {status}."),
            Self::InvalidResponse => {
                f.write_str("The Jcode account service returned an invalid response.")
            }
            Self::Storage => f.write_str("Could not securely save the Jcode account credential."),
        }
    }
}

impl std::error::Error for AccountLoginError {}

impl From<AccountApiError> for AccountLoginError {
    fn from(error: AccountApiError) -> Self {
        // Neither arbitrary backend error codes nor reqwest URLs are safe to log.
        match error {
            AccountApiError::Offline(_) => Self::Offline,
            AccountApiError::Unauthorized => Self::Unauthorized,
            AccountApiError::Forbidden => Self::Denied,
            AccountApiError::LegacyBackend => Self::UnsupportedBackend,
            AccountApiError::Http { status, .. } => Self::Http { status },
            AccountApiError::InvalidResponse(_) => Self::InvalidResponse,
        }
    }
}

/// Opaque device authorization. Its secret and endpoint never appear in Debug.
#[derive(Clone)]
pub struct LoginFlow {
    api_base: String,
    device_code: String,
    auth_url: String,
    interval: Duration,
    expires_in: Duration,
    started_at: Instant,
}

impl LoginFlow {
    pub fn auth_url(&self) -> &str {
        &self.auth_url
    }
    pub fn interval(&self) -> Duration {
        self.interval
    }
    pub fn expires_in(&self) -> Duration {
        self.expires_in
    }
    pub fn is_expired(&self) -> bool {
        self.started_at.elapsed() >= self.expires_in
    }
}

impl fmt::Debug for LoginFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginFlow")
            .field("interval", &self.interval)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// Approved account identity, with an opaque credential. Approval alone is not
/// evidence of a paid plan, and a free/inactive account can be saved normally.
#[derive(Clone)]
pub struct ApprovedLogin {
    api_key: String,
    pub account_id: String,
    pub email: String,
    pub tier: String,
    pub status: String,
}

impl fmt::Debug for ApprovedLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApprovedLogin").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum LoginPoll {
    Pending,
    SlowDown { retry_after: Duration },
    Approved(ApprovedLogin),
    Expired,
    Denied,
}

/// Start a device flow against the configured account API, without selecting a
/// tier or launching a browser. No email is sent until the user signs in there.
pub async fn start(client: &reqwest::Client) -> Result<LoginFlow, AccountLoginError> {
    start_with(client, &subscription_api::configured_api_base()).await
}

async fn start_with(
    client: &reqwest::Client,
    api_base: &str,
) -> Result<LoginFlow, AccountLoginError> {
    let started_at = Instant::now();
    let device = subscription_api::request_device_authorization_for_client(
        client,
        api_base,
        None,
        "jcode-desktop",
    )
    .await?;
    let auth_url = public_auth_url(&device.verification_uri_complete)?;
    Ok(LoginFlow {
        api_base: api_base.to_owned(),
        device_code: device.device_code,
        auth_url,
        interval: Duration::from_secs(device.interval),
        expires_in: Duration::from_secs(device.expires_in),
        started_at,
    })
}

fn public_auth_url(value: &str) -> Result<String, AccountLoginError> {
    let url = reqwest::Url::parse(value).map_err(|_| AccountLoginError::InvalidResponse)?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("jcode.sh" | "www.jcode.sh" | "solosystems.dev")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.path() != "/account"
        || url.fragment().is_some()
    {
        return Err(AccountLoginError::InvalidResponse);
    }
    // Only the deployed public correlation value belongs in a browser URL.
    let params: Vec<_> = url.query_pairs().collect();
    if params.len() != 1
        || params[0].0 != "flow"
        || !(6..=128).contains(&params[0].1.len())
        || !params[0]
            .1
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(AccountLoginError::InvalidResponse);
    }
    Ok(url.to_string())
}

/// Perform one bounded asynchronous poll, with no sleeping, persistence, or
/// activation checks. Do not run concurrent polls for the same flow.
pub async fn poll(
    client: &reqwest::Client,
    flow: &LoginFlow,
) -> Result<LoginPoll, AccountLoginError> {
    if flow.is_expired() {
        return Ok(LoginPoll::Expired);
    }
    Ok(
        match subscription_api::poll_device_token_once(client, &flow.api_base, &flow.device_code)
            .await?
        {
            TokenPollOutcome::Pending => LoginPoll::Pending,
            TokenPollOutcome::SlowDown { retry_after } => LoginPoll::SlowDown {
                retry_after: retry_after
                    .unwrap_or(flow.interval.saturating_add(Duration::from_secs(5)))
                    .max(flow.interval),
            },
            TokenPollOutcome::Expired => LoginPoll::Expired,
            TokenPollOutcome::Denied => LoginPoll::Denied,
            TokenPollOutcome::Approved(key) => LoginPoll::Approved(ApprovedLogin {
                api_key: key.api_key,
                account_id: key.account_id,
                email: key.email,
                tier: key.tier,
                status: key.status,
            }),
        },
    )
}

/// Save to the existing owner-only Jcode credential store. This performs local
/// filesystem I/O, so GUI callers should use their background executor. Does not
/// select a provider, modify runtime routing, or change any billing settings.
pub fn save(approved: &ApprovedLogin) -> Result<(), AccountLoginError> {
    subscription_catalog::persist_account_credentials(
        &approved.api_key,
        Some(&approved.account_id),
        Some(&approved.email),
        Some(&approved.tier),
    )
    .map_err(|_| AccountLoginError::Storage)?;
    crate::auth::AuthStatus::invalidate_cache();
    Ok(())
}

pub fn has_credentials() -> bool {
    subscription_catalog::has_credentials()
}

/// Fetch the current account, including accounts without a subscription. None
/// means no local credential. Errors never clear credentials or switch providers.
pub async fn current_account(
    client: &reqwest::Client,
) -> Result<Option<SubscriptionMe>, AccountLoginError> {
    let Some(api_key) = subscription_catalog::configured_api_key() else {
        return Ok(None);
    };
    subscription_api::fetch_subscription_me_with(
        client,
        &subscription_api::configured_api_base(),
        &api_key,
    )
    .await
    .map(Some)
    .map_err(Into::into)
}

#[cfg(test)]
mod tests;
