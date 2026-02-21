#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ClaudeCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub subscription_type: Option<String>,
}

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

fn jcode_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".jcode").join("auth.json"))
}

// OpenCode auth.json format
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

// jcode auth.json format
#[derive(Deserialize)]
struct JcodeAuth {
    anthropic: Option<JcodeAnthropicAuth>,
}

#[derive(Deserialize)]
struct JcodeAnthropicAuth {
    access: String,
    refresh: String,
    expires: i64,
}

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
        None => true, // unknown = don't restrict
    }
}

pub fn load_credentials() -> Result<ClaudeCredentials> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Try valid credentials in preferred order.
    // jcode is first so tokens refreshed by jcode are used immediately next launch.
    let mut expired_candidates = Vec::new();
    for (source, loader) in [
        (
            "jcode",
            load_jcode_credentials as fn() -> Result<ClaudeCredentials>,
        ),
        (
            "claude",
            load_claude_code_credentials as fn() -> Result<ClaudeCredentials>,
        ),
        (
            "opencode",
            load_opencode_credentials as fn() -> Result<ClaudeCredentials>,
        ),
    ] {
        if let Ok(creds) = loader() {
            if creds.expires_at > now_ms {
                return Ok(creds);
            }
            expired_candidates.push((source, creds));
        }
    }

    // Fall back to any available credentials (even if expired)
    if let Some((source, creds)) = expired_candidates.into_iter().next() {
        crate::logging::info(&format!(
            "{} Claude OAuth token expired; will attempt refresh.",
            source
        ));
        return Ok(creds);
    }

    anyhow::bail!("No Claude OAuth credentials found (checked jcode, Claude Code, OpenCode)")
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

fn load_jcode_credentials() -> Result<ClaudeCredentials> {
    let path = jcode_path()?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read jcode credentials from {:?}", path))?;

    let auth: JcodeAuth =
        serde_json::from_str(&content).context("Could not parse jcode auth credentials")?;

    let anthropic = auth
        .anthropic
        .context("No anthropic OAuth credentials in jcode auth file")?;

    Ok(ClaudeCredentials {
        access_token: anthropic.access,
        refresh_token: anthropic.refresh,
        expires_at: anthropic.expires,
        subscription_type: Some("max".to_string()),
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
