//! Native Muse subscription login (device flow + observed mint).
//!
//! This module implements the RFC 8628 device authorization flow against the
//! same public first-party endpoints the official `muse` launcher script
//! uses, then exchanges the resulting device OAuth access token for a
//! subscription API key using the *observed* community-client mint contract
//! described below.
//!
//! It is a third-party client and is not Meta-supported; the mint step below
//! is NOT verified against a first-party binary.
//!
//! ## Verified sources
//!
//! - Official launcher `/root/.local/bin/muse` (bash): `auth_url`,
//!   `client_id`, `authorization_endpoint`, `token_endpoint`,
//!   `device_code_grant`, redirect refusal, timeouts, and poll-loop error
//!   handling are mirrored 1:1 from that script. The launcher itself contains
//!   **no** mint endpoint string (the compiled CLI likely constructs it), so
//!   the mint step below is NOT verified against a first-party binary.
//! - Observed mint contract (community client mirroring the launcher):
//!   `POST https://api.meta.ai/muse-code/key` with `Authorization: Bearer
//!   <device access token>`, `Content-Type: application/json`, body `{}`.
//!   Response carries `api_key`, `base_url`, and subscription/billing flags.
//!   Source:
//!   `https://github.com/danzilberdan/opencode-muse` (`src/oauth.ts`,
//!   `src/constants.ts`). Treat as an observed contract, not an official
//!   guarantee: it can change or break without notice.
//!
//! ## Safety rules enforced here
//!
//! - HTTP redirects are disabled and requests are bounded by timeouts.
//! - Errors never include response bodies or credential material.
//! - `verification_uri` must be `https://auth.meta.com/...`; the minted
//!   `base_url` must equal exactly `https://api.meta.ai/v1`.
//! - Subscription gates fail closed: the key is only persisted when the mint
//!   response reports `is_subs_active == true` **and** `require_payment ==
//!   false`. Absent billing flags also fail closed. There is deliberately
//!   **no** pay-as-you-go / env-var fallback in this module; the
//!   `is_subs_active` flag is self-reported server metadata, not
//!   independently verified billing settlement, so the guarantee here is
//!   only that no key is persisted without an explicit active-subscription
//!   signal.
//! - Credentials live in Jcode's own file (`muse_subscription.json` under
//!   `JCODE_HOME`), never in Muse's files. [`std::fmt::Debug`] redacts
//!   secrets.
//!
//! No live Meta credentials are used by this module's tests: every test talks
//! to a loopback stub only.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Builtin compatible-profile id for the Muse subscription route.
pub const PROFILE_ID: &str = "muse-subscription";
/// Metadata hint: env var name a compatible profile would use. This is a
/// hint only; the stored JSON file is the source of truth and there is no
/// pay-as-you-go env fallback.
pub const API_KEY_ENV: &str = "JCODE_MUSE_SUBSCRIPTION_API_KEY";
/// Metadata hint: config file name a compatible profile would use. Hint only.
pub const ENV_FILE: &str = "muse-subscription.env";
/// Legacy opt-in gate, kept as a no-op for backward compatibility.
/// Muse subscription login is generally available: no opt-in is required.
/// Existing shell profiles may keep exporting it, but its value has no effect.
pub const EXPERIMENTAL_ENV_VAR: &str = "JCODE_EXPERIMENTAL_MUSE_SUBSCRIPTION";

/// Public OAuth client id shipped inside the official `muse` launcher. This is
/// a public constant, not a secret (numeric public client id).
pub const MUSE_CLIENT_ID: &str = "1031625952748946"; // gitleaks:allow
/// First-party OAuth host from the official launcher.
pub const AUTH_HOST: &str = "https://auth.meta.com";
const DEVICE_AUTHORIZATION_PATH: &str = "/oidc/device/authorization/";
const DEVICE_TOKEN_PATH: &str = "/oidc/device/token/";
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// Observed mint endpoint (community-client contract, see module docs).
pub const MINT_URL: &str = "https://api.meta.ai/muse-code/key";
/// The only API base URL ever accepted from a mint response.
pub const EXPECTED_API_BASE_URL: &str = "https://api.meta.ai/v1";

