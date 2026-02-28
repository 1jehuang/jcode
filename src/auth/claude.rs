#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::RwLock;

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

pub fn set_active_account_override(label: Option<String>) {
    if let Ok(mut guard) = ACTIVE_ACCOUNT_OVERRIDE.write() {
        *guard = label;
    }
}

pub fn get_active_account_override() -> Option<String> {
    ACTIVE_ACCOUNT_OVERRIDE.read().ok().and_then(|g| g.clone())
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
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".claude").join(".credentials.json"))
}

fn opencode_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("auth.json"))
}

pub fn jcode_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".jcode").join("auth.json"))
}

// ---- Multi-account helpers ----

/// Read the jcode auth file, auto-migrating from legacy format if needed.
pub fn load_auth_file() -> Result<JcodeAuthFile> {
    let path = jcode_path()?;
    if !path.exists() {
        return Ok(JcodeAuthFile::default());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read jcode credentials from {:?}", path))?;

    let mut auth: JcodeAuthFile =
        serde_json::from_str(&content).context("Could not parse jcode auth.json")?;

    if auth.anthropic_accounts.is_empty() {
        if let Some(legacy) = auth.anthropic.take() {
            if !legacy.access.is_empty() {
                crate::logging::info(
                    "Migrating legacy single-account auth.json to multi-account format",
                );
                auth.anthropic_accounts.push(AnthropicAccount {
                    label: "default".to_string(),
                    access: legacy.access,
                    refresh: legacy.refresh,
                    expires: legacy.expires,
                    subscription_type: Some("max".to_string()),
                });
                auth.active_anthropic_account = Some("default".to_string());
                let _ = save_auth_file(&auth);
            }
        }
    }

    Ok(auth)
}

/// Write the jcode auth file (multi-account format).
pub fn save_auth_file(auth: &JcodeAuthFile) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home directory"))?;
    let creds_dir = home.join(".jcode");
    std::fs::create_dir_all(&creds_dir)?;

    let clean = JcodeAuthFile {
        anthropic_accounts: auth.anthropic_accounts.clone(),
        active_anthropic_account: auth.active_anthropic_account.clone(),
        anthropic: None,
    };

    let json = serde_json::to_string_pretty(&clean)?;
    let auth_path = creds_dir.join("auth.json");
    std::fs::write(&auth_path, json)?;
    crate::platform::set_permissions_owner_only(&auth_path)?;
    Ok(())
}

/// List all configured Anthropic accounts.
pub fn list_accounts() -> Result<Vec<AnthropicAccount>> {
    let auth = load_auth_file()?;
    Ok(auth.anthropic_accounts)
}

/// Get the label of the currently active account (runtime override > file > first account).
pub fn active_account_label() -> Option<String> {
    if let Some(override_label) = get_active_account_override() {
        return Some(override_label);
    }
    let auth = load_auth_file().ok()?;
    auth.active_anthropic_account
        .or_else(|| auth.anthropic_accounts.first().map(|a| a.label.clone()))
}

/// Persist the active account choice to disk (and set the runtime override).
pub fn set_active_account(label: &str) -> Result<()> {
    let mut auth = load_auth_file()?;
    if !auth.anthropic_accounts.iter().any(|a| a.label == label) {
        anyhow::bail!("No account with label '{}' found", label);
    }
    auth.active_anthropic_account = Some(label.to_string());
    save_auth_file(&auth)?;
    set_active_account_override(Some(label.to_string()));
    Ok(())
}

