use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<Tokens>,
    #[serde(rename = "OPENAI_API_KEY")]
    api_key: Option<String>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    account_id: Option<String>,
    expires_at: Option<i64>,
}

fn auth_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex").join("auth.json"))
}

pub fn load_credentials() -> Result<CodexCredentials> {
    let path = auth_path()?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read credentials from {:?}", path))?;

    let file: AuthFile =
        serde_json::from_str(&content).context("Could not parse Codex credentials")?;

    // Prefer OAuth tokens over API key
    if let Some(tokens) = file.tokens {
        let account_id = tokens
            .account_id
            .clone()
            .or_else(|| tokens.id_token.as_deref().and_then(extract_account_id));
        return Ok(CodexCredentials {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            id_token: tokens.id_token,
            account_id,
            expires_at: tokens.expires_at,
        });
    }

    // Fall back to API key if available
    if let Some(api_key) = file.api_key {
        return Ok(CodexCredentials {
            access_token: api_key,
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            expires_at: None,
        });
    }

    anyhow::bail!("No tokens or API key found in Codex auth file")
}

fn extract_account_id(id_token: &str) -> Option<String> {
    let payload = decode_jwt_payload(id_token)?;
    let auth = payload.get("https://api.openai.com/auth")?;
    auth.get("chatgpt_account_id")?.as_str().map(|s| s.to_string())
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload_b64 = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()).ok()?;
    serde_json::from_slice::<Value>(&decoded).ok()
}