/// Jcode-owned credential file name (under `JCODE_HOME`), never a Muse file.
const CREDENTIALS_FILE_NAME: &str = "muse_subscription.json";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_BODY_LEN: usize = 64 * 1024;
/// Observed client re-mint checkpoint: daily re-mint with a safety window.
/// This is client policy, not server-declared expiry.
const MINT_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const REFRESH_SAFETY_WINDOW_MS: i64 = 5 * 60 * 1000;

/// Notice surfaced before any Muse subscription auth runs.
///
/// Covers the third-party-client status, the billing behavior, and that no
/// local Muse CLI install is needed for this flow.
pub fn experimental_warning_message() -> &'static str {
    "Muse subscription login uses the official launcher's public device \
     flow plus a key-mint step, without requiring a local Muse CLI \
     install. It is a third-party client and is not supported by Meta: \
     Meta may restrict or block this kind of access at any time, and \
     subscription coverage here is unverified: this flow never claims \
     guaranteed subscription coverage and offers no billing support, so a \
     subscription that requires payment cannot be used."
}

/// Muse subscription login is generally available: this always succeeds.
///
/// Retained for source compatibility with existing callers. Subscription,
/// payment and credential validation remain enforced independently.
pub fn ensure_enabled() -> Result<()> {
    Ok(())
}

/// Short-lived HTTP client: redirects disabled, timeouts bounded.
///
/// Callers that already own a client may pass their own to
/// [`begin_login`]/[`poll_login`], but that client must also disable
/// redirects; prefer this constructor.
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        // Meta's device-authorization endpoint answers a request with no
        // `User-Agent` with `302 -> facebook.com/unsupportedbrowser` instead of
        // the challenge JSON. Since this client deliberately does not follow
        // redirects, omitting the header made the login impossible. A plain
        // application user-agent is accepted; browser-like ones are rejected
        // with HTTP 400. Verified against the live endpoint.
        .user_agent(jcode_provider_core::JCODE_USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("Failed to build Muse subscription HTTP client")
}

/// Device authorization challenge returned by [`begin_login`].
///
/// Field-for-field compatible with the scriptable-pending shape the CLI
/// persists: all fields are public and (de)serializable. [`std::fmt::Debug`]
/// redacts `device_code` (long-lived secret material) while leaving
/// `user_code` visible for diagnostics.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceChallenge {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    /// Seconds until the device code expires (clamped to 60..=1800).
    pub expires_in: i64,
    /// Poll interval in seconds: 1..=3600 enforced in [`begin_login`]
    /// (defaults to 5 when the server omits it).
    pub interval: u64,
}

impl std::fmt::Debug for DeviceChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceChallenge")
            .field("device_code", &"[REDACTED]")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &self.verification_uri_complete)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// Single poll attempt outcome. [`poll_login`] performs exactly one token
/// request and never sleeps; the caller loops honoring `interval`.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // One-shot poll result, never stored in bulk.
pub enum PollOutcome {
    /// User has not approved yet; poll again after `interval` seconds.
    Pending,
    /// Server asked us to back off; poll again after `retry_after` seconds.
    SlowDown { retry_after: u64 },
    /// Approved. The minted credentials are not persisted by polling, and the caller
    /// must call [`save_credentials`] explicitly after its own cancellation checks.
    Approved(MuseCredentials),
    /// The user denied the request.
    Denied,
    /// The device code expired before approval.
    Expired,
}

/// Last-known subscription usage snapshot stored alongside the mint.
///
/// Display-only: [`usage_summary`] labels it as a snapshot, never live data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SubsUsage {
    #[serde(default)]
    pub window_used_percent: Option<f64>,
    #[serde(default)]
    pub window_duration_mins: Option<i64>,
    #[serde(default)]
    pub window_resets_at: Option<i64>,
    #[serde(default)]
    pub weekly_used_percent: Option<f64>,
    #[serde(default)]
    pub weekly_resets_at: Option<i64>,
    #[serde(default)]
    pub tier: Option<String>,
}

