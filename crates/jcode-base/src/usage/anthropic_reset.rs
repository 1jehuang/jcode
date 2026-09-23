//! Claude session-limit resets, using the same contract as Claude Code's
//! `/limit-reset` command. The offer is read from the usage endpoint at the
//! five-hour wall and claimed against the account's organization. Reading is
//! side-effect free. Claiming requires an explicit confirmation and spends the
//! account's reset for the week.

use super::*;
use serde::Deserialize;

const API_BASE: &str = "https://api.anthropic.com";
const STATUS_PATH: &str = "/api/oauth/usage?at_wall=1&skip_spend=1";
const PROFILE_PATH: &str = "/api/oauth/profile";
/// Server-side program name of the weekly session-limit reset.
const PROGRAM: &str = "juniper_tide";

pub use jcode_usage_types::AnthropicLimitResetOffer;

#[derive(Debug, Deserialize)]
struct StatusEnvelope {
    juniper_tide: Option<StatusBlock>,
}

#[derive(Debug, Deserialize)]
struct StatusBlock {
    #[serde(default)]
    eligible: bool,
    #[serde(default)]
    arm: Option<String>,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    next_available_at: Option<String>,
    #[serde(default)]
    resets_per_week: Option<u64>,
}

/// Parse the status block from an at-wall usage response. `None` means the
/// account is not offered resets at all, so no reset control should appear.
fn offer_from_status(
    value: &serde_json::Value,
    account_label: Option<&str>,
) -> Option<AnthropicLimitResetOffer> {
    let envelope: StatusEnvelope = serde_json::from_value(value.clone()).ok()?;
    let block = envelope.juniper_tide?;
    // Claude Code only offers a claim to eligible accounts in the reset arm.
    if !block.eligible || block.arm.as_deref() != Some("reset") {
        return None;
    }
    Some(AnthropicLimitResetOffer {
        account_label: account_label.map(str::to_string),
        available: block.available,
        next_available_at: block.next_available_at.filter(|at| valid_timestamp(at)),
        resets_per_week: block.resets_per_week.unwrap_or(1).max(1),
    })
}

fn valid_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn authorize(request: reqwest::RequestBuilder, access_token: &str) -> reqwest::RequestBuilder {
    crate::provider::anthropic::apply_oauth_attribution_headers(
        request
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                crate::provider::anthropic::CLAUDE_CLI_USER_AGENT,
            )
            .header("anthropic-beta", "oauth-2025-04-20,claude-code-20250219"),
        &crate::provider::anthropic::new_oauth_request_id(),
    )
}

fn reset_client() -> Result<reqwest::Client> {
    // Never follow redirects with credentials or retry the claim automatically.
    reqwest::Client::builder()
        .timeout(Duration::from_secs(35))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Could not create the Claude reset client")
}

fn http_failure(status: reqwest::StatusCode) -> anyhow::Error {
    // Raw bodies may carry account details or HTML, so show a hint instead.
    let hint = match status.as_u16() {
        401 | 403 => "Sign in to Claude again, then try again.",
        404 => "Session-limit resets are not available for this login.",
        429 => "Claude is rate limiting requests. Try again later.",
        _ => "Check your connection and try again later.",
    };
    anyhow::anyhow!(
        "Claude reset request failed (HTTP {}). {}",
        status.as_u16(),
        hint
    )
}

async fn get_json(client: &reqwest::Client, url: &str, token: &str) -> Result<serde_json::Value> {
    let response = authorize(client.get(url), token)
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .context("Could not reach Claude")?;
    if !response.status().is_success() {
        return Err(http_failure(response.status()));
    }
    response
        .json()
        .await
        .context("Unrecognized Claude usage response")
}

/// Read-only offer lookup for the usage report. Only meaningful at the wall.
pub(super) async fn fetch_limit_reset_offer(
    access_token: &str,
    account_label: Option<&str>,
) -> Option<AnthropicLimitResetOffer> {
    let client = reset_client().ok()?;
    let value = get_json(&client, &format!("{API_BASE}{STATUS_PATH}"), access_token)
        .await
        .ok()?;
    offer_from_status(&value, account_label)
}

