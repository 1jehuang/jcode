#![allow(dead_code)]
#![allow(dead_code)]

use crate::auth::claude as claude_auth;
use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

/// Claude Code OAuth configuration
pub mod claude {
    pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
    pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
    pub const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
    pub const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
    pub const SCOPES: &str = "org:create_api_key user:profile user:inference";
}

/// OpenAI Codex OAuth configuration
pub mod openai {
    pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
    pub const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
    pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
    pub const REDIRECT_URI: &str = "http://localhost:9876/callback";
    pub const SCOPES: &str = "openid profile email offline_access";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
}

/// Generate PKCE code verifier and challenge
fn generate_pkce() -> (String, String) {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    let verifier: String = (0..64)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    let challenge = URL_SAFE_NO_PAD.encode(hash);

    (verifier, challenge)
}

/// Generate random state for CSRF protection
fn generate_state() -> String {
    let bytes: [u8; 16] = rand::random();
    hex::encode(bytes)
}

/// Start local server and wait for OAuth callback
fn wait_for_callback(port: u16, expected_state: &str) -> Result<String> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
    eprintln!("Waiting for OAuth callback on port {}...", port);

    let (mut stream, _) = listener.accept()?;
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse the request to get the code
    // GET /callback?code=xxx&state=yyy HTTP/1.1
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        anyhow::bail!("Invalid HTTP request");
    }

    let path = parts[1];
    let url = url::Url::parse(&format!("http://localhost{}", path))?;

    let code = url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No code in callback"))?;

    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No state in callback"))?;

    if state != expected_state {
        anyhow::bail!("State mismatch - possible CSRF attack");
    }

    // Send success response
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
        <html><body><h1>Success!</h1><p>You can close this window.</p></body></html>";
    stream.write_all(response.as_bytes())?;

    Ok(code)
}

/// Async version of wait_for_callback using tokio (for use from TUI context)
pub async fn wait_for_callback_async(port: u16, expected_state: &str) -> Result<String> {
    let expected_state = expected_state.to_string();
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await?;

    let (stream, _) = listener.accept().await?;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        anyhow::bail!("Invalid HTTP request");
    }

    let path = parts[1];
    let url = url::Url::parse(&format!("http://localhost{}", path))?;

    let code = url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No code in callback"))?;

    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| anyhow::anyhow!("No state in callback"))?;

    if state != expected_state {
        anyhow::bail!("State mismatch - possible CSRF attack");
    }

    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
        <html><body><h1>Success!</h1><p>You can close this window and return to jcode.</p></body></html>";
    writer.write_all(response.as_bytes()).await?;

    Ok(code)
}

/// Perform OAuth login for Claude
pub async fn login_claude() -> Result<OAuthTokens> {
    let (verifier, challenge) = generate_pkce();
    let _state = generate_state();

    // Build authorization URL (matching OpenCode's format)
    let auth_url = format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        claude::AUTHORIZE_URL,
        claude::CLIENT_ID,
        urlencoding::encode(claude::REDIRECT_URI),
        urlencoding::encode(claude::SCOPES),
        challenge,
        verifier  // state is the verifier in OpenCode's implementation
    );

    eprintln!("\nOpen this URL in your browser:\n");
    eprintln!("{}\n", auth_url);
    eprintln!("Opening browser for Claude login...\n");

    // Try to open browser
    let _ = open::that(&auth_url);

    eprintln!("After logging in, you'll see a page with an authorization code.");
    eprintln!("Copy and paste the code here:\n");
    eprint!("> ");
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();

    // Parse the code - OpenCode format is "code#state" where state=verifier
    // The code might be pasted directly or as part of a URL
    let raw_code = if input.contains("code=") {
        let url = url::Url::parse(input)
            .or_else(|_| url::Url::parse(&format!("https://example.com?{}", input)))?;
        url.query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
            .ok_or_else(|| anyhow::anyhow!("No code found in URL"))?
    } else {
        input.to_string()
    };

    // Split code#state format
    let (code, state_from_callback) = if raw_code.contains('#') {
        let parts: Vec<&str> = raw_code.splitn(2, '#').collect();
        (parts[0].to_string(), Some(parts[1].to_string()))
    } else {
        (raw_code, None)
    };

    // Exchange code for tokens (using JSON format like OpenCode)
    let client = reqwest::Client::new();
    let mut body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": claude::CLIENT_ID,
        "code": code,
        "code_verifier": verifier,
        "redirect_uri": claude::REDIRECT_URI,
    });

    // Add state if present in callback
    if let Some(state) = state_from_callback {
        body["state"] = serde_json::Value::String(state);
    }

    eprintln!("Exchanging code for tokens...");
    eprintln!("Request: {}", serde_json::to_string_pretty(&body)?);

    let resp = client
        .post(claude::TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("Token exchange failed: {}", text);
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        id_token: Option<String>,
    }

    let tokens: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

    Ok(OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: tokens.id_token,
    })
}

