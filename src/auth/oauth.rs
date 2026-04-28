use crate::auth::claude as claude_auth;
use anyhow::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::net::TcpListener;
use std::time::Duration;

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
    pub const CALLBACK_PATH: &str = "/auth/callback";
    pub const SCOPES: &str =
        "openid profile email offline_access api.connectors.read api.connectors.invoke";

    pub fn redirect_uri(port: u16) -> String {
        format!("http://localhost:{}{}", port, CALLBACK_PATH)
    }

    pub fn default_redirect_uri() -> String {
        redirect_uri(DEFAULT_PORT)
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

pub fn generate_pkce_public() -> (String, String) {
    generate_pkce()
}

pub fn generate_state_public() -> String {
    generate_state()
}

const CALLBACK_READ_TIMEOUT_SECS: u64 = 5;

fn bad_request_response(message: &str) -> String {
    let body = format!(
        "<html><body><h1>Authentication not completed</h1><p>{}</p><p>You can close this tab and return to jcode.</p></body></html>",
        message
    );
    format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

fn is_socket_read_timeout(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

fn read_http_request_line_blocking<R: BufRead>(reader: &mut R) -> Result<Option<String>> {
    let mut request_line = String::new();
    match reader.read_line(&mut request_line) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(request_line)),
        Err(err) if is_socket_read_timeout(&err) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn drain_http_headers_blocking<R: BufRead>(reader: &mut R) -> Result<bool> {
    let mut header_line = String::new();
    loop {
        header_line.clear();
        match reader.read_line(&mut header_line) {
            Ok(0) => return Ok(false),
            Ok(_) if header_line.trim().is_empty() => return Ok(true),
            Ok(_) => {}
            Err(err) if is_socket_read_timeout(&err) => return Ok(false),
            Err(err) => return Err(err.into()),
        }
    }
}

async fn read_http_request_line_async<R>(
    reader: &mut tokio::io::BufReader<R>,
) -> Result<Option<String>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut request_line = String::new();
    match tokio::time::timeout(
        Duration::from_secs(CALLBACK_READ_TIMEOUT_SECS),
        tokio::io::AsyncBufReadExt::read_line(reader, &mut request_line),
    )
    .await
    {
        Ok(Ok(0)) => Ok(None),
        Ok(Ok(_)) => Ok(Some(request_line)),
        Ok(Err(err)) => Err(err.into()),
        Err(_) => Ok(None),
    }
}

async fn drain_http_headers_async<R>(reader: &mut tokio::io::BufReader<R>) -> Result<bool>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut header_line = String::new();
    loop {
        header_line.clear();
        match tokio::time::timeout(
            Duration::from_secs(CALLBACK_READ_TIMEOUT_SECS),
            tokio::io::AsyncBufReadExt::read_line(reader, &mut header_line),
        )
        .await
        {
            Ok(Ok(0)) => return Ok(false),
            Ok(Ok(_)) if header_line.trim().is_empty() => return Ok(true),
            Ok(Ok(_)) => {}
            Ok(Err(err)) => return Err(err.into()),
            Err(_) => return Ok(false),
        }
    }
}

/// Start local server and wait for OAuth callback
pub fn wait_for_callback(port: u16, expected_state: &str) -> Result<String> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
    eprintln!("Waiting for OAuth callback on port {}...", port);

    loop {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(CALLBACK_READ_TIMEOUT_SECS)))?;
        let mut reader = BufReader::new(&stream);
        let Some(request_line) = read_http_request_line_blocking(&mut reader)? else {
            continue;
        };
        if !drain_http_headers_blocking(&mut reader)? {
            continue;
        }

        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            let _ = stream.write_all(bad_request_response("Invalid HTTP request.").as_bytes());
            continue;
        }

        let path = parts[1];
        let url = match url::Url::parse(&format!("http://localhost{}", path)) {
            Ok(url) => url,
            Err(_) => {
                let _ = stream.write_all(
                    bad_request_response("Could not parse OAuth callback URL.").as_bytes(),
                );
                continue;
            }
        };

        if let Some(error) = url
            .query_pairs()
            .find(|(k, _)| k == "error")
            .map(|(_, v)| v.to_string())
        {
            let _ = stream.write_all(
                bad_request_response("Authentication was denied or cancelled.").as_bytes(),
            );
            anyhow::bail!("OAuth provider returned error: {}", error);
        }

        let code = match url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
        {
            Some(code) => code,
            None => {
                let _ = stream.write_all(
                    bad_request_response("No authorization code was included in this request.")
                        .as_bytes(),
                );
                continue;
            }
        };

        let state = match url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.to_string())
        {
            Some(state) => state,
            None => {
                let _ = stream.write_all(
                    bad_request_response("No OAuth state was included in this request.").as_bytes(),
                );
                continue;
            }
        };

        if state != expected_state {
            let _ = stream.write_all(
                bad_request_response("OAuth state mismatch. Please retry the latest login flow.")
                    .as_bytes(),
            );
            continue;
        }

        let body = "<html><body><h1>Success!</h1><p>You can close this window.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes())?;

        return Ok(code);
    }
}