/// Stored Muse subscription credentials (Jcode-owned file).
///
/// [`std::fmt::Debug`] is redacted: neither the API key nor the OAuth access
/// token is ever printed.
#[derive(Clone, Serialize, Deserialize)]
pub struct MuseCredentials {
    pub api_key: String,
    pub access_token: String,
    pub base_url: String,
    #[serde(default)]
    pub user_email: Option<String>,
    #[serde(default)]
    pub user_full_name: Option<String>,
    pub is_subs_active: bool,
    /// Unix millis when the key was minted (client-side re-mint checkpoint).
    pub minted_at_ms: i64,
    #[serde(default)]
    pub subs_usage: Option<SubsUsage>,
}

impl std::fmt::Debug for MuseCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MuseCredentials")
            .field("api_key", &"[REDACTED]")
            .field("access_token", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("user_email", &self.user_email)
            .field("user_full_name", &self.user_full_name)
            .field("is_subs_active", &self.is_subs_active)
            .field("minted_at_ms", &self.minted_at_ms)
            .field("subs_usage", &self.subs_usage)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorizationResponse {
    #[serde(default)]
    device_code: Option<String>,
    #[serde(default)]
    user_code: Option<String>,
    #[serde(default)]
    verification_uri: Option<String>,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    interval: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Observed mint response shape (community-client contract, see module docs).
/// Only the fields we gate on are modeled; unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct MintResponse {
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    user_full_name: Option<String>,
    #[serde(default)]
    user_email: Option<String>,
    #[serde(default)]
    require_payment: Option<bool>,
    #[serde(default)]
    is_subs_active: Option<bool>,
    #[serde(default)]
    subs_tier_id: Option<String>,
    #[serde(default)]
    subs_tier_name: Option<String>,
    #[serde(default)]
    subs_usage: Option<MintSubsUsage>,
}

#[derive(Debug, Deserialize)]
struct MintSubsUsage {
    #[serde(default)]
    window: Option<MintUsageWindow>,
    #[serde(default)]
    weekly: Option<MintUsageWeekly>,
    #[serde(default)]
    tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MintUsageWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    window_duration_mins: Option<i64>,
    #[serde(default)]
    resets_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MintUsageWeekly {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<i64>,
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct TestEndpoints {
    auth_base: String,
    api_base: String,
}

#[cfg(test)]
static TEST_ENDPOINTS: std::sync::Mutex<Option<TestEndpoints>> = std::sync::Mutex::new(None);

/// Point this module at loopback stubs. Test-only: production always uses the
/// first-party hosts.
#[cfg(test)]
pub(crate) fn set_test_endpoints(auth_base: &str, api_base: &str) -> TestEndpointGuard {
    *TEST_ENDPOINTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TestEndpoints {
        auth_base: auth_base.to_string(),
        api_base: api_base.to_string(),
    });
    TestEndpointGuard
}

#[cfg(test)]
pub(crate) struct TestEndpointGuard;

#[cfg(test)]
impl Drop for TestEndpointGuard {
    fn drop(&mut self) {
        *TEST_ENDPOINTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[cfg(test)]
fn test_endpoints() -> Option<TestEndpoints> {
    TEST_ENDPOINTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn device_authorization_url() -> String {
    #[cfg(test)]
    if let Some(endpoints) = test_endpoints() {
        return format!("{}{DEVICE_AUTHORIZATION_PATH}", endpoints.auth_base);
    }
    format!("{AUTH_HOST}{DEVICE_AUTHORIZATION_PATH}")
}

fn device_token_url() -> String {
    #[cfg(test)]
    if let Some(endpoints) = test_endpoints() {
        return format!("{}{DEVICE_TOKEN_PATH}", endpoints.api_base);
    }
    format!("{AUTH_HOST}{DEVICE_TOKEN_PATH}")
}

fn mint_url() -> String {
    #[cfg(test)]
    if let Some(endpoints) = test_endpoints() {
        return format!("{}/muse-code/key", endpoints.api_base);
    }
    MINT_URL.to_string()
}

#[cfg(test)]
fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(host, Some("127.0.0.1") | Some("::1") | Some("localhost"))
}

/// The server-supplied verification page must be first-party HTTPS, except in
/// tests where an active loopback override may serve plain HTTP loopback.
fn verification_url_allowed(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    #[cfg(test)]
    if test_endpoints().is_some()
        && parsed.scheme() == "http"
        && is_loopback_host(parsed.host_str())
    {
        return true;
    }
    parsed.scheme() == "https" && parsed.host_str() == Some("auth.meta.com")
}

fn validate_challenge(challenge: &DeviceChallenge) -> Result<()> {
    if challenge.device_code.trim().is_empty() {
        anyhow::bail!("Muse device challenge carries an empty device code");
    }
    if challenge.user_code.trim().is_empty() {
        anyhow::bail!("Muse device challenge carries an empty user code");
    }
    if challenge.verification_uri.trim().is_empty() {
        anyhow::bail!("Muse device challenge carries an empty verification URI");
    }
    if !verification_url_allowed(&challenge.verification_uri) {
        anyhow::bail!("Muse verification URI is not a first-party https://auth.meta.com page");
    }
    if let Some(complete) = challenge.verification_uri_complete.as_deref()
        && !complete.trim().is_empty()
        && !verification_url_allowed(complete)
    {
        anyhow::bail!(
            "Muse verification-complete URI is not a first-party https://auth.meta.com page"
        );
    }
    Ok(())
}

/// Read a bounded JSON body so an oversized response cannot exhaust memory.
///
/// Chunks are streamed with a hard byte bound: the full body is never
/// buffered past `MAX_BODY_LEN`. The body is only used for parsing; it is
/// never embedded in errors.
async fn read_bounded_json<T>(mut response: reqwest::Response) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let mut buffered = Vec::new();
    loop {
        let chunk = response.chunk().await.context("Muse request failed")?;
        let Some(bytes) = chunk else {
            break;
        };
        if buffered.len().saturating_add(bytes.len()) > MAX_BODY_LEN {
            anyhow::bail!("Muse response exceeded size limit");
        }
        buffered.extend_from_slice(&bytes);
    }
    // Drop serde's value-specific error detail: it can echo attacker-supplied
    // body values into the anyhow chain.
    serde_json::from_slice(&buffered)
        .map_err(|_| anyhow::anyhow!("Muse response was not valid JSON/expected shape"))
}

/// Start the RFC 8628 device flow: `POST {auth}/oidc/device/authorization/`
/// with only `client_id` (mirrors the official launcher: no scope).
pub async fn begin_login(client: &reqwest::Client) -> Result<DeviceChallenge> {
    ensure_enabled()?;
    let response = client
        .post(device_authorization_url())
        .form(&[("client_id", MUSE_CLIENT_ID)])
        .send()
        .await
        .context("Muse sign-in could not be started")?;
    if response.status().is_redirection() {
        anyhow::bail!("Muse sign-in service redirected the request; not following it");
    }
    if !response.status().is_success() {
        let status = response.status();
        anyhow::bail!("Muse sign-in could not be started (HTTP {status})");
    }
    let parsed: DeviceAuthorizationResponse = read_bounded_json(response).await?;
    let device_code = parsed.device_code.unwrap_or_default();
    let user_code = parsed.user_code.unwrap_or_default();
    let verification_uri = parsed.verification_uri.unwrap_or_default();
    if device_code.trim().is_empty() || user_code.trim().is_empty() {
        anyhow::bail!("Muse sign-in response carried no usable device code");
    }
    if !verification_url_allowed(&verification_uri) {
        anyhow::bail!("Muse verification URI is not a first-party https://auth.meta.com page");
    }
    let verification_uri_complete = parsed
        .verification_uri_complete
        .filter(|uri| !uri.trim().is_empty() && verification_url_allowed(uri));
    // Mirror the launcher clamps: lifetime into 60..=1800s. Interval is
    // bounded to 1..=3600s so a rogue/malicious server cannot make callers
    // sleep unbounded (or divide/poll at zero); default 5s when omitted.
    let expires_in = parsed.expires_in.unwrap_or(900).clamp(60, 1800);
    let interval = parsed
        .interval
        .filter(|value| (1..=3600).contains(value))
        .unwrap_or(5) as u64;
    let challenge = DeviceChallenge {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in,
        interval,
    };
    validate_challenge(&challenge)?;
    Ok(challenge)
}

/// Poll once for the device token. Performs exactly one token request and
/// never sleeps; on approval it mints the subscription key and returns
/// [`PollOutcome::Approved`]. This function never persists anything: the
/// caller owns persistence and must call [`save_credentials`] explicitly
/// after its own cancellation checks, so a `/cancel` can never leave
/// signed-in credentials behind.
pub async fn poll_login(
    client: &reqwest::Client,
    challenge: &DeviceChallenge,
) -> Result<PollOutcome> {
    ensure_enabled()?;
    validate_challenge(challenge)?;
    poll_login_by_code(client, &challenge.device_code, challenge.interval).await
}

/// Single poll attempt for a raw device code (for scriptable-pending flows
/// that persist only the code and interval, not the full [`DeviceChallenge`]).
pub async fn poll_login_by_code(
    client: &reqwest::Client,
    device_code: &str,
    interval_secs: u64,
) -> Result<PollOutcome> {
    ensure_enabled()?;
    if device_code.trim().is_empty() {
        anyhow::bail!("Muse device code cannot be empty");
    }
    let safe_interval = if interval_secs == 0 { 5 } else { interval_secs };
    let response = client
        .post(device_token_url())
        .form(&[
            ("grant_type", DEVICE_CODE_GRANT),
            ("device_code", device_code),
            ("client_id", MUSE_CLIENT_ID),
        ])
        .send()
        .await
        .context("Muse sign-in poll failed")?;
    if response.status().is_redirection() {
        anyhow::bail!("Muse sign-in service redirected the request; not following it");
    }
    let status = response.status();
    let parsed: DeviceTokenResponse = read_bounded_json(response).await?;
    if status.is_success() {
        if let Some(token) = parsed.access_token.filter(|token| !token.trim().is_empty()) {
            let credentials = mint_credentials(client, &token).await?;
            return Ok(PollOutcome::Approved(credentials));
        }
        if parsed.error.as_deref().is_none() {
            anyhow::bail!("Muse sign-in response carried no usable token");
        }
    }
    match parsed.error.as_deref() {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        Some("slow_down") => Ok(PollOutcome::SlowDown {
            retry_after: safe_interval.saturating_add(5),
        }),
        Some("access_denied") => Ok(PollOutcome::Denied),
        Some("expired_token") => Ok(PollOutcome::Expired),
        Some(_) => anyhow::bail!("Muse sign-in failed (HTTP {status})"),
        None => anyhow::bail!("Muse sign-in failed (HTTP {status})"),
    }
}

/// Exchange a device OAuth access token for a subscription API key using the
/// observed mint contract (`POST https://api.meta.ai/muse-code/key`, Bearer
/// token, JSON body `{}`). Fails closed on revoked logins, inactive
/// subscriptions, payment requirements, unknown billing flags, or an
/// unexpected `base_url`. Does not persist anything; see [`save_credentials`].
pub async fn mint_credentials(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<MuseCredentials> {
    ensure_enabled()?;
    if access_token.trim().is_empty() {
        anyhow::bail!("Muse access token cannot be empty");
    }
    let response = client
        .post(mint_url())
        .bearer_auth(access_token.trim())
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .context("Muse key mint failed")?;
    if response.status().is_redirection() {
        anyhow::bail!("Muse key service redirected the request; not following it");
    }
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        anyhow::bail!("Muse saved login is no longer valid; run login again");
    }
    if !status.is_success() {
        anyhow::bail!("Muse key mint failed (HTTP {status})");
    }
    let parsed: MintResponse = read_bounded_json(response).await?;
    let api_key = parsed.api_key.unwrap_or_default();
    if api_key.trim().is_empty() {
        anyhow::bail!("Muse key mint returned an empty api key");
    }
    match parsed.base_url.as_deref() {
        Some(EXPECTED_API_BASE_URL) => {}
        _ => anyhow::bail!("Muse key mint returned an unexpected base URL"),
    }
    // Fail closed on billing: only an explicitly active subscription with an
    // explicitly absent payment requirement may produce a key. Absent flags
    // also fail closed (unknown response shapes must not silently pass), and
    // there is no pay-as-you-go fallback anywhere in this module.
    if parsed.is_subs_active != Some(true) {
        anyhow::bail!("Muse subscription is not active; refusing to store a key");
    }
    if parsed.require_payment != Some(false) {
        anyhow::bail!("Muse subscription payment status unverified; refusing to store a key");
    }
    Ok(MuseCredentials {
        api_key,
        access_token: access_token.trim().to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: parsed.user_email.filter(|value| !value.trim().is_empty()),
        user_full_name: parsed
            .user_full_name
            .filter(|value| !value.trim().is_empty()),
        is_subs_active: true,
        minted_at_ms: chrono::Utc::now().timestamp_millis(),
        subs_usage: Some(usage_snapshot(
            parsed.subs_usage.as_ref(),
            parsed.subs_tier_name.as_deref(),
            parsed.subs_tier_id.as_deref(),
        )),
    })
}

/// Build the stored usage snapshot, preferring the structured `subs_usage`
/// tier and falling back to the top-level `subs_tier_name`/`subs_tier_id`
/// fields when the nested object omits it.
fn usage_snapshot(
    usage: Option<&MintSubsUsage>,
    tier_name: Option<&str>,
    tier_id: Option<&str>,
) -> SubsUsage {
    let tier = usage
        .and_then(|usage| usage.tier.as_deref())
        .filter(|tier| !tier.trim().is_empty())
        .or_else(|| tier_name.filter(|name| !name.trim().is_empty()))
        .or_else(|| tier_id.filter(|id| !id.trim().is_empty()))
        .map(str::to_string);
    SubsUsage {
        window_used_percent: usage
            .and_then(|usage| usage.window.as_ref().and_then(|window| window.used_percent)),
        window_duration_mins: usage.and_then(|usage| {
            usage
                .window
                .as_ref()
                .and_then(|window| window.window_duration_mins)
        }),
        window_resets_at: usage
            .and_then(|usage| usage.window.as_ref().and_then(|window| window.resets_at)),
        weekly_used_percent: usage
            .and_then(|usage| usage.weekly.as_ref().and_then(|weekly| weekly.used_percent)),
        weekly_resets_at: usage
            .and_then(|usage| usage.weekly.as_ref().and_then(|weekly| weekly.resets_at)),
        tier,
    }
}

/// Jcode-owned credential file path. The file lives under `JCODE_HOME` and is
/// never a Muse-owned file.
pub fn credentials_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(CREDENTIALS_FILE_NAME))
}

/// Sibling lock file path used for cross-process advisory synchronization.
pub fn credentials_lock_path() -> Result<PathBuf> {
    Ok(credentials_path()?.with_extension("lock"))
}

/// Process-wide lock synchronizing credential modifications and read-compare-write cycles.
static CREDENTIALS_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

fn credentials_mutex() -> &'static std::sync::Mutex<()> {
    CREDENTIALS_MUTEX.get_or_init(|| std::sync::Mutex::new(()))
}

