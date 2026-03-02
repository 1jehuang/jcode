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
    pub const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
    pub const SCOPES: &str = "org:create_api_key user:profile user:inference";
}

/// OpenAI Codex OAuth configuration
pub mod openai {
    pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
    pub const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
    pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
    pub const DEFAULT_PORT: u16 = 1455;
    pub const SCOPES: &str = "openid profile email offline_access";

    pub fn redirect_uri(port: u16) -> String {
        format!("http://localhost:{}/auth/callback", port)
    }
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

    eprintln!("Exchanging code for tokens...");

    exchange_claude_code(&verifier, input, claude::REDIRECT_URI).await
}

/// Parse Claude auth input.
///
/// Accepted formats:
/// - plain code (`abc123`)
/// - URL/query with `code=`
/// - `code#state` (OpenCode-style)
fn parse_claude_code_input(input: &str) -> Result<(String, Option<String>)> {
    let trimmed = input.trim();

    let (raw_code, state_from_query) = if trimmed.contains("code=") {
        let url = url::Url::parse(trimmed)
            .or_else(|_| url::Url::parse(&format!("https://example.com?{}", trimmed)))?;
        let code = url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
            .ok_or_else(|| anyhow::anyhow!("No code found in URL"))?;
        let state = url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.to_string());
        (code, state)
    } else {
        (trimmed.to_string(), None)
    };

    if raw_code.contains('#') {
        let parts: Vec<&str> = raw_code.splitn(2, '#').collect();
        Ok((parts[0].to_string(), Some(parts[1].to_string())))
    } else {
        Ok((raw_code, state_from_query))
    }
}

async fn exchange_claude_code_at_url(
    token_url: &str,
    verifier: &str,
    input: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    let (code, state_from_callback) = parse_claude_code_input(input)?;
    // Anthropic's token endpoint expects `state`; callback mode gives it via query,
    // manual mode can omit it, so we fall back to the verifier value used in auth URL.
    let state = state_from_callback
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(verifier)
        .to_string();

    let client = reqwest::Client::new();
    let params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("client_id", claude::CLIENT_ID.to_string()),
        ("code", code),
        ("code_verifier", verifier.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("state", state),
    ];

    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
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

/// Exchange a Claude authorization code for OAuth tokens.
///
/// `input` can be a plain code, a URL/query containing `code=`, or `code#state`.
pub async fn exchange_claude_code(
    verifier: &str,
    input: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    exchange_claude_code_at_url(claude::TOKEN_URL, verifier, input, redirect_uri).await
}

/// Perform OAuth login for OpenAI/Codex
pub async fn login_openai() -> Result<OAuthTokens> {
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let port = openai::DEFAULT_PORT;
    let redirect_uri = openai::redirect_uri(port);

    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=codex_cli_rs",
        openai::AUTHORIZE_URL,
        openai::CLIENT_ID,
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(openai::SCOPES),
        challenge,
        state
    );

    eprintln!("\nOpen this URL in your browser:\n");
    eprintln!("{}\n", auth_url);

    // Try to open browser
    let _ = open::that(&auth_url);

    // Wait for callback
    let code = wait_for_callback(port, &state)?;

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
            urlencoding::encode(&redirect_uri)
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
        email: None,
        subscription_type: None,
    };
    claude_auth::upsert_account(account)?;
    Ok(())
}

#[derive(Deserialize)]
struct ClaudeProfileResponse {
    #[serde(default)]
    account: ClaudeProfileAccount,
}

#[derive(Deserialize, Default)]
struct ClaudeProfileAccount {
    email: Option<String>,
}

async fn fetch_claude_profile_email_at_url(
    access_token: &str,
    profile_url: &str,
) -> Result<Option<String>> {
    let client = reqwest::Client::new();
    let resp = client
        .get(profile_url)
        .header("Accept", "application/json")
        .header("User-Agent", "claude-cli/1.0.0")
        .header("anthropic-beta", "oauth-2025-04-20,claude-code-20250219")
        .bearer_auth(access_token)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Profile fetch failed ({}): {}", status, body);
    }

    let profile: ClaudeProfileResponse = resp.json().await?;
    Ok(profile.account.email)
}

