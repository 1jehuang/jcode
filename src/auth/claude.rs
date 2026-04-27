use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::RwLock;

pub const CLAUDE_CODE_AUTH_SOURCE_ID: &str = "claude_code_credentials";
pub const OPENCODE_AUTH_SOURCE_ID: &str = "opencode_anthropic_auth";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalClaudeAuthSource {
    ClaudeCode,
    OpenCode,
}

impl ExternalClaudeAuthSource {
    pub fn source_id(self) -> &'static str {
        match self {
            Self::ClaudeCode => CLAUDE_CODE_AUTH_SOURCE_ID,
            Self::OpenCode => OPENCODE_AUTH_SOURCE_ID,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::OpenCode => "OpenCode",
        }
    }

    pub fn path(self) -> Result<PathBuf> {
        match self {
            Self::ClaudeCode => claude_code_path(),
            Self::OpenCode => opencode_path(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub subscription_type: Option<String>,
}

/// Represents a named Anthropic OAuth account stored in jcode's auth.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicAccount {
    pub label: String,
    pub access: String,
    pub refresh: String,
    pub expires: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
}

/// Multi-account jcode auth.json format.
/// Backwards-compatible: also reads the old single-account `{"anthropic": {...}}` layout.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JcodeAuthFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anthropic_accounts: Vec<AnthropicAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_anthropic_account: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    anthropic: Option<LegacyAnthropicAuth>,
}

/// Legacy single-account format (for migration).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyAnthropicAuth {
    #[serde(default)]
    access: String,
    #[serde(default)]
    refresh: String,
    #[serde(default)]
    expires: i64,
}

/// Runtime override for the active account label.
/// This allows `/account switch <label>` to take effect without rewriting the file.
static ACTIVE_ACCOUNT_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);
const ACCOUNT_LABEL_PREFIX: &str = "claude";

pub fn set_active_account_override(label: Option<String>) {
    if let Ok(mut guard) = ACTIVE_ACCOUNT_OVERRIDE.write() {
        *guard = label;
    }
}

pub fn get_active_account_override() -> Option<String> {
    ACTIVE_ACCOUNT_OVERRIDE.read().ok().and_then(|g| g.clone())
}

pub fn primary_account_label() -> String {
    crate::auth::account_store::canonical_account_label(ACCOUNT_LABEL_PREFIX, 1)
}

pub fn next_account_label() -> Result<String> {
    let auth = load_auth_file()?;
    Ok(crate::auth::account_store::next_account_label(
        ACCOUNT_LABEL_PREFIX,
        auth.anthropic_accounts.len(),
    ))
}

pub fn login_target_label(requested: Option<&str>) -> Result<String> {
    let auth = load_auth_file()?;
    Ok(crate::auth::account_store::login_target_label(
        ACCOUNT_LABEL_PREFIX,
        requested,
        auth.active_anthropic_account,
        &auth.anthropic_accounts,
        |account| account.label.as_str(),
    ))
}

fn relabel_accounts(auth: &mut JcodeAuthFile) -> bool {
    let outcome = crate::auth::account_store::relabel_accounts(
        ACCOUNT_LABEL_PREFIX,
        &mut auth.anthropic_accounts,
        &mut auth.active_anthropic_account,
        get_active_account_override(),
        |account| account.label.as_str(),
        |account, label| account.label = label,
    );
    if let Some(label) = outcome.canonical_override_label {
        set_active_account_override(Some(label));
    }
    outcome.changed
}

// -- Claude Code credentials file format --
#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOAuth>,
}

#[derive(Deserialize)]
struct ClaudeOAuth {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

// -- OpenCode auth.json format --
#[derive(Deserialize)]
struct OpenCodeAuth {
    anthropic: Option<OpenCodeAnthropicAuth>,
}

#[derive(Deserialize)]
struct OpenCodeAnthropicAuth {
    access: String,
    refresh: String,
    expires: i64,
}

fn claude_code_path() -> Result<PathBuf> {
    crate::storage::user_home_path(".claude/.credentials.json")
}

fn opencode_path() -> Result<PathBuf> {
    crate::storage::user_home_path(".local/share/opencode/auth.json")
}

pub fn jcode_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("auth.json"))
}