/// Test-only hook invoked inside [`refresh_credentials`]' locked critical
/// section, after the final reload/compare and before the credential write.
///
/// It exists so a test can prove that a concurrent logout cannot interleave
/// between the comparison and the write. Bound to `Send` rather than
/// `Send + Sync` because the hook holds a channel receiver.
#[cfg(test)]
static REFRESH_CRITICAL_SECTION_HOOK: std::sync::Mutex<Option<Box<dyn Fn() + Send>>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) struct RefreshCriticalSectionHookGuard;

#[cfg(test)]
impl Drop for RefreshCriticalSectionHookGuard {
    fn drop(&mut self) {
        *REFRESH_CRITICAL_SECTION_HOOK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[cfg(test)]
pub(crate) fn set_refresh_critical_section_hook<F>(hook: F) -> RefreshCriticalSectionHookGuard
where
    F: Fn() + Send + 'static,
{
    *REFRESH_CRITICAL_SECTION_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(hook));
    RefreshCriticalSectionHookGuard
}

#[cfg(test)]
fn run_refresh_critical_section_hook() {
    // Take the hook so it runs at most once, and so the slot mutex is not held
    // while the hook body runs: a panicking hook must not poison the slot.
    let hook = REFRESH_CRITICAL_SECTION_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(hook) = hook {
        hook();
    }
}

/// Cross-process advisory lock on the credentials lock file.
///
/// Uses the standard library's portable `File::lock`, so the guarantee holds on
/// Windows as well as Unix. The same idiom already guards pending scriptable
/// logins, and it avoids a platform-specific downgrade that would leave the
/// refresh-versus-logout race open on non-Unix hosts.
#[derive(Debug)]
pub(crate) struct FileLockGuard {
    file: std::fs::File,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        // Releasing explicitly the lock, then dropping the handle. `lock()` is
        // blocking and retries on interruption, so no manual retry loop is
        // needed here.
        let _ = self.file.unlock();
    }
}