/// Fetch profile metadata for a Claude account and persist any discovered fields.
pub async fn update_claude_account_profile(
    label: &str,
    access_token: &str,
) -> Result<Option<String>> {
    let email = fetch_claude_profile_email_at_url(access_token, claude::PROFILE_URL).await?;
    claude_auth::update_account_profile(label, email.clone())?;
    Ok(email)
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
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", claude::CLIENT_ID),
    ];
    let resp = client
        .post(claude::TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
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
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", claude::CLIENT_ID),
    ];
    let resp = client
        .post(claude::TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
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

/// Build a Claude token exchange request (extracted for testability).
/// Returns (url, content_type, body_bytes).
#[cfg(test)]
fn build_claude_exchange_request(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    state: Option<&str>,
) -> (String, String, Vec<u8>) {
    let effective_state = state.unwrap_or(verifier);
    let params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("client_id", claude::CLIENT_ID.to_string()),
        ("code", code.to_string()),
        ("code_verifier", verifier.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("state", effective_state.to_string()),
    ];
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params.iter())
        .finish();
    (
        claude::TOKEN_URL.to_string(),
        "application/x-www-form-urlencoded".to_string(),
        body.into_bytes(),
    )
}

/// Build a Claude token refresh request (extracted for testability).
#[cfg(test)]
fn build_claude_refresh_request(refresh_token: &str) -> (String, String, Vec<u8>) {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", claude::CLIENT_ID),
    ];
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params.iter())
        .finish();
    (
        claude::TOKEN_URL.to_string(),
        "application/x-www-form-urlencoded".to_string(),
        body.into_bytes(),
    )
}