/// Async version of wait_for_callback using tokio (for use from TUI context)
pub async fn wait_for_callback_async(port: u16, expected_state: &str) -> Result<String> {
    let listener = bind_callback_listener(port)?;
    wait_for_callback_async_on_listener(listener, expected_state).await
}

pub fn bind_callback_listener(port: u16) -> Result<tokio::net::TcpListener> {
    let std_listener = std::net::TcpListener::bind(format!("127.0.0.1:{port}"))?;
    std_listener.set_nonblocking(true)?;
    Ok(tokio::net::TcpListener::from_std(std_listener)?)
}

pub async fn wait_for_callback_async_on_listener(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String> {
    let expected_state = expected_state.to_string();

    use tokio::io::{AsyncWriteExt, BufReader};

    loop {
        let (stream, _) = listener.accept().await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let Some(request_line) = read_http_request_line_async(&mut reader).await? else {
            continue;
        };
        if !drain_http_headers_async(&mut reader).await? {
            continue;
        }

        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            let _ = writer
                .write_all(bad_request_response("Invalid HTTP request.").as_bytes())
                .await;
            continue;
        }

        let path = parts[1];
        let url = match url::Url::parse(&format!("http://localhost{}", path)) {
            Ok(url) => url,
            Err(_) => {
                let _ = writer
                    .write_all(
                        bad_request_response("Could not parse OAuth callback URL.").as_bytes(),
                    )
                    .await;
                continue;
            }
        };

        if let Some(error) = url
            .query_pairs()
            .find(|(k, _)| k == "error")
            .map(|(_, v)| v.to_string())
        {
            let _ = writer
                .write_all(
                    bad_request_response("Authentication was denied or cancelled.").as_bytes(),
                )
                .await;
            anyhow::bail!("OAuth provider returned error: {}", error);
        }

        let code = match url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
        {
            Some(code) => code,
            None => {
                let _ = writer
                    .write_all(
                        bad_request_response("No authorization code was included in this request.")
                            .as_bytes(),
                    )
                    .await;
                continue;
            }
        };

        let state = match url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.to_string())
        {
            Some(state) => state,
            None => {
                let _ = writer
                    .write_all(
                        bad_request_response("No OAuth state was included in this request.")
                            .as_bytes(),
                    )
                    .await;
                continue;
            }
        };

        if state != expected_state {
            let _ = writer
                .write_all(
                    bad_request_response(
                        "OAuth state mismatch. Please retry the latest login flow.",
                    )
                    .as_bytes(),
                )
                .await;
            continue;
        }

        let body = "<html><body><h1>Success!</h1><p>You can close this window and return to jcode.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        writer.write_all(response.as_bytes()).await?;

        return Ok(code);
    }
}