fn acquire_file_lock(lock_path: &Path) -> Result<FileLockGuard> {
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .with_context(|| format!("Failed to open lock file {}", lock_path.display()))?;
    crate::storage::harden_secret_file_permissions(lock_path);
    file.lock()
        .with_context(|| format!("Failed to acquire lock on {}", lock_path.display()))?;
    Ok(FileLockGuard { file })
}

/// Advisory lock guard protecting credential read-compare-write, save, and clear
/// operations against concurrent threads in the same process and against other
/// processes, on every supported platform.
pub(crate) struct CredentialsLockGuard {
    _file_guard: FileLockGuard,
    _process_guard: std::sync::MutexGuard<'static, ()>,
}

pub(crate) fn lock_credentials() -> Result<CredentialsLockGuard> {
    let process_guard = credentials_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let lock_path = credentials_lock_path()?;
    let file_guard = acquire_file_lock(&lock_path)?;
    Ok(CredentialsLockGuard {
        _file_guard: file_guard,
        _process_guard: process_guard,
    })
}

/// Persist credentials to Jcode's own file with owner-only permissions. The
/// subscription gates are re-checked fail-closed so a key can never be stored
/// for an inactive subscription, a payment requirement, or an unexpected API
/// base URL.
pub fn save_credentials(credentials: &MuseCredentials) -> Result<()> {
    let _guard = lock_credentials()?;
    save_credentials_locked(credentials)
}