/// A confirmation pins the login, organization and token, so switching the
/// active account while the prompt is visible cannot spend another account's reset.
#[derive(Clone)]
pub struct PendingAnthropicLimitReset {
    access_token: String,
    organization_uuid: String,
    account_label: Option<String>,
    account_display: String,
    resets_per_week: u64,
}

// Never expose bearer tokens in client state or logs.
impl std::fmt::Debug for PendingAnthropicLimitReset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingAnthropicLimitReset")
            .field("account", &self.account_display)
            .finish_non_exhaustive()
    }
}

impl PendingAnthropicLimitReset {
    pub fn account_label(&self) -> Option<&str> {
        self.account_label.as_deref()
    }

    pub fn account_display(&self) -> &str {
        &self.account_display
    }

    /// Review lines for graphical clients.
    pub fn confirmation_details(&self) -> Vec<String> {
        vec![
            "Resets your Claude five-hour session limit right away.".to_string(),
            "Your weekly limit still applies.".to_string(),
            format!(
                "{} session reset(s) per week. The next one becomes available after this one is used.",
                self.resets_per_week
            ),
        ]
    }
}

/// Why a reset cannot be prepared right now, safe to show to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnthropicLimitResetUnavailable {
    /// This login is not offered session resets, or is not at the wall.
    NotOffered,
    /// The weekly reset was already used.
    Spent { next_available_at: Option<String> },
}