// ---- Multi-account helpers ----

/// Read the jcode auth file, auto-migrating from legacy format if needed.
pub fn load_auth_file() -> Result<JcodeAuthFile> {
    let path = jcode_path()?;
    if !path.exists() {
        return Ok(JcodeAuthFile::default());
    }

    crate::storage::harden_secret_file_permissions(&path);

    let mut auth: JcodeAuthFile = crate::storage::read_json(&path)
        .with_context(|| format!("Could not read jcode credentials from {:?}", path))?;

    if auth.anthropic_accounts.is_empty()
        && let Some(legacy) = auth.anthropic.take()
        && !legacy.access.is_empty()
    {
        crate::logging::info("Migrating legacy single-account auth.json to multi-account format");
        auth.anthropic_accounts.push(AnthropicAccount {
            label: "default".to_string(),
            access: legacy.access,
            refresh: legacy.refresh,
            expires: legacy.expires,
            email: None,
            subscription_type: Some("max".to_string()),
        });
        auth.active_anthropic_account = Some("default".to_string());
        let _ = save_auth_file(&auth);
    }

    if relabel_accounts(&mut auth) {
        crate::logging::info(
            "Renaming Claude accounts to numbered labels (claude-1, claude-2, ...)",
        );
        save_auth_file(&auth)?;
    }

    Ok(auth)
}

/// Write the jcode auth file (multi-account format).
pub fn save_auth_file(auth: &JcodeAuthFile) -> Result<()> {
    let auth_path = jcode_path()?;

    let clean = JcodeAuthFile {
        anthropic_accounts: auth.anthropic_accounts.clone(),
        active_anthropic_account: auth.active_anthropic_account.clone(),
        anthropic: None,
    };

    crate::storage::write_json_secret(&auth_path, &clean)?;
    Ok(())
}

/// List all configured Anthropic accounts.
pub fn list_accounts() -> Result<Vec<AnthropicAccount>> {
    let auth = load_auth_file()?;
    Ok(auth.anthropic_accounts)
}

/// Get the label of the currently active account (runtime override > file > first account).
pub fn active_account_label() -> Option<String> {
    let auth = load_auth_file().ok()?;
    crate::auth::account_store::active_account_label(
        get_active_account_override(),
        auth.active_anthropic_account,
        &auth.anthropic_accounts,
        |account| account.label.as_str(),
    )
}

/// Persist the active account choice to disk (and set the runtime override).
pub fn set_active_account(label: &str) -> Result<()> {
    let mut auth = load_auth_file()?;
    crate::auth::account_store::set_active_account(
        label,
        &auth.anthropic_accounts,
        &mut auth.active_anthropic_account,
        "No account with label '{}' found",
        |account| account.label.as_str(),
    )?;
    save_auth_file(&auth)?;
    set_active_account_override(Some(label.to_string()));
    Ok(())
}

/// Add or update an account. Returns the label used.
pub fn upsert_account(account: AnthropicAccount) -> Result<String> {
    let mut auth = load_auth_file()?;
    let label = crate::auth::account_store::upsert_account(
        ACCOUNT_LABEL_PREFIX,
        &mut auth.anthropic_accounts,
        &mut auth.active_anthropic_account,
        account,
        |account| account.label.as_str(),
        |account, label| account.label = label,
    );
    save_auth_file(&auth)?;
    Ok(label)
}

/// Remove an account by label.
pub fn remove_account(label: &str) -> Result<()> {
    let mut auth = load_auth_file()?;
    let before = auth.anthropic_accounts.len();
    auth.anthropic_accounts.retain(|a| a.label != label);
    if auth.anthropic_accounts.len() == before {
        anyhow::bail!("No account with label '{}' found", label);
    }

    if auth.active_anthropic_account.as_deref() == Some(label) {
        auth.active_anthropic_account = auth.anthropic_accounts.first().map(|a| a.label.clone());
    }

    save_auth_file(&auth)?;

    if get_active_account_override().as_deref() == Some(label) {
        set_active_account_override(auth.active_anthropic_account.clone());
    }

    Ok(())
}

