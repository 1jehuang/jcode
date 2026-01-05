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

fn credentials_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".claude").join(".credentials.json"))
}

pub fn load_credentials() -> Result<ClaudeCredentials> {
    let path = credentials_path()?;
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
        anyhow::bail!(
            "Claude OAuth token expired. Run 'claude' and '/login' to refresh."
        );
    }

    Ok(ClaudeCredentials {
        access_token: oauth.access_token,
        refresh_token: oauth.refresh_token,
        expires_at: oauth.expires_at,
        subscription_type: oauth.subscription_type,
    })
}
