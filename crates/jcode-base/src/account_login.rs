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
    start_with_api_base(client, &subscription_api::configured_api_base()).await
}

/// Start against an explicitly selected, trusted account API instead of the
/// process-wide configuration. The flow retains this endpoint for subsequent
/// polls. Browser URLs still require the same public account URL validation.
/// This also lets clients exercise their network lifecycle with an isolated API
/// without changing environment variables shared by other login operations.
pub async fn start_with_api_base(
    client: &reqwest::Client,
    api_base: &str,
) -> Result<LoginFlow, AccountLoginError> {
    let started_at = Instant::now();
    // The deployed API currently requires the protocol client name `jcode-cli`,
    // even for Desktop. It rejects `jcode-desktop` with HTTP 400. Reuse the
    // supported shared contract rather than inventing an unrecognized name.
    let device = subscription_api::request_device_authorization(client, api_base, None).await?;
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

/// A native email-code sign-in. The token is an in-memory secret that binds
/// the emailed code to this client, so it never appears in Debug output.
#[derive(Clone)]
pub struct EmailLogin {
    api_base: String,
    email: String,
    token: String,
    code_length: usize,
    expires_in: Duration,
    started_at: Instant,
}

impl EmailLogin {
    pub fn email(&self) -> &str {
        &self.email
    }
    pub fn code_length(&self) -> usize {
        self.code_length
    }
    pub fn expires_in(&self) -> Duration {
        self.expires_in
    }
    pub fn is_expired(&self) -> bool {
        self.started_at.elapsed() >= self.expires_in
    }
}

impl fmt::Debug for EmailLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmailLogin")
            .field("code_length", &self.code_length)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum EmailCodeResult {
    Approved(ApprovedLogin),
    Incorrect { attempts_remaining: Option<u32> },
    Expired,
}

/// Address the account service sends sign-in codes from.
pub const LOGIN_EMAIL_SENDER: &str = "login@solosystems.dev";

/// Gmail link for `email` that searches for our sign-in email, including
/// Spam (`in:anywhere`). `authuser` picks the matching signed-in account.
pub fn gmail_search_link(email: &str) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    format!(
        "https://mail.google.com/mail/?authuser={}#search/{}",
        enc(&email.trim().to_lowercase()),
        enc(&format!(
            "from:{LOGIN_EMAIL_SENDER} in:anywhere newer_than:1d"
        )),
    )
}

/// Why an email sign-in could not start, in words safe for the UI.
pub fn email_start_error_message(error: &AccountLoginError) -> String {
    match error {
        AccountLoginError::Http { status: 400 } => "Enter a valid email address.".into(),
        AccountLoginError::Http { status: 429 } => {
            "Too many sign-in emails. Wait a few minutes and try again.".into()
        }
        AccountLoginError::Http { status: 502 | 503 } => {
            "We could not send the email right now. Try again shortly.".into()
        }
        other => other.to_string(),
    }
}

/// Email a sign-in code to `email`. Opens no browser.
pub async fn start_email(
    client: &reqwest::Client,
    email: &str,
) -> Result<EmailLogin, AccountLoginError> {
    start_email_with_api_base(client, &subscription_api::configured_api_base(), email).await
}

pub async fn start_email_with_api_base(
    client: &reqwest::Client,
    api_base: &str,
    email: &str,
) -> Result<EmailLogin, AccountLoginError> {
    let started_at = Instant::now();
    let start = subscription_api::request_email_code(client, api_base, email).await?;
    Ok(EmailLogin {
        api_base: api_base.to_owned(),
        email: email.trim().to_lowercase(),
        token: start.login_token,
        code_length: start.code_length,
        expires_in: Duration::from_secs(start.expires_in),
        started_at,
    })
}

/// Check a typed code. Does not persist anything; call `save()` on approval.
pub async fn verify_email(
    client: &reqwest::Client,
    login: &EmailLogin,
    code: &str,
) -> Result<EmailCodeResult, AccountLoginError> {
    if login.is_expired() {
        return Ok(EmailCodeResult::Expired);
    }
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    Ok(
        match subscription_api::verify_email_code(client, &login.api_base, &login.token, &digits)
            .await?
        {
            subscription_api::EmailCodeVerifyOutcome::Approved(key) => {
                EmailCodeResult::Approved(ApprovedLogin {
                    api_key: key.api_key,
                    account_id: key.account_id,
                    email: key.email,
                    tier: key.tier,
                    status: key.status,
                })
            }
            subscription_api::EmailCodeVerifyOutcome::Incorrect { attempts_remaining } => {
                EmailCodeResult::Incorrect { attempts_remaining }
            }
            subscription_api::EmailCodeVerifyOutcome::Expired => EmailCodeResult::Expired,
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