/// Update tokens for a specific account (called after token refresh).
pub fn update_account_tokens(label: &str, access: &str, refresh: &str, expires: i64) -> Result<()> {
    let mut auth = load_auth_file()?;
    if let Some(account) = auth
        .anthropic_accounts
        .iter_mut()
        .find(|a| a.label == label)
    {
        account.access = access.to_string();
        account.refresh = refresh.to_string();
        account.expires = expires;
        save_auth_file(&auth)?;
        Ok(())
    } else {
        anyhow::bail!("No account with label '{}' found for token update", label);
    }
}

/// Update profile metadata for a specific account.
pub fn update_account_profile(label: &str, email: Option<String>) -> Result<()> {
    let mut auth = load_auth_file()?;
    if let Some(account) = auth
        .anthropic_accounts
        .iter_mut()
        .find(|a| a.label == label)
    {
        account.email = email;
        save_auth_file(&auth)?;
        Ok(())
    } else {
        anyhow::bail!("No account with label '{}' found for profile update", label);
    }
}

// ---- Credential loading (used by provider) ----

/// Check if OAuth credentials are available (quick check, doesn't validate)
pub fn has_credentials() -> bool {
    load_credentials().is_ok()
}

pub fn preferred_external_auth_source() -> Option<ExternalClaudeAuthSource> {
    [
        ExternalClaudeAuthSource::ClaudeCode,
        ExternalClaudeAuthSource::OpenCode,
    ]
    .into_iter()
    .find(|source| source.path().map(|path| path.exists()).unwrap_or(false))
}

pub fn has_unconsented_external_auth() -> Option<ExternalClaudeAuthSource> {
    let source = preferred_external_auth_source()?;
    let allowed = source
        .path()
        .ok()
        .map(|path| match source {
            ExternalClaudeAuthSource::OpenCode => {
                crate::config::Config::external_auth_source_allowed_for_path(
                    source.source_id(),
                    &path,
                ) || crate::config::Config::external_auth_source_allowed_for_path(
                    crate::auth::external::OPENCODE_AUTH_JSON_SOURCE_ID,
                    &path,
                )
            }
            ExternalClaudeAuthSource::ClaudeCode => {
                crate::config::Config::external_auth_source_allowed_for_path(
                    source.source_id(),
                    &path,
                )
            }
        })
        .unwrap_or(false);
    if allowed { None } else { Some(source) }
}

pub fn trust_external_auth_source(source: ExternalClaudeAuthSource) -> Result<()> {
    let path = source.path()?;
    crate::config::Config::allow_external_auth_source_for_path(source.source_id(), &path)?;
    if matches!(source, ExternalClaudeAuthSource::OpenCode) {
        crate::config::Config::allow_external_auth_source_for_path(
            crate::auth::external::OPENCODE_AUTH_JSON_SOURCE_ID,
            &path,
        )?;
    }
    super::AuthStatus::invalidate_cache();
    Ok(())
}

/// Get the subscription type (e.g., "pro", "max") if available.
pub fn get_subscription_type() -> Option<String> {
    load_credentials().ok().and_then(|c| c.subscription_type)
}

/// Check if the subscription is Claude Max (allows Opus models).
/// Returns true if subscription type is "max" or unknown (benefit of the doubt).
pub fn is_max_subscription() -> bool {
    match get_subscription_type() {
        Some(t) => t != "pro",
        None => true,
    }
}