/// Internal helper that persists credentials assuming [`lock_credentials`] is
/// already held by the caller.
fn save_credentials_locked(credentials: &MuseCredentials) -> Result<()> {
    ensure_enabled()?;
    validate_stored_credentials(credentials)
        .map_err(|err| anyhow::anyhow!("Refusing to store Muse credentials: {err:#}"))?;
    let path = credentials_path()?;
    let json = serde_json::to_string_pretty(credentials).context("Failed to encode credentials")?;
    crate::storage::write_text_secret(&path, &json)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    super::AuthStatus::invalidate_cache();
    Ok(())
}

/// Load stored credentials. Returns an error when none are stored or the
/// stored record fails fail-closed validation (key, access token, base URL,
/// active flag). Validation errors redact all secret material.
pub fn load_credentials() -> Result<MuseCredentials> {
    let path = credentials_path()?;
    if !path.exists() {
        anyhow::bail!("Muse subscription login not found");
    }
    crate::storage::harden_secret_file_permissions(&path);
    // Drop serde's value-specific error detail so a tampered file's values
    // cannot echo into error chains; the path context is safe.
    let credentials: MuseCredentials = crate::storage::read_json(&path)
        .map_err(|_| anyhow::anyhow!("Stored Muse credentials are not valid JSON/expected shape"))
        .with_context(|| format!("Failed to read {}", path.display()))?;
    validate_stored_credentials(&credentials)?;
    Ok(credentials)
}