/// Build an OpenAI token exchange request (extracted for testability).
#[cfg(test)]
fn build_openai_exchange_request(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> (String, String, Vec<u8>) {
    let body = format!(
        "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
        openai::CLIENT_ID,
        code,
        verifier,
        urlencoding::encode(redirect_uri)
    );
    (
        openai::TOKEN_URL.to_string(),
        "application/x-www-form-urlencoded".to_string(),
        body.into_bytes(),
    )
}

/// Build an OpenAI token refresh request (extracted for testability).
#[cfg(test)]
fn build_openai_refresh_request(refresh_token: &str) -> (String, String, Vec<u8>) {
    let body = format!(
        "grant_type=refresh_token&client_id={}&refresh_token={}",
        openai::CLIENT_ID,
        urlencoding::encode(refresh_token)
    );
    (
        openai::TOKEN_URL.to_string(),
        "application/x-www-form-urlencoded".to_string(),
        body.into_bytes(),
    )
}

/// Exchange an auth code for tokens against a configurable URL.
/// Used by tests with a mock server.
#[cfg(test)]
async fn exchange_code_at_url(
    token_url: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    state: Option<&str>,
) -> Result<OAuthTokens> {
    let effective_state = state.unwrap_or(verifier);
    let params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("client_id", claude::CLIENT_ID.to_string()),
        ("code", code.to_string()),
        ("code_verifier", verifier.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("state", effective_state.to_string()),
    ];

    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
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

/// Refresh tokens against a configurable URL.
/// Used by tests with a mock server.
#[cfg(test)]
async fn refresh_tokens_at_url(token_url: &str, refresh_token: &str) -> Result<OAuthTokens> {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", claude::CLIENT_ID),
    ];

    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
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

    Ok(OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: None,
    })
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
        assert!(claude::PROFILE_URL.starts_with("https://"));
        assert!(claude::REDIRECT_URI.starts_with("https://"));
        assert!(!claude::SCOPES.is_empty());
    }

    #[tokio::test]
    async fn fetch_claude_profile_email_reads_account_email() {
        let body = serde_json::json!({
            "account": {
                "email": "user@example.com"
            }
        })
        .to_string();
        let (port, _handle) = mock_token_server(200, &body).await;

        let url = format!("http://127.0.0.1:{}/api/oauth/profile", port);
        let email = fetch_claude_profile_email_at_url("token", &url)
            .await
            .unwrap();

        assert_eq!(email, Some("user@example.com".to_string()));
    }

    #[tokio::test]
    async fn fetch_claude_profile_email_handles_missing_email() {
        let body = serde_json::json!({
            "account": {}
        })
        .to_string();
        let (port, _handle) = mock_token_server(200, &body).await;

        let url = format!("http://127.0.0.1:{}/api/oauth/profile", port);
        let email = fetch_claude_profile_email_at_url("token", &url)
            .await
            .unwrap();

        assert!(email.is_none());
    }

    #[tokio::test]
    async fn fetch_claude_profile_email_propagates_http_error() {
        let body = serde_json::json!({
            "error": "bad_token"
        })
        .to_string();
        let (port, _handle) = mock_token_server(401, &body).await;

        let url = format!("http://127.0.0.1:{}/api/oauth/profile", port);
        let err = fetch_claude_profile_email_at_url("token", &url)
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("Profile fetch failed"));
    }

    #[test]
    fn openai_oauth_constants() {
        assert!(!openai::CLIENT_ID.is_empty());
        assert!(openai::AUTHORIZE_URL.starts_with("https://"));
        assert!(openai::TOKEN_URL.starts_with("https://"));
        assert!(openai::redirect_uri(openai::DEFAULT_PORT).starts_with("http"));
        assert!(!openai::SCOPES.is_empty());
    }

    #[tokio::test]
    async fn wait_for_callback_async_parses_code() {
        let state = "test_state_abc";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let state_clone = state.to_string();
        let handle = tokio::spawn(async move { wait_for_callback_async(port, &state_clone).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
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

        let handle =
            tokio::spawn(async move { wait_for_callback_async(port, "expected_state").await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        use tokio::io::AsyncWriteExt;
        stream
            .write_all(
                b"GET /callback?code=code123&state=wrong_state HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .await
            .unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("State mismatch"));
    }

    #[tokio::test]
    async fn wait_for_callback_async_rejects_missing_code() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = tokio::spawn(async move { wait_for_callback_async(port, "state123").await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
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

    /// Helper: start a mock HTTP server that captures the request and returns a
    /// configurable response. Returns (port, join_handle).
    /// The join handle resolves to (method, path, headers_map, body_string).
    async fn mock_token_server(
        status: u16,
        response_body: &str,
    ) -> (
        u16,
        tokio::task::JoinHandle<(
            String,
            String,
            std::collections::HashMap<String, String>,
            String,
        )>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let resp_body = response_body.to_string();

        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);

            let mut request_line = String::new();
            reader.read_line(&mut request_line).await.unwrap();
            let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
            let method = parts.get(0).unwrap_or(&"").to_string();
            let path = parts.get(1).unwrap_or(&"").to_string();

            let mut headers = std::collections::HashMap::new();
            let mut content_length: usize = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((key, value)) = trimmed.split_once(':') {
                    let k = key.trim().to_lowercase();
                    let v = value.trim().to_string();
                    if k == "content-length" {
                        content_length = v.parse().unwrap_or(0);
                    }
                    headers.insert(k, v);
                }
            }

            let mut body_bytes = vec![0u8; content_length];
            if content_length > 0 {
                reader.read_exact(&mut body_bytes).await.unwrap();
            }
            let body = String::from_utf8(body_bytes).unwrap_or_default();

            let response = format!(
                "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                status,
                resp_body.len(),
                resp_body
            );
            writer.write_all(response.as_bytes()).await.unwrap();

            (method, path, headers, body)
        });

        (port, handle)
    }

    // ========================
    // REGRESSION: Content-Type must be form-urlencoded, not JSON
    // ========================

    #[test]
    fn claude_exchange_request_uses_form_urlencoded() {
        let (_url, content_type, _body) =
            build_claude_exchange_request("code123", "verifier456", claude::REDIRECT_URI, None);
        assert_eq!(content_type, "application/x-www-form-urlencoded");
        assert_ne!(content_type, "application/json");
    }

    #[test]
    fn claude_exchange_request_body_is_not_json() {
        let (_url, _ct, body) =
            build_claude_exchange_request("code123", "verifier456", claude::REDIRECT_URI, None);
        let body_str = String::from_utf8(body).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&body_str).is_err(),
            "Body must NOT be valid JSON (should be form-urlencoded)"
        );
    }

    #[test]
    fn claude_refresh_request_uses_form_urlencoded() {
        let (_url, content_type, _body) = build_claude_refresh_request("rt_test");
        assert_eq!(content_type, "application/x-www-form-urlencoded");
        assert_ne!(content_type, "application/json");
    }

    #[test]
    fn claude_refresh_request_body_is_not_json() {
        let (_url, _ct, body) = build_claude_refresh_request("rt_test");
        let body_str = String::from_utf8(body).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&body_str).is_err(),
            "Body must NOT be valid JSON (should be form-urlencoded)"
        );
    }

    // ========================
    // Claude exchange request body validation
    // ========================

    #[test]
    fn claude_exchange_request_contains_required_fields() {
        let (_url, _ct, body) = build_claude_exchange_request(
            "auth_code_xyz",
            "verifier_abc",
            "https://example.com/callback",
            None,
        );
        let body_str = String::from_utf8(body).unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body_str.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("grant_type").unwrap(), "authorization_code");
        assert_eq!(pairs.get("client_id").unwrap(), claude::CLIENT_ID);
        assert_eq!(pairs.get("code").unwrap(), "auth_code_xyz");
        assert_eq!(pairs.get("code_verifier").unwrap(), "verifier_abc");
        assert_eq!(
            pairs.get("redirect_uri").unwrap(),
            "https://example.com/callback"
        );
        assert_eq!(pairs.get("state").unwrap(), "verifier_abc");
    }

    #[test]
    fn claude_exchange_request_includes_state_when_present() {
        let (_url, _ct, body) = build_claude_exchange_request(
            "code",
            "verifier",
            claude::REDIRECT_URI,
            Some("state_value"),
        );
        let body_str = String::from_utf8(body).unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body_str.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("state").unwrap(), "state_value");
    }

    #[test]
    fn claude_exchange_request_targets_correct_url() {
        let (url, _ct, _body) = build_claude_exchange_request("c", "v", claude::REDIRECT_URI, None);
        assert_eq!(url, "https://console.anthropic.com/v1/oauth/token");
    }

    // ========================
    // Claude refresh request body validation
    // ========================

    #[test]
    fn claude_refresh_request_contains_required_fields() {
        let (_url, _ct, body) = build_claude_refresh_request("rt_refresh_token_value");
        let body_str = String::from_utf8(body).unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body_str.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("grant_type").unwrap(), "refresh_token");
        assert_eq!(
            pairs.get("refresh_token").unwrap(),
            "rt_refresh_token_value"
        );
        assert_eq!(pairs.get("client_id").unwrap(), claude::CLIENT_ID);
    }

    #[test]
    fn claude_refresh_request_targets_correct_url() {
        let (url, _ct, _body) = build_claude_refresh_request("rt");
        assert_eq!(url, "https://console.anthropic.com/v1/oauth/token");
    }

    // ========================
    // OpenAI exchange request validation
    // ========================

    #[test]
    fn openai_exchange_request_uses_form_urlencoded() {
        let (_url, content_type, _body) = build_openai_exchange_request(
            "code",
            "verifier",
            "http://localhost:1455/auth/callback",
        );
        assert_eq!(content_type, "application/x-www-form-urlencoded");
    }

    #[test]
    fn openai_exchange_request_contains_required_fields() {
        let (_url, _ct, body) = build_openai_exchange_request(
            "oai_code_123",
            "oai_verifier",
            "http://localhost:1455/auth/callback",
        );
        let body_str = String::from_utf8(body).unwrap();
        assert!(body_str.contains("grant_type=authorization_code"));
        assert!(body_str.contains(&format!("client_id={}", openai::CLIENT_ID)));
        assert!(body_str.contains("code=oai_code_123"));
        assert!(body_str.contains("code_verifier=oai_verifier"));
        assert!(body_str.contains("redirect_uri="));
    }

    #[test]
    fn openai_exchange_request_targets_correct_url() {
        let (url, _ct, _body) = build_openai_exchange_request("c", "v", "http://localhost/cb");
        assert_eq!(url, "https://auth.openai.com/oauth/token");
    }

    // ========================
    // OpenAI refresh request validation
    // ========================

    #[test]
    fn openai_refresh_request_uses_form_urlencoded() {
        let (_url, content_type, _body) = build_openai_refresh_request("rt_oai");
        assert_eq!(content_type, "application/x-www-form-urlencoded");
    }

    #[test]
    fn openai_refresh_request_contains_required_fields() {
        let (_url, _ct, body) = build_openai_refresh_request("rt_oai_value");
        let body_str = String::from_utf8(body).unwrap();
        assert!(body_str.contains("grant_type=refresh_token"));
        assert!(body_str.contains(&format!("client_id={}", openai::CLIENT_ID)));
        assert!(body_str.contains("refresh_token=rt_oai_value"));
    }

    #[test]
    fn openai_refresh_request_targets_correct_url() {
        let (url, _ct, _body) = build_openai_refresh_request("rt");
        assert_eq!(url, "https://auth.openai.com/oauth/token");
    }

    // ========================
    // Auth URL construction
    // ========================

    #[test]
    fn claude_auth_url_contains_required_params() {
        let (verifier, challenge) = generate_pkce();
        let auth_url = format!(
            "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            claude::AUTHORIZE_URL,
            claude::CLIENT_ID,
            urlencoding::encode(claude::REDIRECT_URI),
            urlencoding::encode(claude::SCOPES),
            challenge,
            verifier,
        );
        let parsed = url::Url::parse(&auth_url).unwrap();
        let params: std::collections::HashMap<String, String> = parsed
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(params.get("code").unwrap(), "true");
        assert_eq!(params.get("client_id").unwrap(), claude::CLIENT_ID);
        assert_eq!(params.get("response_type").unwrap(), "code");
        assert_eq!(params.get("redirect_uri").unwrap(), claude::REDIRECT_URI);
        assert_eq!(params.get("scope").unwrap(), claude::SCOPES);
        assert_eq!(params.get("code_challenge").unwrap(), &challenge);
        assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(params.get("state").unwrap(), &verifier);
    }

    #[test]
    fn openai_auth_url_contains_required_params() {
        let (verifier, challenge) = generate_pkce();
        let state = generate_state();
        let redirect_uri = openai::redirect_uri(openai::DEFAULT_PORT);
        let auth_url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            openai::AUTHORIZE_URL,
            openai::CLIENT_ID,
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(openai::SCOPES),
            challenge,
            state,
        );
        let parsed = url::Url::parse(&auth_url).unwrap();
        let params: std::collections::HashMap<String, String> = parsed
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(params.get("response_type").unwrap(), "code");
        assert_eq!(params.get("client_id").unwrap(), openai::CLIENT_ID);
        assert_eq!(params.get("redirect_uri").unwrap(), &redirect_uri);
        assert_eq!(params.get("scope").unwrap(), openai::SCOPES);
        assert_eq!(params.get("code_challenge").unwrap(), &challenge);
        assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(params.get("state").unwrap(), &state);
    }

    #[test]
    fn claude_auth_url_with_dynamic_redirect_uri() {
        let (verifier, challenge) = generate_pkce();
        let dynamic_redirect = "http://localhost:34531/callback";
        let auth_url = format!(
            "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            claude::AUTHORIZE_URL,
            claude::CLIENT_ID,
            urlencoding::encode(dynamic_redirect),
            urlencoding::encode(claude::SCOPES),
            challenge,
            verifier,
        );
        let parsed = url::Url::parse(&auth_url).unwrap();
        let params: std::collections::HashMap<String, String> = parsed
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(params.get("redirect_uri").unwrap(), dynamic_redirect);
    }

    // ========================
    // Code parsing (plain code, URL, code#state)
    // ========================

    #[test]
    fn parse_plain_auth_code() {
        let input = "abc123def456";
        let (raw_code, state) = parse_claude_code_input(input).unwrap();
        assert_eq!(raw_code, "abc123def456");
        assert!(state.is_none());
    }

    #[test]
    fn parse_code_from_url() {
        let input = "https://example.com/callback?code=mycode123&state=mystate";
        let (raw_code, state) = parse_claude_code_input(input).unwrap();
        assert_eq!(raw_code, "mycode123");
        assert_eq!(state, Some("mystate".to_string()));
    }

    #[test]
    fn parse_code_from_query_string() {
        let input = "code=mycode456&state=s";
        let (raw_code, state) = parse_claude_code_input(input).unwrap();
        assert_eq!(raw_code, "mycode456");
        assert_eq!(state, Some("s".to_string()));
    }

    #[test]
    fn parse_code_hash_state_format() {
        let raw_code = "authcode789#statevalue";
        let (code, state) = parse_claude_code_input(raw_code).unwrap();
        assert_eq!(code, "authcode789");
        assert_eq!(state, Some("statevalue".to_string()));
    }

    #[test]
    fn parse_code_without_hash() {
        let raw_code = "authcode_no_hash";
        let (code, state) = parse_claude_code_input(raw_code).unwrap();
        assert_eq!(code, "authcode_no_hash");
        assert!(state.is_none());
    }

    #[test]
    fn parse_code_trims_input_whitespace() {
        let input = "   authcode_trim   ";
        let (code, state) = parse_claude_code_input(input).unwrap();
        assert_eq!(code, "authcode_trim");
        assert!(state.is_none());
    }

    #[test]
    fn parse_code_url_with_whitespace_extracts_state() {
        let input = "   https://example.com/callback?code=mycode&state=mystate   ";
        let (code, state) = parse_claude_code_input(input).unwrap();
        assert_eq!(code, "mycode");
        assert_eq!(state, Some("mystate".to_string()));
    }

    // ========================
    // Mock server integration: Claude exchange
    // ========================

    #[tokio::test]
    async fn claude_exchange_mock_server_receives_form_urlencoded() {
        let success_body = serde_json::json!({
            "access_token": "at_mock",
            "refresh_token": "rt_mock",
            "expires_in": 3600,
            "id_token": "idt_mock"
        })
        .to_string();
        let (port, handle) = mock_token_server(200, &success_body).await;

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let result = exchange_code_at_url(&url, "code123", "verifier456", "https://redir", None)
            .await
            .unwrap();

        assert_eq!(result.access_token, "at_mock");
        assert_eq!(result.refresh_token, "rt_mock");
        assert_eq!(result.id_token, Some("idt_mock".to_string()));

        let (method, _path, headers, body) = handle.await.unwrap();
        assert_eq!(method, "POST");
        assert_eq!(
            headers.get("content-type").unwrap(),
            "application/x-www-form-urlencoded"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&body).is_err(),
            "Body must be form-urlencoded, not JSON"
        );
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("grant_type").unwrap(), "authorization_code");
        assert_eq!(pairs.get("code").unwrap(), "code123");
        assert_eq!(pairs.get("code_verifier").unwrap(), "verifier456");
        assert_eq!(pairs.get("state").unwrap(), "verifier456");
    }

    #[tokio::test]
    async fn claude_exchange_mock_server_with_state() {
        let success_body = serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 3600
        })
        .to_string();
        let (port, handle) = mock_token_server(200, &success_body).await;

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let _ = exchange_code_at_url(&url, "c", "v", "https://r", Some("my_state"))
            .await
            .unwrap();

        let (_method, _path, _headers, body) = handle.await.unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("state").unwrap(), "my_state");
    }

    #[tokio::test]
    async fn claude_exchange_uses_state_from_url_query_when_present() {
        let success_body = serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 3600
        })
        .to_string();
        let (port, handle) = mock_token_server(200, &success_body).await;

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let _ = exchange_claude_code_at_url(
            &url,
            "verifier_fallback",
            "https://example.com/callback?code=test_code&state=query_state",
            "https://r",
        )
        .await
        .unwrap();

        let (_method, _path, _headers, body) = handle.await.unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("state").unwrap(), "query_state");
        assert_eq!(pairs.get("code").unwrap(), "test_code");
    }

    #[tokio::test]
    async fn claude_exchange_falls_back_to_verifier_when_input_has_no_state() {
        let success_body = serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 3600
        })
        .to_string();
        let (port, handle) = mock_token_server(200, &success_body).await;

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let _ = exchange_claude_code_at_url(&url, "verifier_only", "plain_code", "https://r")
            .await
            .unwrap();

        let (_method, _path, _headers, body) = handle.await.unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("state").unwrap(), "verifier_only");
        assert_eq!(pairs.get("code").unwrap(), "plain_code");
    }

    #[tokio::test]
    async fn claude_exchange_uses_verifier_when_input_state_is_empty() {
        let success_body = serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 3600
        })
        .to_string();
        let (port, handle) = mock_token_server(200, &success_body).await;

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let _ = exchange_claude_code_at_url(&url, "verifier_only", "plain_code#", "https://r")
            .await
            .unwrap();

        let (_method, _path, _headers, body) = handle.await.unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("state").unwrap(), "verifier_only");
    }

    #[tokio::test]
    async fn claude_exchange_mock_server_error_propagates() {
        let error_body =
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"Invalid"}}"#;
        let (port, _handle) = mock_token_server(400, error_body).await;

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let result = exchange_code_at_url(&url, "c", "v", "https://r", None).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Token exchange failed"));
    }

    // ========================
    // Mock server integration: Claude refresh
    // ========================

    #[tokio::test]
    async fn claude_refresh_mock_server_receives_form_urlencoded() {
        let success_body = serde_json::json!({
            "access_token": "at_refreshed",
            "refresh_token": "rt_refreshed",
            "expires_in": 7200
        })
        .to_string();
        let (port, handle) = mock_token_server(200, &success_body).await;

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let result = refresh_tokens_at_url(&url, "old_refresh_token")
            .await
            .unwrap();

        assert_eq!(result.access_token, "at_refreshed");
        assert_eq!(result.refresh_token, "rt_refreshed");

        let (method, _path, headers, body) = handle.await.unwrap();
        assert_eq!(method, "POST");
        assert_eq!(
            headers.get("content-type").unwrap(),
            "application/x-www-form-urlencoded"
        );
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("grant_type").unwrap(), "refresh_token");
        assert_eq!(pairs.get("refresh_token").unwrap(), "old_refresh_token");
        assert_eq!(pairs.get("client_id").unwrap(), claude::CLIENT_ID);
    }

    #[tokio::test]
    async fn claude_refresh_mock_server_error_propagates() {
        let error_body = r#"{"error":"invalid_grant"}"#;
        let (port, _handle) = mock_token_server(400, error_body).await;

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let result = refresh_tokens_at_url(&url, "expired_token").await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Token refresh failed"));
    }

    // ========================
    // Regression: JSON body must be rejected
    // ========================

    #[tokio::test]
    async fn regression_json_body_rejected_by_strict_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);

            let mut request_line = String::new();
            reader.read_line(&mut request_line).await.unwrap();

            let mut content_type = String::new();
            let mut content_length: usize = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((k, v)) = trimmed.split_once(':') {
                    let k = k.trim().to_lowercase();
                    if k == "content-type" {
                        content_type = v.trim().to_string();
                    }
                    if k == "content-length" {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; content_length];
            if content_length > 0 {
                reader.read_exact(&mut body).await.unwrap();
            }

            if content_type.contains("application/json") {
                let error_resp = r#"{"type":"error","error":{"type":"invalid_request_error","message":"Invalid request format"}}"#;
                let response = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    error_resp.len(),
                    error_resp
                );
                writer.write_all(response.as_bytes()).await.unwrap();
                return false;
            }

            let success = serde_json::json!({
                "access_token": "at",
                "refresh_token": "rt",
                "expires_in": 3600
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                success.len(),
                success
            );
            writer.write_all(response.as_bytes()).await.unwrap();
            true
        });

        let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
        let result = exchange_code_at_url(&url, "code", "verifier", "https://redir", None).await;

        let server_accepted = handle.await.unwrap();
        assert!(
            server_accepted,
            "Server should have accepted the form-urlencoded request"
        );
        assert!(
            result.is_ok(),
            "Exchange should succeed with form-urlencoded"
        );
    }

    // ========================
    // Token response parsing
    // ========================

    #[tokio::test]
    async fn exchange_parses_optional_id_token() {
        let body_with = serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 3600,
            "id_token": "idt_value"
        })
        .to_string();
        let (port, _handle) = mock_token_server(200, &body_with).await;
        let url = format!("http://127.0.0.1:{}/token", port);
        let result = exchange_code_at_url(&url, "c", "v", "r", None)
            .await
            .unwrap();
        assert_eq!(result.id_token, Some("idt_value".to_string()));
    }

    #[tokio::test]
    async fn exchange_handles_missing_id_token() {
        let body_without = serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 3600
        })
        .to_string();
        let (port, _handle) = mock_token_server(200, &body_without).await;
        let url = format!("http://127.0.0.1:{}/token", port);
        let result = exchange_code_at_url(&url, "c", "v", "r", None)
            .await
            .unwrap();
        assert!(result.id_token.is_none());
    }

    #[tokio::test]
    async fn exchange_sets_expires_at_in_future() {
        let body = serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 3600
        })
        .to_string();
        let (port, _handle) = mock_token_server(200, &body).await;
        let url = format!("http://127.0.0.1:{}/token", port);
        let before = chrono::Utc::now().timestamp_millis();
        let result = exchange_code_at_url(&url, "c", "v", "r", None)
            .await
            .unwrap();
        let after = chrono::Utc::now().timestamp_millis();
        assert!(result.expires_at >= before + 3600 * 1000);
        assert!(result.expires_at <= after + 3600 * 1000);
    }

    // ========================
    // Special characters / URL encoding
    // ========================

    #[test]
    fn claude_exchange_handles_special_chars_in_code() {
        let (_url, _ct, body) = build_claude_exchange_request(
            "code+with/special=chars&more",
            "verifier",
            claude::REDIRECT_URI,
            None,
        );
        let body_str = String::from_utf8(body).unwrap();
        let pairs: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(body_str.as_bytes())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(pairs.get("code").unwrap(), "code+with/special=chars&more");
    }

    #[test]
    fn openai_redirect_uri_format() {
        let uri = openai::redirect_uri(1455);
        assert_eq!(uri, "http://localhost:1455/auth/callback");
        let uri2 = openai::redirect_uri(9999);
        assert_eq!(uri2, "http://localhost:9999/auth/callback");
    }

    // ========================
    // All providers use form-urlencoded (comprehensive check)
    // ========================

    #[test]
    fn all_token_requests_use_form_urlencoded_not_json() {
        let checks: Vec<(&str, String)> = vec![
            (
                "claude_exchange",
                build_claude_exchange_request("c", "v", "r", None).1,
            ),
            (
                "claude_exchange_with_state",
                build_claude_exchange_request("c", "v", "r", Some("s")).1,
            ),
            ("claude_refresh", build_claude_refresh_request("rt").1),
            (
                "openai_exchange",
                build_openai_exchange_request("c", "v", "r").1,
            ),
            ("openai_refresh", build_openai_refresh_request("rt").1),
        ];
        for (name, ct) in checks {
            assert_eq!(
                ct, "application/x-www-form-urlencoded",
                "{} must use application/x-www-form-urlencoded, got {}",
                name, ct
            );
        }
    }
}