/// Load credentials for the active Anthropic account.
/// Falls through Claude Code -> jcode accounts -> OpenCode, preferring non-expired tokens.
pub fn load_credentials() -> Result<ClaudeCredentials> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut expired_candidates: Vec<(&str, ClaudeCredentials)> = Vec::new();

    if claude_code_path()
        .ok()
        .map(|path| {
            crate::config::Config::external_auth_source_allowed_for_path(
                CLAUDE_CODE_AUTH_SOURCE_ID,
                &path,
            )
        })
        .unwrap_or(false)
        && let Ok(creds) = load_claude_code_credentials()
    {
        if creds.expires_at > now_ms {
            return Ok(creds);
        }
        expired_candidates.push(("claude", creds));
    }

    if let Ok(creds) = load_jcode_credentials() {
        if creds.expires_at > now_ms {
            return Ok(creds);
        }
        expired_candidates.push(("jcode", creds));
    }

    if opencode_path()
        .ok()
        .map(|path| {
            crate::config::Config::external_auth_source_allowed_for_path(
                OPENCODE_AUTH_SOURCE_ID,
                &path,
            ) || crate::config::Config::external_auth_source_allowed_for_path(
                crate::auth::external::OPENCODE_AUTH_JSON_SOURCE_ID,
                &path,
            )
        })
        .unwrap_or(false)
        && let Ok(creds) = load_opencode_credentials()
    {
        if creds.expires_at > now_ms {
            return Ok(creds);
        }
        expired_candidates.push(("opencode", creds));
    }

    if let Some((_source, creds)) = expired_candidates.into_iter().next() {
        return Ok(creds);
    }

    anyhow::bail!("No Claude OAuth credentials found (checked Claude Code, jcode, OpenCode)")
}

/// Load credentials for a specific jcode account by label.
pub fn load_credentials_for_account(label: &str) -> Result<ClaudeCredentials> {
    let auth = load_auth_file()?;
    let account = auth
        .anthropic_accounts
        .iter()
        .find(|a| a.label == label)
        .with_context(|| format!("No account with label '{}'", label))?;

    Ok(ClaudeCredentials {
        access_token: account.access.clone(),
        refresh_token: account.refresh.clone(),
        expires_at: account.expires,
        subscription_type: account.subscription_type.clone(),
    })
}

/// Load credentials from the active jcode account (multi-account aware).
fn load_jcode_credentials() -> Result<ClaudeCredentials> {
    let auth = load_auth_file()?;
    if auth.anthropic_accounts.is_empty() {
        anyhow::bail!("No anthropic accounts configured in jcode auth.json");
    }

    let active_label = get_active_account_override()
        .or(auth.active_anthropic_account)
        .unwrap_or_else(primary_account_label);

    let account = auth
        .anthropic_accounts
        .iter()
        .find(|a| a.label == active_label)
        .or_else(|| auth.anthropic_accounts.first())
        .context("No anthropic accounts in jcode auth.json")?;

    Ok(ClaudeCredentials {
        access_token: account.access.clone(),
        refresh_token: account.refresh.clone(),
        expires_at: account.expires,
        subscription_type: account
            .subscription_type
            .clone()
            .or_else(|| Some("max".to_string())),
    })
}

fn load_claude_code_credentials() -> Result<ClaudeCredentials> {
    let path = crate::storage::validate_external_auth_file(&claude_code_path()?)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read credentials from {:?}", path))?;

    let file: CredentialsFile =
        serde_json::from_str(&content).context("Could not parse Claude credentials")?;

    let oauth = file
        .claude_ai_oauth
        .context("No claudeAiOauth found in credentials")?;

    Ok(ClaudeCredentials {
        access_token: oauth.access_token,
        refresh_token: oauth.refresh_token,
        expires_at: oauth.expires_at,
        subscription_type: oauth.subscription_type,
    })
}

pub fn load_opencode_credentials() -> Result<ClaudeCredentials> {
    let path = crate::storage::validate_external_auth_file(&opencode_path()?)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read OpenCode credentials from {:?}", path))?;

    let anthropic = serde_json::from_str::<OpenCodeAuth>(&content)
        .ok()
        .and_then(|auth| auth.anthropic)
        .map(|anthropic| ClaudeCredentials {
            access_token: anthropic.access,
            refresh_token: anthropic.refresh,
            expires_at: anthropic.expires,
            subscription_type: Some("max".to_string()),
        })
        .or_else(|| {
            crate::auth::external::load_anthropic_oauth_tokens().map(|tokens| ClaudeCredentials {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                expires_at: tokens.expires_at,
                subscription_type: Some("max".to_string()),
            })
        })
        .context("No anthropic OAuth credentials in OpenCode auth file")?;

    let now_ms = chrono::Utc::now().timestamp_millis();
    if anthropic.expires_at <= now_ms {
        crate::logging::info("OpenCode Anthropic token expired; will attempt refresh.");
    }
    crate::logging::info("Using OpenCode Anthropic credentials");

    Ok(anthropic)
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