/// Perform OAuth login for OpenAI/Codex
pub async fn login_openai() -> Result<OAuthTokens> {
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    // Build authorization URL with Codex-specific params
    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=codex_cli_rs",
        openai::AUTHORIZE_URL,
        openai::CLIENT_ID,
        urlencoding::encode(openai::REDIRECT_URI),
        urlencoding::encode(openai::SCOPES),
        challenge,
        state
    );

    eprintln!("\nOpen this URL in your browser:\n");
    eprintln!("{}\n", auth_url);

    // Try to open browser
    let _ = open::that(&auth_url);

    // Wait for callback
    let code = wait_for_callback(9876, &state)?;

    // Exchange code for tokens
    let client = reqwest::Client::new();
    let resp = client
        .post(openai::TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
            openai::CLIENT_ID,
            code,
            verifier,
            urlencoding::encode(openai::REDIRECT_URI)
        ))
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("Token exchange failed: {}", text);
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        id_token: Option<String>,
    }

    let tokens: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

    Ok(OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: tokens.id_token,
    })
}

/// Save Claude tokens to jcode's credentials file (default account).
pub fn save_claude_tokens(tokens: &OAuthTokens) -> Result<()> {
    save_claude_tokens_for_account(tokens, "default")
}

/// Save Claude tokens for a named account.
pub fn save_claude_tokens_for_account(tokens: &OAuthTokens, label: &str) -> Result<()> {
    let account = claude_auth::AnthropicAccount {
        label: label.to_string(),
        access: tokens.access_token.clone(),
        refresh: tokens.refresh_token.clone(),
        expires: tokens.expires_at,
        subscription_type: None,
    };
    claude_auth::upsert_account(account)?;
    Ok(())
}

/// Load Claude tokens from jcode's credentials file (active account).
pub fn load_claude_tokens() -> Result<OAuthTokens> {
    if let Ok(creds) = claude_auth::load_credentials() {
        return Ok(OAuthTokens {
            access_token: creds.access_token,
            refresh_token: creds.refresh_token,
            expires_at: creds.expires_at,
            id_token: None,
        });
    }

    anyhow::bail!("No Claude Max OAuth credentials found. Run 'jcode login --provider claude'.");
}

/// Load Claude tokens for a specific named account.
pub fn load_claude_tokens_for_account(label: &str) -> Result<OAuthTokens> {
    let creds = claude_auth::load_credentials_for_account(label)?;
    Ok(OAuthTokens {
        access_token: creds.access_token,
        refresh_token: creds.refresh_token,
        expires_at: creds.expires_at,
        id_token: None,
    })
}

/// Refresh Claude OAuth tokens
pub async fn refresh_claude_tokens(refresh_token: &str) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(claude::TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": claude::CLIENT_ID,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("Token refresh failed: {}", text);
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
    }

    let tokens: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

    let oauth_tokens = OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: None,
    };

    // Save the refreshed tokens to the active account
    let active_label = claude_auth::active_account_label().unwrap_or_else(|| "default".to_string());
    save_claude_tokens_for_account(&oauth_tokens, &active_label)?;

    Ok(oauth_tokens)
}

/// Refresh Claude OAuth tokens for a specific account.
pub async fn refresh_claude_tokens_for_account(
    refresh_token: &str,
    label: &str,
) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(claude::TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": claude::CLIENT_ID,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("Token refresh failed for account '{}': {}", label, text);
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
    }

    let tokens: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

    let oauth_tokens = OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: None,
    };

    save_claude_tokens_for_account(&oauth_tokens, label)?;

    Ok(oauth_tokens)
}

/// Save OpenAI tokens to auth file
pub fn save_openai_tokens(tokens: &OAuthTokens) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home directory"))?;
    let creds_dir = home.join(".codex");
    std::fs::create_dir_all(&creds_dir)?;

    #[derive(Serialize)]
    struct AuthFile {
        tokens: TokenInfo,
    }

    #[derive(Serialize)]
    struct TokenInfo {
        access_token: String,
        refresh_token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id_token: Option<String>,
        expires_at: i64,
    }

    let auth = AuthFile {
        tokens: TokenInfo {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            id_token: tokens.id_token.clone(),
            expires_at: tokens.expires_at,
        },
    };

    let json = serde_json::to_string_pretty(&auth)?;
    let auth_path = creds_dir.join("auth.json");
    std::fs::write(&auth_path, json)?;
    crate::platform::set_permissions_owner_only(&auth_path)?;

    Ok(())
}

