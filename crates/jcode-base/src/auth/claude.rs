use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const CLAUDE_CODE_AUTH_SOURCE_ID: &str = "claude_code_credentials";
pub const OPENCODE_AUTH_SOURCE_ID: &str = "opencode_anthropic_auth";
/// Source id used to remember that the user approved importing Claude Code's
/// native credentials (macOS Keychain item or `CLAUDE_CODE_OAUTH_TOKEN`). These
/// have no stable on-disk path, so they are tracked as a source-level trust
/// rather than a path-bound one.
pub const CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID: &str = "claude_code_native_credentials";

/// macOS login Keychain service name used by Claude Code (v2.1+) to store its
/// OAuth credentials. The stored secret is a JSON blob with `accessToken`,
/// `refreshToken`, and `expiresAt`.
const CLAUDE_CODE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Environment variable Claude Code reads its OAuth token from (set by
/// `claude setup-token`, and prioritized by Claude Code over the Keychain).
const CLAUDE_CODE_OAUTH_TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

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
    pub scopes: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
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

const ACCOUNT_LABEL_PREFIX: &str = "claude";

/// Set the runtime override for the active account label.
/// This allows `/account switch <label>` to take effect without rewriting the file.
pub fn set_active_account_override(label: Option<String>) {
    crate::auth::account_store::set_runtime_active_override(ACCOUNT_LABEL_PREFIX, label);
}

pub fn get_active_account_override() -> Option<String> {
    crate::auth::account_store::runtime_active_override(ACCOUNT_LABEL_PREFIX)
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
    #[serde(rename = "refreshToken", default)]
    refresh_token: String,
    // Claude Code stores `expiresAt` as epoch milliseconds in the JSON file
    // (`~/.claude/.credentials.json`) but as an RFC 3339 string in the macOS
    // Keychain blob. Accept either so a single parser handles both sources.
    #[serde(
        rename = "expiresAt",
        default,
        deserialize_with = "deserialize_expires_at"
    )]
    expires_at: i64,
    #[serde(rename = "subscriptionType", default)]
    subscription_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_scopes")]
    scopes: Vec<String>,
}

/// Parse Claude Code's `expiresAt`, which may be epoch milliseconds (JSON file)
/// or an RFC 3339 string (macOS Keychain blob). Missing/invalid values map to
/// `0`, which the loaders treat as "expired" and refresh lazily.
fn deserialize_expires_at<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(0),
        serde_json::Value::Number(num) => {
            if let Some(int) = num.as_i64() {
                Ok(int)
            } else if let Some(float) = num.as_f64() {
                Ok(float as i64)
            } else {
                Ok(0)
            }
        }
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(0);
            }
            if let Ok(int) = trimmed.parse::<i64>() {
                return Ok(int);
            }
            chrono::DateTime::parse_from_rfc3339(trimmed)
                .map(|dt| dt.timestamp_millis())
                .map_err(|err| D::Error::custom(format!("invalid expiresAt '{trimmed}': {err}")))
        }
        other => Err(D::Error::custom(format!(
            "unexpected expiresAt type: {other}"
        ))),
    }
}

/// Claude Code usually stores `scopes` as a JSON array, but some token blobs
/// (notably `claude setup-token` output) store it as a single space-delimited
/// string. Accept both so scope checks stay correct across sources.
fn deserialize_scopes<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(ToOwned::to_owned))
            .collect(),
        Some(serde_json::Value::String(text)) => {
            text.split_whitespace().map(ToOwned::to_owned).collect()
        }
        _ => Vec::new(),
    })
}

