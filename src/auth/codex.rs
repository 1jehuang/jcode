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
    auth.get("chatgpt_account_id")?
        .as_str()
        .map(|s| s.to_string())
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload_b64 = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()).ok()?;
    serde_json::from_slice::<Value>(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_file_with_oauth_tokens() {
        let json = r#"{
            "tokens": {
                "access_token": "at_openai_123",
                "refresh_token": "rt_openai_456",
                "id_token": "header.payload.signature",
                "account_id": "acct_789",
                "expires_at": 9999999999999
            }
        }"#;
        let file: AuthFile = serde_json::from_str(json).unwrap();
        let tokens = file.tokens.unwrap();
        assert_eq!(tokens.access_token, "at_openai_123");
        assert_eq!(tokens.refresh_token, "rt_openai_456");
        assert_eq!(tokens.id_token, Some("header.payload.signature".to_string()));
        assert_eq!(tokens.account_id, Some("acct_789".to_string()));
        assert_eq!(tokens.expires_at, Some(9999999999999));
    }

    #[test]
    fn auth_file_with_api_key_only() {
        let json = r#"{
            "OPENAI_API_KEY": "sk-test-key-123"
        }"#;
        let file: AuthFile = serde_json::from_str(json).unwrap();
        assert!(file.tokens.is_none());
        assert_eq!(file.api_key, Some("sk-test-key-123".to_string()));
    }

    #[test]
    fn auth_file_empty() {
        let json = r#"{}"#;
        let file: AuthFile = serde_json::from_str(json).unwrap();
        assert!(file.tokens.is_none());
        assert!(file.api_key.is_none());
    }

    #[test]
    fn auth_file_minimal_tokens() {
        let json = r#"{
            "tokens": {
                "access_token": "at",
                "refresh_token": "rt"
            }
        }"#;
        let file: AuthFile = serde_json::from_str(json).unwrap();
        let tokens = file.tokens.unwrap();
        assert_eq!(tokens.access_token, "at");
        assert!(tokens.id_token.is_none());
        assert!(tokens.account_id.is_none());
        assert!(tokens.expires_at.is_none());
    }

    #[test]
    fn decode_jwt_payload_valid() {
        let payload = serde_json::json!({
            "sub": "user123",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_abc"
            }
        });
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{}.signature", payload_b64);

        let decoded = decode_jwt_payload(&token).unwrap();
        assert_eq!(decoded["sub"], "user123");
    }

    #[test]
    fn decode_jwt_payload_invalid() {
        assert!(decode_jwt_payload("not-a-jwt").is_none());
        assert!(decode_jwt_payload("").is_none());
        assert!(decode_jwt_payload("a.!!!invalid-base64.c").is_none());
    }

    #[test]
    fn extract_account_id_from_jwt() {
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_test_123"
            }
        });
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{}.signature", payload_b64);

        let account_id = extract_account_id(&token);
        assert_eq!(account_id, Some("acct_test_123".to_string()));
    }

    #[test]
    fn extract_account_id_missing_field() {
        let payload = serde_json::json!({"sub": "user123"});
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{}.signature", payload_b64);

        assert!(extract_account_id(&token).is_none());
    }

    #[test]
    fn extract_account_id_invalid_token() {
        assert!(extract_account_id("garbage").is_none());
    }

    #[test]
    fn codex_credentials_from_oauth() {
        let json = r#"{
            "tokens": {
                "access_token": "at_test",
                "refresh_token": "rt_test",
                "expires_at": 5000
            }
        }"#;
        let file: AuthFile = serde_json::from_str(json).unwrap();
        let tokens = file.tokens.unwrap();
        assert_eq!(tokens.access_token, "at_test");
        assert_eq!(tokens.refresh_token, "rt_test");
        assert_eq!(tokens.expires_at, Some(5000));
    }
}