/// Refresh OpenAI/Codex OAuth tokens
pub async fn refresh_openai_tokens(refresh_token: &str) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(openai::TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=refresh_token&client_id={}&refresh_token={}",
            openai::CLIENT_ID,
            urlencoding::encode(refresh_token)
        ))
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("OpenAI token refresh failed: {}", text);
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        id_token: Option<String>,
    }

    let tokens: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

    let oauth_tokens = OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: tokens.id_token,
    };

    save_openai_tokens(&oauth_tokens)?;
    Ok(oauth_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_and_challenge_are_different() {
        let (verifier, challenge) = generate_pkce();
        assert_ne!(verifier, challenge);
        assert_eq!(verifier.len(), 64);
        assert!(!challenge.is_empty());
    }

    #[test]
    fn pkce_challenge_is_base64url() {
        let (_, challenge) = generate_pkce();
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
        assert!(!challenge.contains('='));
    }

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let (verifier, challenge) = generate_pkce();
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let expected = URL_SAFE_NO_PAD.encode(hash);
        assert_eq!(challenge, expected);
    }

    #[test]
    fn pkce_generates_unique_values() {
        let (v1, c1) = generate_pkce();
        let (v2, c2) = generate_pkce();
        assert_ne!(v1, v2);
        assert_ne!(c1, c2);
    }

    #[test]
    fn state_is_random_hex() {
        let state = generate_state();
        assert_eq!(state.len(), 32);
        assert!(state.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn state_generates_unique_values() {
        let s1 = generate_state();
        let s2 = generate_state();
        assert_ne!(s1, s2);
    }

    #[test]
    fn oauth_tokens_serialization_roundtrip() {
        let tokens = OAuthTokens {
            access_token: "at_abc".to_string(),
            refresh_token: "rt_def".to_string(),
            expires_at: 1234567890,
            id_token: Some("idt_ghi".to_string()),
        };
        let json = serde_json::to_string(&tokens).unwrap();
        let parsed: OAuthTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.access_token, "at_abc");
        assert_eq!(parsed.refresh_token, "rt_def");
        assert_eq!(parsed.expires_at, 1234567890);
        assert_eq!(parsed.id_token, Some("idt_ghi".to_string()));
    }

    #[test]
    fn oauth_tokens_without_id_token() {
        let tokens = OAuthTokens {
            access_token: "at".to_string(),
            refresh_token: "rt".to_string(),
            expires_at: 0,
            id_token: None,
        };
        let json = serde_json::to_string(&tokens).unwrap();
        assert!(!json.contains("id_token"));
        let parsed: OAuthTokens = serde_json::from_str(&json).unwrap();
        assert!(parsed.id_token.is_none());
    }

    #[test]
    fn claude_oauth_constants() {
        assert!(!claude::CLIENT_ID.is_empty());
        assert!(claude::AUTHORIZE_URL.starts_with("https://"));
        assert!(claude::TOKEN_URL.starts_with("https://"));
        assert!(claude::REDIRECT_URI.starts_with("https://"));
        assert!(!claude::SCOPES.is_empty());
    }

    #[test]
    fn openai_oauth_constants() {
        assert!(!openai::CLIENT_ID.is_empty());
        assert!(openai::AUTHORIZE_URL.starts_with("https://"));
        assert!(openai::TOKEN_URL.starts_with("https://"));
        assert!(openai::REDIRECT_URI.starts_with("http"));
        assert!(!openai::SCOPES.is_empty());
    }

    #[tokio::test]
    async fn wait_for_callback_async_parses_code() {
        let state = "test_state_abc";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let state_clone = state.to_string();
        let handle = tokio::spawn(async move {
            wait_for_callback_async(port, &state_clone).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream =
            tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
                .await
                .unwrap();
        use tokio::io::AsyncWriteExt;
        stream
            .write_all(
                format!(
                    "GET /callback?code=test_code_123&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
                    state
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test_code_123");
    }

    #[tokio::test]
    async fn wait_for_callback_async_rejects_wrong_state() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = tokio::spawn(async move {
            wait_for_callback_async(port, "expected_state").await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream =
            tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
                .await
                .unwrap();
        use tokio::io::AsyncWriteExt;
        stream
            .write_all(b"GET /callback?code=code123&state=wrong_state HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("State mismatch"));
    }

    #[tokio::test]
    async fn wait_for_callback_async_rejects_missing_code() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = tokio::spawn(async move {
            wait_for_callback_async(port, "state123").await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream =
            tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
                .await
                .unwrap();
        use tokio::io::AsyncWriteExt;
        stream
            .write_all(b"GET /callback?state=state123 HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err());
    }
}
