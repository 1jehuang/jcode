#![allow(dead_code)]
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

pub fn load_credentials() -> Result<ClaudeCredentials> {
    // First try OpenCode credentials (they work with the API)
    if let Ok(creds) = load_opencode_credentials() {
        return Ok(creds);
    }

    // Fall back to Claude Code credentials
    let path = claude_code_path()?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read credentials from {:?}", path))?;

    let file: CredentialsFile =
        serde_json::from_str(&content).context("Could not parse Claude credentials")?;

    let oauth = file
        .claude_ai_oauth
        .context("No claudeAiOauth found in credentials")?;

    // Check if token is expired
    let now_ms = chrono::Utc::now().timestamp_millis();
    if oauth.expires_at < now_ms {
        crate::logging::info("Claude OAuth token expired; will attempt refresh.");
    }

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