/// Parse a Claude Code credentials blob into [`ClaudeCredentials`].
///
/// Accepts both the wrapped file form (`{"claudeAiOauth": {...}}`) and a bare
/// OAuth object (`{"accessToken": ..., ...}`), which is the shape the macOS
/// Keychain item and `CLAUDE_CODE_OAUTH_TOKEN` env var use.
fn parse_claude_code_credentials_blob(content: &str) -> Result<ClaudeCredentials> {
    let content = content.trim();
    if content.is_empty() {
        anyhow::bail!("Claude credentials blob is empty");
    }

    let oauth = if let Ok(file) = serde_json::from_str::<CredentialsFile>(content) {
        file.claude_ai_oauth
    } else {
        None
    };

    let oauth = match oauth {
        Some(oauth) => oauth,
        None => serde_json::from_str::<ClaudeOAuth>(content)
            .context("Could not parse Claude credentials (expected claudeAiOauth wrapper or a bare OAuth object)")?,
    };

    if oauth.access_token.trim().is_empty() && oauth.refresh_token.trim().is_empty() {
        anyhow::bail!("Claude credentials blob contained no access or refresh token");
    }

    Ok(ClaudeCredentials {
        access_token: oauth.access_token,
        refresh_token: oauth.refresh_token,
        expires_at: oauth.expires_at,
        scopes: oauth.scopes,
        subscription_type: oauth.subscription_type,
    })
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
            scopes: Vec::new(),
        });
        auth.active_anthropic_account = Some("default".to_string());
        let _ = save_auth_file(&auth);
    }

    if relabel_accounts(&mut auth) {
        crate::logging::info(
            "Renaming Claude accounts to animal labels (claude-otter, claude-fox, ...)",
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

/// Remove every stored Anthropic account in one write.
pub fn clear_accounts() -> Result<usize> {
    let mut auth = load_auth_file()?;
    let removed = auth.anthropic_accounts.len();
    auth.anthropic_accounts.clear();
    auth.active_anthropic_account = None;
    save_auth_file(&auth)?;
    set_active_account_override(None);
    Ok(removed)
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
            // Hot path: this runs per credential probe (route pricing, auth
            // status, subscription checks). Use the process-cached config
            // snapshot instead of re-parsing config.toml on every call.
            crate::config::Config::external_auth_source_allowed_for_path_cached(
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

    // Claude Code's `CLAUDE_CODE_OAUTH_TOKEN` env var. Reading the env var is
    // cheap and never prompts, so it is consulted live on the hot path (after
    // the user has approved importing Claude Code native credentials). The
    // macOS Keychain is NOT read here: it can trigger an interactive unlock
    // prompt, so it is read once at import time and copied into jcode's own
    // auth.json (see `import_native_credentials_into_account`).
    if native_source_allowed()
        && let Some(creds) = load_claude_code_env_credentials()
    {
        if creds.expires_at > now_ms {
            return Ok(creds);
        }
        expired_candidates.push(("claude-native", creds));
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
            crate::config::Config::external_auth_source_allowed_for_path_cached(
                OPENCODE_AUTH_SOURCE_ID,
                &path,
            ) || crate::config::Config::external_auth_source_allowed_for_path_cached(
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
        scopes: account.scopes.clone(),
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
        scopes: account.scopes.clone(),
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

    parse_claude_code_credentials_blob(&content)
        .with_context(|| format!("Could not parse Claude credentials from {:?}", path))
}

/// Read the `CLAUDE_CODE_OAUTH_TOKEN` env var if it holds a usable credential.
///
/// Claude Code (and the recovery workflow for SSH sessions) sets this to the
/// JSON blob `claude setup-token` produces. We also tolerate a bare access
/// token string for forward compatibility.
fn load_claude_code_env_credentials() -> Option<ClaudeCredentials> {
    let raw = std::env::var(CLAUDE_CODE_OAUTH_TOKEN_ENV).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(creds) = parse_claude_code_credentials_blob(trimmed) {
        return Some(creds);
    }

    // Bare token (no JSON wrapper): treat as an access token with unknown
    // expiry. expires_at = 0 marks it expired so a refresh is attempted, but a
    // bare token has no refresh token, so it is only useful while still valid.
    if !trimmed.contains('{') {
        return Some(ClaudeCredentials {
            access_token: trimmed.to_string(),
            refresh_token: String::new(),
            expires_at: 0,
            scopes: Vec::new(),
            subscription_type: None,
        });
    }

    None
}

/// macOS: read Claude Code's OAuth credentials from the login Keychain item
/// (`Claude Code-credentials`). Returns `None` on non-macOS platforms, when the
/// item is missing, or when the Keychain is locked/inaccessible (e.g. over SSH).
#[cfg(target_os = "macos")]
fn load_claude_code_keychain_credentials() -> Option<ClaudeCredentials> {
    let blob = read_claude_code_keychain_blob()?;
    match parse_claude_code_credentials_blob(&blob) {
        Ok(creds) => Some(creds),
        Err(err) => {
            crate::logging::warn(&format!(
                "Found Claude Code Keychain item but could not parse it: {err}"
            ));
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn load_claude_code_keychain_credentials() -> Option<ClaudeCredentials> {
    None
}

/// Shell out to `security find-generic-password -w` to read the Claude Code
/// Keychain secret. Bounded by a short timeout so a locked Keychain prompt can
/// never hang startup/auth probes.
#[cfg(target_os = "macos")]
fn read_claude_code_keychain_blob() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            CLAUDE_CODE_KEYCHAIN_SERVICE,
            "-w",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let timeout = Duration::from_secs(5);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                let trimmed = out.trim();
                if trimmed.is_empty() {
                    return None;
                }
                return Some(trimmed.to_string());
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    crate::logging::warn(
                        "Timed out reading Claude Code credentials from macOS Keychain (locked or prompting?)",
                    );
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

/// Whether Claude Code's native credentials (Keychain or env token) appear to
/// be present, regardless of trust. Used to surface an import candidate even
/// when the JSON credentials file does not exist (the common macOS case).
///
/// Detection is intentionally cheap and must not prompt: it checks for the
/// env var and for the Keychain item's existence (attributes only), never
/// reading the secret value. The secret is only read during an approved
/// import or at runtime load.
pub fn native_credentials_present() -> bool {
    if std::env::var(CLAUDE_CODE_OAUTH_TOKEN_ENV)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    claude_code_keychain_item_exists()
}

/// macOS: cheaply check whether the Claude Code Keychain item exists without
/// reading (and therefore without unlocking/prompting for) its secret value.
#[cfg(target_os = "macos")]
fn claude_code_keychain_item_exists() -> bool {
    use std::process::{Command, Stdio};
    Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", CLAUDE_CODE_KEYCHAIN_SERVICE])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn claude_code_keychain_item_exists() -> bool {
    false
}

/// Load Claude Code's native (Keychain or env) credentials, preferring the env
/// token (which Claude Code itself prioritizes), then the Keychain.
pub fn load_native_credentials() -> Result<ClaudeCredentials> {
    if let Some(creds) = load_claude_code_env_credentials() {
        return Ok(creds);
    }
    if let Some(creds) = load_claude_code_keychain_credentials() {
        return Ok(creds);
    }
    anyhow::bail!(
        "No Claude Code native credentials found ({CLAUDE_CODE_OAUTH_TOKEN_ENV} env or macOS Keychain)"
    )
}

/// Whether the user has approved importing Claude Code's native credentials.
pub fn native_source_allowed() -> bool {
    crate::config::Config::external_auth_source_allowed(CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID)
}

/// Human-facing name for the Claude Code native credential source, reflecting
/// where the credentials actually live on this platform.
pub fn native_source_display_name() -> &'static str {
    if std::env::var(CLAUDE_CODE_OAUTH_TOKEN_ENV)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return "Claude Code (CLAUDE_CODE_OAUTH_TOKEN)";
    }
    if cfg!(target_os = "macos") {
        "Claude Code (macOS Keychain)"
    } else {
        "Claude Code (native credentials)"
    }
}

/// A display-only "path" hint for the native source, used by review UIs that
/// expect a location string. The Keychain/env credentials have no real file
/// path, so this returns a descriptive pseudo-location.
pub fn native_source_path_hint() -> PathBuf {
    if std::env::var(CLAUDE_CODE_OAUTH_TOKEN_ENV)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return PathBuf::from(format!("env:{CLAUDE_CODE_OAUTH_TOKEN_ENV}"));
    }
    if cfg!(target_os = "macos") {
        PathBuf::from(format!("keychain:{CLAUDE_CODE_KEYCHAIN_SERVICE}"))
    } else {
        PathBuf::from("claude-code-native")
    }
}

/// Remember approval to import Claude Code's native credentials.
pub fn trust_native_source() -> Result<()> {
    crate::config::Config::allow_external_auth_source(CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID)?;
    super::AuthStatus::invalidate_cache();
    Ok(())
}

/// Read Claude Code's native credentials once (Keychain or env token) and copy
/// them into a jcode-managed Anthropic account so future loads and token
/// refreshes are served from jcode's own `auth.json`.
///
/// This is the durable import path for the macOS Keychain: we do not want to
/// hit the Keychain on every request (it can prompt to unlock), so we snapshot
/// the credentials at import time. The OAuth refresh token is long-lived and
/// rotates into jcode's store on refresh, so the copy stays usable after the
/// original Claude Code token expires.
///
/// Returns the label of the account the credentials were stored under.
pub fn import_native_credentials_into_account() -> Result<String> {
    let creds = load_native_credentials()
        .context("Could not read Claude Code native credentials to import")?;

    if creds.refresh_token.trim().is_empty() {
        // Without a refresh token we cannot keep the account alive past the
        // current access token, so we do not persist a dead-end account.
        // The env-token path still works live via `load_claude_code_env_credentials`.
        anyhow::bail!(
            "Claude Code native credentials have no refresh token; nothing durable to import"
        );
    }

    let label = login_target_label(None)?;
    upsert_account(AnthropicAccount {
        label: label.clone(),
        access: creds.access_token,
        refresh: creds.refresh_token,
        expires: creds.expires_at,
        email: None,
        subscription_type: creds.subscription_type.or_else(|| Some("max".to_string())),
        scopes: creds.scopes,
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
            scopes: Vec::new(),
            subscription_type: Some("max".to_string()),
        })
        .or_else(|| {
            crate::auth::external::load_anthropic_oauth_tokens().map(|tokens| ClaudeCredentials {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                expires_at: tokens.expires_at,
                scopes: Vec::new(),
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