impl AnthropicLimitResetUnavailable {
    pub fn message(&self) -> String {
        match self {
            Self::NotOffered => "A session-limit reset isn't available right now.".to_string(),
            Self::Spent {
                next_available_at: Some(at),
            } => format!(
                "This week's session-limit reset was already used. The next one is available in {}.",
                format_reset_time(at)
            ),
            Self::Spent { .. } => "This week's session-limit reset was already used.".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProfileResponse {
    #[serde(default)]
    organization: Option<ProfileOrganization>,
    #[serde(default)]
    account: Option<ProfileAccount>,
}

#[derive(Debug, Deserialize)]
struct ProfileOrganization {
    uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileAccount {
    email: Option<String>,
}

fn display_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}

async fn fresh_access_token(account_label: Option<&str>) -> Result<String> {
    let credentials = match account_label {
        Some(label) => auth::claude::load_credentials_for_account(label),
        None => auth::claude::load_credentials(),
    }
    .context("Session-limit resets need a Claude subscription login. Sign in to Claude first.")?;
    if credentials.access_token.is_empty() {
        anyhow::bail!("Session-limit resets need a Claude subscription login.");
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    if credentials.expires_at >= now_ms + 300_000 {
        return Ok(credentials.access_token);
    }
    // Refresh only stored accounts. An explicit label never falls back to another login.
    match account_label {
        Some(label) if !credentials.refresh_token.is_empty() => Ok(
            auth::oauth::refresh_claude_tokens_for_account(&credentials.refresh_token, label)
                .await
                .context("Could not refresh the Claude login. Sign in to Claude again.")?
                .access_token,
        ),
        _ if credentials.expires_at > now_ms => Ok(credentials.access_token),
        _ => anyhow::bail!("The Claude login has expired. Sign in to Claude again."),
    }
}

/// Check whether one Claude login can claim a session-limit reset. Never claims.
pub async fn prepare_anthropic_limit_reset(
    account_label: Option<String>,
) -> Result<std::result::Result<PendingAnthropicLimitReset, AnthropicLimitResetUnavailable>> {
    let access_token = fresh_access_token(account_label.as_deref()).await?;
    prepare_with_token(&reset_client()?, API_BASE, access_token, account_label).await
}

async fn prepare_with_token(
    client: &reqwest::Client,
    base: &str,
    access_token: String,
    account_label: Option<String>,
) -> Result<std::result::Result<PendingAnthropicLimitReset, AnthropicLimitResetUnavailable>> {
    let status = get_json(client, &format!("{base}{STATUS_PATH}"), &access_token).await?;
    let Some(offer) = offer_from_status(&status, account_label.as_deref()) else {
        return Ok(Err(AnthropicLimitResetUnavailable::NotOffered));
    };
    if !offer.available {
        return Ok(Err(AnthropicLimitResetUnavailable::Spent {
            next_available_at: offer.next_available_at,
        }));
    }
    let profile: ProfileResponse = serde_json::from_value(
        get_json(client, &format!("{base}{PROFILE_PATH}"), &access_token).await?,
    )
    .context("Unrecognized Claude profile response")?;
    let organization_uuid = profile
        .organization
        .and_then(|organization| organization.uuid)
        .filter(|uuid| {
            !uuid.is_empty()
                && uuid
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .context("This Claude login has no organization, so its limits cannot be reset.")?;
    let email = profile.account.and_then(|account| account.email);
    let account_display = match (account_label.as_deref(), email) {
        (Some(label), Some(email)) => format!("{} ({})", display_text(label), display_text(&email)),
        (Some(label), None) => display_text(label),
        (None, Some(email)) => display_text(&email),
        (None, None) => "current Claude login".to_string(),
    };
    Ok(Ok(PendingAnthropicLimitReset {
        access_token,
        organization_uuid,
        account_label,
        account_display,
        resets_per_week: offer.resets_per_week,
    }))
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ClaimResult {
    Reset,
    AlreadyUsed,
    NotLimited,
    Ineligible,
    #[serde(other)]
    Unavailable,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicLimitResetOutcome {
    result: ClaimResult,
    #[serde(default)]
    next_available_at: Option<String>,
}

impl AnthropicLimitResetOutcome {
    /// Whether limits are now clear, so a waiting request can proceed.
    pub fn limits_cleared(&self) -> bool {
        matches!(self.result, ClaimResult::Reset | ClaimResult::NotLimited)
    }

    pub fn message(&self) -> String {
        let next = self
            .next_available_at
            .as_deref()
            .filter(|at| valid_timestamp(at))
            .map(|at| format!(" The next reset is available in {}.", format_reset_time(at)))
            .unwrap_or_default();
        match self.result {
            ClaimResult::Reset => {
                format!("Claude session limit reset. Your weekly limit still applies.{next}")
            }
            ClaimResult::NotLimited => {
                "Claude reports your session limit is already clear. No reset was needed."
                    .to_string()
            }
            ClaimResult::AlreadyUsed => {
                format!("This week's session-limit reset was already used.{next}")
            }
            ClaimResult::Ineligible => {
                "A session-limit reset isn't available for this login.".to_string()
            }
            ClaimResult::Unavailable => {
                "A session-limit reset isn't available right now. Try again later.".to_string()
            }
        }
    }
}

/// Claim the confirmed login's reset. Safe to retry after an uncertain error:
/// a claim that already succeeded answers `already_used` instead of spending twice.
pub async fn consume_anthropic_limit_reset(
    pending: &PendingAnthropicLimitReset,
) -> Result<AnthropicLimitResetOutcome> {
    let result = consume_with_client(&reset_client()?, API_BASE, pending).await;
    // Invalidate even on uncertain errors: a timed-out claim may have applied.
    invalidate_anthropic_usage_reset_state(pending.account_label.as_deref());
    result
}

async fn consume_with_client(
    client: &reqwest::Client,
    base: &str,
    pending: &PendingAnthropicLimitReset,
) -> Result<AnthropicLimitResetOutcome> {
    let url = format!(
        "{base}/api/organizations/{}/reset_rate_limits",
        pending.organization_uuid
    );
    let result = async {
        let response = authorize(client.post(url), &pending.access_token)
            .json(&serde_json::json!({ "program": PROGRAM }))
            .send()
            .await
            .context("Could not receive the Claude reset result")?;
        if !response.status().is_success() {
            return Err(http_failure(response.status()));
        }
        response
            .json::<AnthropicLimitResetOutcome>()
            .await
            .context("Unrecognized Claude reset response")
    }
    .await;
    result.map_err(|error| {
        anyhow::anyhow!(
            "{error:#}\nThe reset outcome may be uncertain. Check your usage before trying again."
        )
    })
}

/// Drop cached quota and temporary unavailability for one Claude login after a
/// reset. Only local state changes, so this is safe to repeat.
pub fn invalidate_anthropic_usage_reset_state(account_label: Option<&str>) {
    super::cache::invalidate_anthropic_usage_after_reset(account_label);
    crate::provider::clear_claude_provider_unavailability_for_account_label(account_label);
}

#[cfg(test)]
#[path = "anthropic_reset_tests.rs"]
mod tests;
