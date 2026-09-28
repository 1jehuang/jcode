//! Banked Codex resets, using the same contract as openai/codex's backend-client
//! `client/rate_limit_resets.rs`. These are earned, single-use resets, not API credits.
//! Preparation is read-only. Redemption always requires an explicit confirmation.

use super::*;
use serde::{Deserialize, Serialize};

const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";

#[derive(Debug, Deserialize)]
struct ResetCredits {
    available_count: u64,
    credits: Vec<ResetCredit>,
}

#[derive(Debug, Deserialize)]
struct ResetCredit {
    id: String,
    status: String,
    reset_type: String,
    expires_at: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

/// Only reads credit metadata. This path never prepares or redeems a reset.
pub(super) async fn fetch_available_expirations(
    client: &reqwest::Client,
    credentials: &auth::codex::CodexCredentials,
) -> Result<Vec<Option<String>>> {
    fetch_available_expirations_at(client, RESET_CREDITS_URL, credentials).await
}

async fn fetch_available_expirations_at(
    client: &reqwest::Client,
    url: &str,
    credentials: &auth::codex::CodexCredentials,
) -> Result<Vec<Option<String>>> {
    let response = authorize(client.get(url), credentials)
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    let credits: ResetCredits = decode_response(response).await?;
    Ok(available_expirations(&credits, chrono::Utc::now()))
}

fn available_expirations(
    credits: &ResetCredits,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Option<String>> {
    credits
        .credits
        .iter()
        .filter(|credit| credit.status == "available" && !credit.id.is_empty())
        .filter(|credit| {
            credit.expires_at.as_deref().is_none_or(|expiry| {
                chrono::DateTime::parse_from_rfc3339(expiry).map_or(true, |expiry| expiry > now)
            })
        })
        .take(credits.available_count.try_into().unwrap_or(usize::MAX))
        .map(|credit| {
            credit
                .expires_at
                .as_deref()
                .and_then(|expiry| chrono::DateTime::parse_from_rfc3339(expiry).ok())
                .map(|expiry| expiry.to_rfc3339())
        })
        .collect()
}

/// A confirmation pins the account, credit and idempotency key. In particular,
/// switching accounts while the prompt is visible cannot spend a different reset.
#[derive(Clone)]
pub struct PendingOpenAiUsageReset {
    credentials: auth::codex::CodexCredentials,
    account_label: Option<String>,
    account_display: String,
    credit_id: String,
    title: String,
    description: Option<String>,
    expires_at: Option<String>,
    available_count: u64,
    available_expirations: Vec<Option<String>>,
    redeem_request_id: String,
}

// Never expose bearer or refresh tokens in TUI state/debug output.
impl std::fmt::Debug for PendingOpenAiUsageReset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingOpenAiUsageReset")
            .field("account", &self.account_display)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl PendingOpenAiUsageReset {
    pub fn account_label(&self) -> Option<&str> {
        self.account_label.as_deref()
    }

    /// Human-readable account identity, safe to display.
    pub fn account_display(&self) -> &str {
        &self.account_display
    }

    /// Review lines for graphical clients: the same facts as
    /// [`Self::confirmation_message`] without the TUI command hints.
    pub fn confirmation_details(&self) -> Vec<String> {
        let mut lines = vec![
            format!("{} banked reset(s) available", self.available_count),
            format!("Selected: {}", self.title),
        ];
        if let Some(description) = &self.description {
            lines.push(display_text(description));
        }
        if let Some(expiry) = &self.expires_at {
            lines.push(format!("Selected reset expires: {}", display_text(expiry)));
        }
        lines
    }

    pub fn confirmation_message(&self) -> String {
        let mut message = format!(
            "OpenAI account: {}\n{} banked usage reset(s) available.\nSelected: {}",
            self.account_display, self.available_count, self.title,
        );
        for (index, expiry) in self.available_expirations.iter().enumerate() {
            let expiry = expiry
                .as_deref()
                .map(display_text)
                .unwrap_or_else(|| "unknown".into());
            message.push_str(&format!("\nReset {} expires: {}", index + 1, expiry));
        }
        let missing = self
            .available_count
            .saturating_sub(self.available_expirations.len() as u64);
        if missing > 0 {
            message.push_str(&format!("\nExpiry unknown for {missing} other reset(s)"));
        }
        if let Some(description) = &self.description {
            message.push_str(&format!("\n{}", display_text(description)));
        }
        if let Some(expiry) = &self.expires_at {
            message.push_str(&format!("\nExpires: {}", display_text(expiry)));
        }
        message.push_str(
            "\nThis spends one banked reset and cannot be undone. It does not buy credits or increase your plan limits.\nConfirm: /reset usage limits openai confirm\nCancel: /reset usage limits openai cancel",
        );
        message
    }
}

fn display_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(500)
        .collect()
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ResetCode {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiUsageResetOutcome {
    code: ResetCode,
    #[serde(default)]
    windows_reset: u64,
}

impl OpenAiUsageResetOutcome {
    pub fn message(&self) -> String {
        match self.code {
            ResetCode::Reset => format!(
                "OpenAI usage limits reset ({} window(s)). One banked reset was redeemed. Use /usage to view current limits.",
                self.windows_reset,
            ),
            ResetCode::NothingToReset =>
                "OpenAI reports nothing to reset. No banked reset was spent.".to_string(),
            ResetCode::NoCredit =>
                "OpenAI reports no banked reset available. No reset was applied.".to_string(),
            ResetCode::AlreadyRedeemed =>
                "This OpenAI banked reset was already redeemed. No additional reset was spent. Use /usage to check current limits.".to_string(),
        }
    }
}

#[derive(Serialize)]
struct ConsumeRequest<'a> {
    redeem_request_id: &'a str,
    credit_id: &'a str,
}

fn reset_client() -> Result<reqwest::Client> {
    // Do not follow redirects with credentials or retry a mutation automatically.
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Could not create the OpenAI reset client")
}

fn authorize(
    request: reqwest::RequestBuilder,
    credentials: &auth::codex::CodexCredentials,
) -> reqwest::RequestBuilder {
    let request = request
        .bearer_auth(&credentials.access_token)
        .header("Accept", "application/json");
    if let Some(account_id) = &credentials.account_id {
        request.header("chatgpt-account-id", account_id)
    } else {
        request
    }
}

async fn decode_response<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    if !status.is_success() {
        // Raw server bodies may contain sensitive account information or HTML.
        let hint = match status.as_u16() {
            401 => "Sign in again with /login openai.",
            403 => "This account may not be eligible for banked resets.",
            404 => "Banked resets are not available from this OpenAI endpoint.",
            429 => "OpenAI is rate limiting requests. Try again later.",
            _ => "Check your connection and try again later.",
        };
        anyhow::bail!(
            "OpenAI banked reset request failed (HTTP {}). {}",
            status.as_u16(),
            hint
        );
    }
    response
        .json()
        .await
        .context("Unrecognized OpenAI banked reset response")
}

/// Read banked reset availability for the active OpenAI OAuth account. Never POSTs.
pub async fn prepare_openai_usage_reset() -> Result<Option<PendingOpenAiUsageReset>> {
    prepare_openai_usage_reset_for_account(auth::codex::active_account_label()).await
}

/// Read banked reset availability for one OpenAI OAuth login. `None` is the
/// default (unlabelled) login. Never POSTs, and never falls back to another
/// account, so a confirmation always spends a reset from the named login.
pub async fn prepare_openai_usage_reset_for_account(
    account_label: Option<String>,
) -> Result<Option<PendingOpenAiUsageReset>> {
    // An explicit account must never silently fall back to another login or an API key.
    let mut credentials = match account_label.as_deref() {
        Some(label) => auth::codex::load_credentials_for_account(label),
        None => auth::codex::load_oauth_credentials(),
    }
    .context("Banked resets require an OpenAI ChatGPT login. Use /login openai (API keys are not supported).")?;
    if credentials.access_token.is_empty()
        || (credentials.refresh_token.is_empty() && credentials.id_token.is_none())
    {
        anyhow::bail!("Banked resets require a ChatGPT OAuth login. Use /login openai.");
    }
    if credentials
        .expires_at
        .is_some_and(|expiry| expiry < chrono::Utc::now().timestamp_millis() + 300_000)
        && !credentials.refresh_token.is_empty()
    {
        let refreshed = match account_label.as_deref() {
            Some(label) => {
                auth::oauth::refresh_openai_tokens_for_account(&credentials.refresh_token, label)
                    .await
            }
            None => auth::oauth::refresh_openai_tokens(&credentials.refresh_token).await,
        }
        .context("Could not refresh OpenAI login. Use /login openai.")?;
        credentials.access_token = refreshed.access_token;
        credentials.refresh_token = refreshed.refresh_token;
        credentials.id_token = refreshed.id_token.or(credentials.id_token);
        credentials.expires_at = Some(refreshed.expires_at);
    }
    credentials.account_id = credentials.account_id.or_else(|| {
        credentials
            .id_token
            .as_deref()
            .and_then(auth::codex::extract_account_id)
    });
    let email = credentials
        .id_token
        .as_deref()
        .and_then(auth::codex::extract_email);
    let account_display = match (account_label.as_deref(), email) {
        (Some(label), Some(email)) => format!("{} ({})", display_text(label), display_text(&email)),
        (Some(label), None) => display_text(label),
        (None, Some(email)) => display_text(&email),
        (None, None) => credentials
            .account_id
            .as_deref()
            .map(display_text)
            .unwrap_or_else(|| "current ChatGPT login".into()),
    };
    prepare_with_credentials(
        &reset_client()?,
        RESET_CREDITS_URL,
        credentials,
        account_label,
        account_display,
    )
    .await
}

async fn prepare_with_credentials(
    client: &reqwest::Client,
    url: &str,
    credentials: auth::codex::CodexCredentials,
    account_label: Option<String>,
    account_display: String,
) -> Result<Option<PendingOpenAiUsageReset>> {
    let response = authorize(client.get(url), &credentials)
        .send()
        .await
        .context("Could not check OpenAI banked resets")?;
    let credits: ResetCredits = decode_response(response).await?;
    if credits.available_count == 0 {
        return Ok(None);
    }
    let now = chrono::Utc::now();
    let available_expirations = available_expirations(&credits, now);
    let credit = credits
        .credits
        .into_iter()
        .filter(|credit| credit.status == "available" && !credit.id.is_empty())
        .filter(|credit| {
            credit.expires_at.as_deref().is_none_or(|expiry| {
                chrono::DateTime::parse_from_rfc3339(expiry).is_ok_and(|expiry| expiry > now)
            })
        })
        // Spend the soonest-expiring credit first, with non-expiring credits last.
        .min_by_key(|credit| {
            credit
                .expires_at
                .as_deref()
                .and_then(|expiry| chrono::DateTime::parse_from_rfc3339(expiry).ok())
                .map(|expiry| expiry.timestamp())
                .unwrap_or(i64::MAX)
        });
    let Some(credit) = credit else {
        anyhow::bail!(
            "OpenAI reports banked resets, but none can currently be selected. Check https://chatgpt.com/codex/settings/usage."
        );
    };
    Ok(Some(PendingOpenAiUsageReset {
        credentials,
        account_label,
        account_display,
        credit_id: credit.id,
        title: display_text(credit.title.as_deref().unwrap_or(&credit.reset_type)),
        description: credit.description,
        expires_at: credit.expires_at,
        available_count: credits.available_count,
        available_expirations,
        redeem_request_id: uuid::Uuid::new_v4().to_string(),
    }))
}

/// Spend exactly the credit the user confirmed. Keep `pending` on errors and reuse
/// it for retries: a timed-out POST may already have succeeded on the server.
pub async fn consume_openai_usage_reset(
    pending: &PendingOpenAiUsageReset,
) -> Result<OpenAiUsageResetOutcome> {
    let result = consume_with_client(&reset_client()?, RESET_CREDITS_URL, pending).await;
    // Invalidate even on uncertain errors: a timed-out POST may have reset usage.
    super::cache::invalidate_openai_usage_after_reset(
        &pending.credentials.access_token,
        pending.account_label.as_deref(),
    );
    invalidate_openai_usage_reset_state(pending.account_label.as_deref()).await;
    result
}

/// Drop cached quota and temporary unavailability for this account after a reset.
/// Also used by the daemon: its process-local caches are independent of the TUI.
/// This only invalidates local state, never redeems or purchases anything.
pub async fn invalidate_openai_usage_reset_state(account_label: Option<&str>) {
    invalidate_openai_usage_cache(account_label).await;
    crate::provider::clear_openai_provider_unavailability_for_account_label(account_label);
}

/// Force the next read-only quota check to fetch fresh data after a hard limit.
/// Unlike reset completion, this deliberately preserves provider cooldowns.
pub async fn invalidate_openai_usage_cache(account_label: Option<&str>) {
    super::cache::invalidate_openai_usage_after_reset("", account_label);
    if auth::codex::active_account_label().as_deref() == account_label {
        *get_openai_usage_cell().await.write().await = OpenAIUsageData::default();
    }
}

async fn consume_with_client(
    client: &reqwest::Client,
    url: &str,
    pending: &PendingOpenAiUsageReset,
) -> Result<OpenAiUsageResetOutcome> {
    let request = authorize(client.post(format!("{url}/consume")), &pending.credentials).json(
        &ConsumeRequest {
            redeem_request_id: &pending.redeem_request_id,
            credit_id: &pending.credit_id,
        },
    );
    let result = async {
        let response = request
            .send()
            .await
            .context("Could not receive OpenAI reset result")?;
        decode_response(response).await
    }
    .await;
    result.map_err(|error: anyhow::Error| anyhow::anyhow!(
        "{error:#}\nThe reset outcome may be uncertain. Check /usage. To retry this SAME redemption safely, use /reset usage limits openai confirm."
    ))
}

#[cfg(test)]
mod tests;