/// Perform OAuth login for Claude
pub async fn login_claude(no_browser: bool) -> Result<OAuthTokens> {
    let (verifier, challenge) = generate_pkce();
    if let Ok(code) = std::env::var("JCODE_CLAUDE_AUTH_CODE") {
        let trimmed = code.trim();
        if trimmed.is_empty() {
            anyhow::bail!("JCODE_CLAUDE_AUTH_CODE is set but empty");
        }
        eprintln!("Exchanging code for tokens...");
        return exchange_claude_code(&verifier, trimmed, claude::REDIRECT_URI).await;
    }

    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "Claude login needs an authorization code from stdin. Re-run in an interactive terminal, or set JCODE_CLAUDE_AUTH_CODE."
        );
    }

    // Try local callback first for a fully automatic flow.
    if let Ok(listener) = bind_callback_listener(0) {
        let port = listener.local_addr()?.port();

        let redirect_uri = format!("http://localhost:{}/callback", port);
        let auth_url = claude_auth_url(&redirect_uri, &challenge, &verifier);
        let manual_auth_url = claude_auth_url(claude::REDIRECT_URI, &challenge, &verifier);

        eprintln!("\nOpen this URL in your browser:\n");
        eprintln!("{}\n", auth_url);
        if let Some(qr) = crate::login_qr::indented_section(
            &manual_auth_url,
            "No browser on this machine? Scan this QR on another device, finish login there, then paste the full callback URL back here:",
            "    ",
        ) {
            eprintln!("{qr}\n");
        }
        eprintln!("Opening browser for Claude login...\n");
        let browser_opened = if crate::auth::browser_suppressed(no_browser) {
            false
        } else {
            open::that(&auth_url).is_ok()
        };
        if browser_opened {
            eprintln!(
                "Waiting up to 120s for automatic callback on {}",
                redirect_uri
            );
        } else {
            eprintln!(
                "Couldn't open a browser on this machine. Use the QR code or manual URL above, then paste the callback URL here.\n"
            );
        }

        if browser_opened {
            match tokio::time::timeout(
                std::time::Duration::from_secs(120),
                wait_for_callback_async_on_listener(listener, &verifier),
            )
            .await
            {
                Ok(Ok(code)) => {
                    eprintln!("Received callback. Exchanging code for tokens...");
                    return exchange_claude_code(&verifier, &code, &redirect_uri).await;
                }
                Ok(Err(err)) => {
                    eprintln!(
                        "Automatic callback failed ({err}). Falling back to manual code paste."
                    );
                }
                Err(_) => {
                    eprintln!("Timed out waiting for callback. Falling back to manual code paste.");
                }
            }
        }

        eprintln!("Paste the authorization code (or callback URL) here:\n");
        eprint!("> ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            anyhow::bail!("No authorization code entered.");
        }
        eprintln!("Exchanging code for tokens...");
        let selected_redirect_uri = claude_redirect_uri_for_input(trimmed, &redirect_uri);
        return exchange_claude_code(&verifier, trimmed, &selected_redirect_uri).await;
    }

    // Last-resort manual flow if localhost callback binding is unavailable.
    let auth_url = claude_auth_url(claude::REDIRECT_URI, &challenge, &verifier);

    eprintln!("\nOpen this URL in your browser:\n");
    eprintln!("{}\n", auth_url);
    if let Some(qr) = crate::login_qr::indented_section(
        &auth_url,
        "Scan this QR on another device if this machine has no browser:",
        "    ",
    ) {
        eprintln!("{qr}\n");
    }
    eprintln!("Opening browser for Claude login...\n");
    if !crate::auth::browser_suppressed(no_browser) {
        let _ = open::that(&auth_url);
    }
    eprintln!("After logging in, copy and paste the callback URL or code here:\n");
    eprint!("> ");
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("No authorization code entered.");
    }

    eprintln!("Exchanging code for tokens...");
    exchange_claude_code(&verifier, trimmed, claude::REDIRECT_URI).await
}

pub fn claude_auth_url(redirect_uri: &str, challenge: &str, state: &str) -> String {
    format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        claude::AUTHORIZE_URL,
        claude::CLIENT_ID,
        urlencoding::encode(redirect_uri),
        urlencoding::encode(claude::SCOPES),
        challenge,
        state
    )
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

    let (code, state) = if raw_code.contains('#') {
        let parts: Vec<&str> = raw_code.splitn(2, '#').collect();
        (parts[0].to_string(), Some(parts[1].to_string()))
    } else {
        (raw_code, state_from_query)
    };

    if code.trim().is_empty() {
        anyhow::bail!("No authorization code provided");
    }

    Ok((code, state))
}