/// Add or update an account. Returns the label used.
pub fn upsert_account(account: AnthropicAccount) -> Result<String> {
    let mut auth = load_auth_file()?;
    let label = account.label.clone();

    if let Some(existing) = auth
        .anthropic_accounts
        .iter_mut()
        .find(|a| a.label == label)
    {
        *existing = account;
    } else {
        auth.anthropic_accounts.push(account);
    }

    if auth.active_anthropic_account.is_none() || auth.anthropic_accounts.len() == 1 {
        auth.active_anthropic_account = Some(label.clone());
    }

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

// ---- Credential loading (used by provider) ----

/// Check if OAuth credentials are available (quick check, doesn't validate)
pub fn has_credentials() -> bool {
    load_credentials().is_ok()
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
/// Falls through jcode accounts -> Claude Code -> OpenCode, preferring non-expired tokens.
pub fn load_credentials() -> Result<ClaudeCredentials> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut expired_candidates: Vec<(&str, ClaudeCredentials)> = Vec::new();

    if let Ok(creds) = load_jcode_credentials() {
        if creds.expires_at > now_ms {
            return Ok(creds);
        }
        expired_candidates.push(("jcode", creds));
    }

    if let Ok(creds) = load_claude_code_credentials() {
        if creds.expires_at > now_ms {
            return Ok(creds);
        }
        expired_candidates.push(("claude", creds));
    }

    if let Ok(creds) = load_opencode_credentials() {
        if creds.expires_at > now_ms {
            return Ok(creds);
        }
        expired_candidates.push(("opencode", creds));
    }

    if let Some((source, creds)) = expired_candidates.into_iter().next() {
        crate::logging::info(&format!(
            "{} Claude OAuth token expired; will attempt refresh.",
            source
        ));
        return Ok(creds);
    }

    anyhow::bail!("No Claude OAuth credentials found (checked jcode, Claude Code, OpenCode)")
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
        .unwrap_or_else(|| "default".to_string());

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
    let path = claude_code_path()?;
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
    let path = opencode_path()?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read OpenCode credentials from {:?}", path))?;

    let auth: OpenCodeAuth =
        serde_json::from_str(&content).context("Could not parse OpenCode credentials")?;

    let anthropic = auth
        .anthropic
        .context("No anthropic OAuth credentials in OpenCode auth file")?;

    let now_ms = chrono::Utc::now().timestamp_millis();
    if anthropic.expires <= now_ms {
        crate::logging::info("OpenCode Anthropic token expired; will attempt refresh.");
    }
    crate::logging::info("Using OpenCode Anthropic credentials");

    Ok(ClaudeCredentials {
        access_token: anthropic.access,
        refresh_token: anthropic.refresh,
        expires_at: anthropic.expires,
        subscription_type: Some("max".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jcode_auth_file_default_is_empty() {
        let auth = JcodeAuthFile::default();
        assert!(auth.anthropic_accounts.is_empty());
        assert!(auth.active_anthropic_account.is_none());
    }

    #[test]
    fn jcode_auth_file_roundtrip() {
        let auth = JcodeAuthFile {
            anthropic_accounts: vec![AnthropicAccount {
                label: "work".to_string(),
                access: "acc_123".to_string(),
                refresh: "ref_456".to_string(),
                expires: 9999999999999,
                subscription_type: Some("max".to_string()),
            }],
            active_anthropic_account: Some("work".to_string()),
            anthropic: None,
        };

        let json = serde_json::to_string_pretty(&auth).unwrap();
        let parsed: JcodeAuthFile = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.anthropic_accounts.len(), 1);
        assert_eq!(parsed.anthropic_accounts[0].label, "work");
        assert_eq!(parsed.anthropic_accounts[0].access, "acc_123");
        assert_eq!(parsed.active_anthropic_account, Some("work".to_string()));
    }

    #[test]
    fn jcode_auth_file_multi_account() {
        let auth = JcodeAuthFile {
            anthropic_accounts: vec![
                AnthropicAccount {
                    label: "personal".to_string(),
                    access: "acc_personal".to_string(),
                    refresh: "ref_personal".to_string(),
                    expires: 1000,
                    subscription_type: Some("pro".to_string()),
                },
                AnthropicAccount {
                    label: "work".to_string(),
                    access: "acc_work".to_string(),
                    refresh: "ref_work".to_string(),
                    expires: 2000,
                    subscription_type: Some("max".to_string()),
                },
            ],
            active_anthropic_account: Some("work".to_string()),
            anthropic: None,
        };

        let json = serde_json::to_string(&auth).unwrap();
        let parsed: JcodeAuthFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.anthropic_accounts.len(), 2);
        assert_eq!(parsed.active_anthropic_account, Some("work".to_string()));
    }

    #[test]
    fn jcode_auth_file_legacy_migration_format() {
        let legacy_json = r#"{
            "anthropic": {
                "access": "legacy_acc",
                "refresh": "legacy_ref",
                "expires": 12345
            }
        }"#;
        let parsed: JcodeAuthFile = serde_json::from_str(legacy_json).unwrap();
        assert!(parsed.anthropic_accounts.is_empty());
        assert!(parsed.anthropic.is_some());
    }

    #[test]
    fn anthropic_account_no_subscription_type() {
        let json = r#"{
            "label": "test",
            "access": "acc",
            "refresh": "ref",
            "expires": 0
        }"#;
        let account: AnthropicAccount = serde_json::from_str(json).unwrap();
        assert_eq!(account.label, "test");
        assert!(account.subscription_type.is_none());
    }

    #[test]
    fn anthropic_account_subscription_type_serialized_when_present() {
        let account = AnthropicAccount {
            label: "test".to_string(),
            access: "acc".to_string(),
            refresh: "ref".to_string(),
            expires: 0,
            subscription_type: Some("max".to_string()),
        };
        let json = serde_json::to_string(&account).unwrap();
        assert!(json.contains("subscription_type"));
        assert!(json.contains("max"));
    }

    #[test]
    fn anthropic_account_subscription_type_omitted_when_none() {
        let account = AnthropicAccount {
            label: "test".to_string(),
            access: "acc".to_string(),
            refresh: "ref".to_string(),
            expires: 0,
            subscription_type: None,
        };
        let json = serde_json::to_string(&account).unwrap();
        assert!(!json.contains("subscription_type"));
    }

    #[test]
    fn is_max_subscription_pro_is_false() {
        // This tests the logic directly since we can't mock the file
        let sub_type = Some("pro".to_string());
        let is_max = match sub_type {
            Some(t) => t != "pro",
            None => true,
        };
        assert!(!is_max);
    }

    #[test]
    fn is_max_subscription_max_is_true() {
        let sub_type = Some("max".to_string());
        let is_max = match sub_type {
            Some(t) => t != "pro",
            None => true,
        };
        assert!(is_max);
    }

    #[test]
    fn is_max_subscription_unknown_is_true() {
        let sub_type: Option<String> = None;
        let is_max = match sub_type {
            Some(t) => t != "pro",
            None => true,
        };
        assert!(is_max);
    }

    #[test]
    fn claude_code_credentials_format() {
        let json = r#"{
            "claudeAiOauth": {
                "accessToken": "at_12345",
                "refreshToken": "rt_67890",
                "expiresAt": 9999999999999,
                "subscriptionType": "max"
            }
        }"#;
        let file: CredentialsFile = serde_json::from_str(json).unwrap();
        let oauth = file.claude_ai_oauth.unwrap();
        assert_eq!(oauth.access_token, "at_12345");
        assert_eq!(oauth.refresh_token, "rt_67890");
        assert_eq!(oauth.expires_at, 9999999999999);
        assert_eq!(oauth.subscription_type, Some("max".to_string()));
    }

    #[test]
    fn claude_code_credentials_no_subscription() {
        let json = r#"{
            "claudeAiOauth": {
                "accessToken": "at",
                "refreshToken": "rt",
                "expiresAt": 0
            }
        }"#;
        let file: CredentialsFile = serde_json::from_str(json).unwrap();
        let oauth = file.claude_ai_oauth.unwrap();
        assert!(oauth.subscription_type.is_none());
    }

    #[test]
    fn claude_code_credentials_missing_oauth() {
        let json = r#"{}"#;
        let file: CredentialsFile = serde_json::from_str(json).unwrap();
        assert!(file.claude_ai_oauth.is_none());
    }

    #[test]
    fn opencode_credentials_format() {
        let json = r#"{
            "anthropic": {
                "access": "oc_acc",
                "refresh": "oc_ref",
                "expires": 1234567890
            }
        }"#;
        let auth: OpenCodeAuth = serde_json::from_str(json).unwrap();
        let anthropic = auth.anthropic.unwrap();
        assert_eq!(anthropic.access, "oc_acc");
        assert_eq!(anthropic.refresh, "oc_ref");
        assert_eq!(anthropic.expires, 1234567890);
    }

    #[test]
    fn opencode_credentials_no_anthropic() {
        let json = r#"{}"#;
        let auth: OpenCodeAuth = serde_json::from_str(json).unwrap();
        assert!(auth.anthropic.is_none());
    }

    #[test]
    fn active_account_override_roundtrip() {
        set_active_account_override(Some("test-override".to_string()));
        assert_eq!(
            get_active_account_override(),
            Some("test-override".to_string())
        );
        set_active_account_override(None);
        assert_eq!(get_active_account_override(), None);
    }
}