/// Fail-closed validation shared by [`load_credentials`] and
/// [`save_credentials`].
fn validate_stored_credentials(credentials: &MuseCredentials) -> Result<()> {
    for (label, value) in [
        ("api key", credentials.api_key.as_str()),
        ("access token", credentials.access_token.as_str()),
    ] {
        if value.trim().is_empty() {
            anyhow::bail!("Stored Muse credentials carry an empty {label}");
        }
        if value.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
            anyhow::bail!("Stored Muse credentials carry a malformed {label}");
        }
    }
    if credentials.base_url != EXPECTED_API_BASE_URL {
        anyhow::bail!("Stored Muse credentials carry an unexpected base URL");
    }
    if !credentials.is_subs_active {
        anyhow::bail!("Stored Muse subscription is not active");
    }
    Ok(())
}

/// True when a non-empty stored key exists.
pub fn has_cached_auth() -> bool {
    load_credentials()
        .map(|credentials| !credentials.api_key.trim().is_empty())
        .unwrap_or(false)
}

/// Delete the stored credentials. Missing file is success.
pub fn clear_credentials() -> Result<()> {
    let _guard = lock_credentials()?;
    clear_credentials_locked()
}

/// Internal helper that deletes credentials assuming [`lock_credentials`] is
/// already held by the caller.
fn clear_credentials_locked() -> Result<()> {
    let path = credentials_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        super::AuthStatus::invalidate_cache();
    }
    Ok(())
}