pub fn claude_redirect_uri_for_input(input: &str, fallback_redirect_uri: &str) -> String {
    let trimmed = input.trim();
    let Ok(url) = url::Url::parse(trimmed) else {
        return fallback_redirect_uri.to_string();
    };

    let Ok(expected_manual) = url::Url::parse(claude::REDIRECT_URI) else {
        return fallback_redirect_uri.to_string();
    };

    if url.scheme() == expected_manual.scheme()
        && url.host_str() == expected_manual.host_str()
        && url.path() == expected_manual.path()
    {
        claude::REDIRECT_URI.to_string()
    } else {
        fallback_redirect_uri.to_string()
    }
}

pub fn parse_callback_input_with_state(input: &str) -> Result<(String, String)> {
    let (code, state) = parse_claude_code_input(input)?;
    let state = state
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Please paste the full callback URL or query string so jcode can verify the login state."
            )
        })?;
    Ok((code, state))
}

async fn exchange_claude_code_at_url(
    token_url: &str,
    verifier: &str,
    input: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    let (code, state_from_callback) = parse_claude_code_input(input)?;
    // Anthropic's token endpoint expects `state`.
    // We bind state to the PKCE verifier in the auth URL; if callback input
    // includes a non-empty state, it must match to avoid CSRF or stale-code mixups.
    let state = match state_from_callback.as_deref().filter(|s| !s.is_empty()) {
        Some(callback_state) if callback_state != verifier => {
            anyhow::bail!(
                "OAuth state mismatch. Start login again and use the latest callback/code."
            )
        }
        Some(callback_state) => callback_state.to_string(),
        None => verifier.to_string(),
    };

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

pub fn openai_auth_url(redirect_uri: &str, challenge: &str, state: &str) -> String {
    openai_auth_url_with_prompt(redirect_uri, challenge, state, None)
}

pub fn openai_auth_url_with_prompt(
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    prompt: Option<&str>,
) -> String {
    let prompt_param = prompt
        .map(|p| format!("&prompt={}", urlencoding::encode(p)))
        .unwrap_or_default();
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=codex_cli_rs{}",
        openai::AUTHORIZE_URL,
        openai::CLIENT_ID,
        urlencoding::encode(redirect_uri),
        urlencoding::encode(openai::SCOPES),
        challenge,
        state,
        prompt_param
    )
}

pub fn callback_listener_available(port: u16) -> bool {
    std::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .map(|listener| {
            drop(listener);
            true
        })
        .unwrap_or(false)
}

async fn exchange_openai_code_at_url(
    token_url: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
            openai::CLIENT_ID,
            code,
            verifier,
            urlencoding::encode(redirect_uri)
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

pub async fn exchange_openai_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    exchange_openai_code_at_url(openai::TOKEN_URL, code, verifier, redirect_uri).await
}

pub async fn exchange_openai_callback_input(
    verifier: &str,
    input: &str,
    expected_state: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    let (code, callback_state) = parse_callback_input_with_state(input)?;
    if callback_state != expected_state {
        anyhow::bail!("OAuth state mismatch. Start login again and use the latest callback URL.");
    }
    exchange_openai_code(&code, verifier, redirect_uri).await
}

/// Perform OAuth login for OpenAI/Codex
pub async fn login_openai(no_browser: bool) -> Result<OAuthTokens> {
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let port = openai::DEFAULT_PORT;
    let redirect_uri = openai::redirect_uri(port);
    let auth_url = openai_auth_url_with_prompt(&redirect_uri, &challenge, &state, Some("login"));

    eprintln!("\nOpen this URL in your browser:\n");
    eprintln!("{}\n", auth_url);
    if let Some(qr) = crate::login_qr::indented_section(
        &auth_url,
        "Scan this QR on another device if this machine has no browser:",
        "    ",
    ) {
        eprintln!("{qr}\n");
    }

    let callback_listener = bind_callback_listener(port).ok();
    let browser_opened = if crate::auth::browser_suppressed(no_browser) {
        false
    } else {
        open::that(&auth_url).is_ok()
    };

    if browser_opened {
        if let Some(listener) = callback_listener {
            eprintln!(
                "Waiting up to 300s for automatic callback on {}",
                redirect_uri
            );
            match tokio::time::timeout(
                std::time::Duration::from_secs(300),
                wait_for_callback_async_on_listener(listener, &state),
            )
            .await
            {
                Ok(Ok(code)) => return exchange_openai_code(&code, &verifier, &redirect_uri).await,
                Ok(Err(err)) => {
                    eprintln!("Automatic callback failed ({err}). Falling back to manual paste.");
                }
                Err(_) => {
                    eprintln!("Timed out waiting for callback. Falling back to manual paste.");
                }
            }
        } else {
            eprintln!(
                "Local callback port {} is unavailable. Finish login in any browser, then paste the full callback URL here.\n",
                port
            );
        }
    } else if !browser_opened {
        eprintln!(
            "Couldn't open a browser on this machine. Use the QR code above, then paste the full callback URL here.\n"
        );
    }

    eprintln!("Paste the full callback URL (or query string) here:\n");
    eprint!("> ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("No callback URL entered.");
    }
    exchange_openai_callback_input(&verifier, trimmed, &state, &redirect_uri).await
}

