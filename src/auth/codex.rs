use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
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
        return Ok(CodexCredentials {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            id_token: tokens.id_token,
        });
    }

    // Fall back to API key if available
    if let Some(api_key) = file.api_key {
        return Ok(CodexCredentials {
            access_token: api_key,
            refresh_token: String::new(),
            id_token: None,
        });
    }

    anyhow::bail!("No tokens or API key found in Codex auth file")
}