/// Re-mint the subscription key from the stored OAuth access token and persist
/// the result. Fails closed (no pay-as-you-go fallback) when the login was
/// revoked or the subscription is no longer active.
///
/// To avoid resurrecting a logged-out session, the stored file is reloaded
/// after the network call and the save is skipped (returning an error) when
/// the stored access token changed or disappeared mid-mint.
pub async fn refresh_credentials(client: &reqwest::Client) -> Result<MuseCredentials> {
    ensure_enabled()?;
    let stored = load_credentials()?;
    let credentials = mint_credentials(client, &stored.access_token).await?;
    let _guard = lock_credentials()?;
    let current = load_credentials();
    match current {
        Ok(current) if current.access_token == stored.access_token => {}
        _ => anyhow::bail!("Muse credentials changed during refresh; refusing to overwrite"),
    }
    #[cfg(test)]
    run_refresh_critical_section_hook();
    save_credentials_locked(&credentials)?;
    Ok(credentials)
}

/// Return stored credentials, re-minting only when the client-side daily
/// checkpoint has passed (observed client policy, not server expiry). Revoked
/// logins fail closed; they never fall back to another billing route.
pub async fn refresh_if_needed(client: &reqwest::Client) -> Result<MuseCredentials> {
    ensure_enabled()?;
    let stored = load_credentials()?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let age_ms = now_ms.saturating_sub(stored.minted_at_ms);
    if stored.minted_at_ms > 0 && age_ms < MINT_TTL_MS.saturating_sub(REFRESH_SAFETY_WINDOW_MS) {
        return Ok(stored);
    }
    refresh_credentials(client).await
}

/// Human-readable usage based on the stored mint snapshot. Labeled as a
/// snapshot, never implied to be live or per-turn data.
///
/// Only finite 0..=100 percentages and 1..=10080-minute durations are
/// rendered (values outside are dropped, never clamped or fabricated). When
/// no valid window/weekly percent survives, the summary reports
/// `quota unknown` even if a tier string exists, because tier alone says
/// nothing about remaining quota.
pub fn usage_summary() -> Option<String> {
    let credentials = load_credentials().ok()?;
    let mut parts = Vec::new();
    if let Some(usage) = credentials.subs_usage.as_ref() {
        if let Some(percent) = valid_percent(usage.window_used_percent) {
            let window = usage
                .window_duration_mins
                .filter(|mins| (1..=10_080).contains(mins))
                .map(|mins| format!("{mins}-minute"))
                .unwrap_or_else(|| "current".to_string());
            parts.push(format!("{window} window {percent:.0}% used"));
        }
        if let Some(percent) = valid_percent(usage.weekly_used_percent) {
            parts.push(format!("weekly {percent:.0}% used"));
        }
        if !parts.is_empty()
            && let Some(tier) = usage
                .tier
                .as_deref()
                .map(sanitize_tier)
                .filter(|tier| !tier.is_empty())
        {
            parts.push(format!("tier {tier}"));
        }
    }
    let detail = if parts.is_empty() {
        "quota unknown".to_string()
    } else {
        parts.join("; ")
    };
    Some(format!(
        "Muse subscription usage (last account snapshot, not live): {detail}"
    ))
}

/// Accept only finite 0..=100 percentages; reject NaN, infinities, and
/// out-of-range values instead of rendering them.
fn valid_percent(value: Option<f64>) -> Option<f64> {
    value.filter(|percent| percent.is_finite() && (0.0..=100.0).contains(percent))
}

/// Strip control characters from a tier label and cap its length so a
/// hostile snapshot cannot inject terminal escape sequences or junk.
fn sanitize_tier(tier: &str) -> String {
    tier.chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(64)
        .collect()
}

#[cfg(test)]
#[path = "muse_tests.rs"]
mod tests;