/// Save Claude tokens to jcode's credentials file (active account or first numbered account).
pub fn save_claude_tokens(tokens: &OAuthTokens) -> Result<()> {
    let label = claude_auth::login_target_label(None)?;
    save_claude_tokens_for_account(tokens, &label)
}

/// Save Claude tokens for a specific stored account label.
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
        let body = crate::util::http_error_body(resp, "HTTP error").await;
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

/// Load Claude tokens for a specific stored account label.
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
    let result: Result<OAuthTokens> = async {
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

        let active_label =
            claude_auth::active_account_label().unwrap_or_else(claude_auth::primary_account_label);
        save_claude_tokens_for_account(&oauth_tokens, &active_label)?;

        Ok(oauth_tokens)
    }
    .await;

    match &result {
        Ok(_) => {
            let _ = crate::auth::refresh_state::record_success("claude");
        }
        Err(err) => {
            let _ = crate::auth::refresh_state::record_failure("claude", err.to_string());
        }
    }

    result
}

/// Refresh Claude OAuth tokens for a specific account.
pub async fn refresh_claude_tokens_for_account(
    refresh_token: &str,
    label: &str,
) -> Result<OAuthTokens> {
    let result: Result<OAuthTokens> = async {
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
    .await;

    match &result {
        Ok(_) => {
            let _ = crate::auth::refresh_state::record_success("claude");
        }
        Err(err) => {
            let _ = crate::auth::refresh_state::record_failure("claude", err.to_string());
        }
    }

    result
}

/// Save OpenAI tokens to auth file
pub fn save_openai_tokens(tokens: &OAuthTokens) -> Result<()> {
    let label = crate::auth::codex::login_target_label(None)?;
    save_openai_tokens_for_account(tokens, &label)
}

/// Save OpenAI tokens for a specific stored account label.
pub fn save_openai_tokens_for_account(tokens: &OAuthTokens, label: &str) -> Result<()> {
    crate::auth::codex::upsert_account_from_tokens(
        label,
        &tokens.access_token,
        &tokens.refresh_token,
        tokens.id_token.clone(),
        Some(tokens.expires_at),
    )?;
    Ok(())
}

/// Refresh OpenAI/Codex OAuth tokens
pub async fn refresh_openai_tokens(refresh_token: &str) -> Result<OAuthTokens> {
    let active_label = crate::auth::codex::active_account_label();
    refresh_openai_tokens_inner(refresh_token, active_label.as_deref()).await
}

/// Refresh OpenAI/Codex OAuth tokens for a specific stored account label.
pub async fn refresh_openai_tokens_for_account(
    refresh_token: &str,
    label: &str,
) -> Result<OAuthTokens> {
    refresh_openai_tokens_inner(refresh_token, Some(label)).await
}

async fn refresh_openai_tokens_inner(
    refresh_token: &str,
    label: Option<&str>,
) -> Result<OAuthTokens> {
    let result: Result<OAuthTokens> = async {
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

        if let Some(label) = label {
            save_openai_tokens_for_account(&oauth_tokens, label)?;
        } else {
            crate::logging::info(
                "Refreshed OpenAI/Codex tokens from an external source without storing them in jcode auth",
            );
        }
        Ok(oauth_tokens)
    }
    .await;

    match &result {
        Ok(_) => {
            let _ = crate::auth::refresh_state::record_success("openai");
        }
        Err(err) => {
            let _ = crate::auth::refresh_state::record_failure("openai", err.to_string());
        }
    }

    result
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
    let params = [
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
#[path = "oauth_tests/mod.rs"]
mod tests;
