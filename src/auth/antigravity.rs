use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, IsTerminal, Write};

const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v1/userinfo?alt=json";
pub const DEFAULT_PORT: u16 = 51121;
const LOOPBACK_HOST: &str = "127.0.0.1";
const REDIRECT_PATH: &str = "/oauth-callback";
// OAuth credentials from Google's Antigravity desktop app.
// These are for a desktop OAuth client where the client secret is safe to embed.
// Env vars remain available as optional overrides.
// gitleaks:allow - public desktop OAuth credentials, safe to embed
const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com"; // gitleaks:allow
const ANTIGRAVITY_CLIENT_SECRET: &str = "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"; // gitleaks:allow
const CLIENT_ID_ENV: &str = "JCODE_ANTIGRAVITY_CLIENT_ID";
const CLIENT_SECRET_ENV: &str = "JCODE_ANTIGRAVITY_CLIENT_SECRET";
const VERSION_ENV: &str = "JCODE_ANTIGRAVITY_VERSION";
const ANTIGRAVITY_VERSION: &str = "1.18.3";
const ANTIGRAVITY_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];
const LOAD_ENDPOINTS: &[&str] = &[
    "https://cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
    "https://autopush-cloudcode-pa.sandbox.googleapis.com",
];
const GOOGLE_OAUTH_USER_AGENT: &str = "google-api-nodejs-client/9.15.1";

fn antigravity_client_id() -> String {
    std::env::var(CLIENT_ID_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ANTIGRAVITY_CLIENT_ID.to_string())
}

fn antigravity_client_secret() -> String {
    std::env::var(CLIENT_SECRET_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ANTIGRAVITY_CLIENT_SECRET.to_string())
}

fn antigravity_version() -> String {
    std::env::var(VERSION_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ANTIGRAVITY_VERSION.to_string())
}

fn metadata_platform() -> &'static str {
    // The Cloud Code backend currently rejects OS-specific string enum values
    // such as MACOS, WINDOWS, and LINUX for ClientMetadata.Platform. Use the
    // string value that is accepted across platforms instead of varying by OS.
    "PLATFORM_UNSPECIFIED"
}

fn user_agent() -> String {
    if cfg!(target_os = "windows") {
        format!("antigravity/{} windows/amd64", antigravity_version())
    } else if cfg!(target_arch = "aarch64") {
        format!("antigravity/{} darwin/arm64", antigravity_version())
    } else {
        format!("antigravity/{} darwin/amd64", antigravity_version())
    }
}

fn client_metadata_header() -> String {
    format!(
        "{{\"ideType\":\"ANTIGRAVITY\",\"platform\":\"{}\",\"pluginType\":\"GEMINI\"}}",
        metadata_platform()
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

impl AntigravityTokens {
    pub fn is_expired(&self) -> bool {
        super::google_oauth::token_is_expired(self.expires_at)
    }
}

#[derive(Debug, Deserialize)]
struct GoogleUserInfo {
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssistResponse {
    #[serde(default)]
    cloudaicompanion_project: Option<serde_json::Value>,
}

pub fn tokens_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("antigravity_oauth.json"))
}

pub fn load_tokens() -> Result<AntigravityTokens> {
    let path = tokens_path()?;
    if path.exists() {
        crate::storage::harden_secret_file_permissions(&path);
        return crate::storage::read_json(&path).map_err(|_| {
            anyhow::anyhow!(
                "No Antigravity tokens found. Run `jcode login --provider antigravity`."
            )
        });
    }

    if let Some(tokens) = crate::auth::external::load_antigravity_oauth_tokens() {
        return Ok(AntigravityTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_at,
            email: None,
            project_id: None,
        });
    }

    anyhow::bail!("No Antigravity tokens found. Run `jcode login --provider antigravity`.");
}

pub fn save_tokens(tokens: &AntigravityTokens) -> Result<()> {
    let path = tokens_path()?;
    crate::storage::write_json_secret(&path, tokens)
}

pub fn has_cached_auth() -> bool {
    load_tokens().is_ok()
}

pub async fn load_or_refresh_tokens() -> Result<AntigravityTokens> {
    let tokens = load_tokens()?;
    if tokens.is_expired() {
        refresh_tokens(&tokens).await
    } else {
        Ok(tokens)
    }
}

pub async fn refresh_tokens(tokens: &AntigravityTokens) -> Result<AntigravityTokens> {
    let result: Result<AntigravityTokens> = async {
        let client = reqwest::Client::new();
        let client_id = antigravity_client_id();
        let client_secret = antigravity_client_secret();
        let resp = client
            .post(GOOGLE_TOKEN_URL)
            .header(reqwest::header::USER_AGENT, GOOGLE_OAUTH_USER_AGENT)
            .form(&vec![
                ("grant_type", "refresh_token".to_string()),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", tokens.refresh_token.clone()),
            ])
            .send()
            .await
            .context("Failed to refresh Antigravity OAuth token")?;

        if !resp.status().is_success() {
            let body = crate::util::http_error_body(resp, "HTTP error").await;
            anyhow::bail!("Antigravity token refresh failed: {}", body.trim());
        }

        let token_resp: super::google_oauth::TokenResponse = resp
            .json()
            .await
            .context("Failed to parse Antigravity refresh response")?;

        let mut refreshed = AntigravityTokens {
            access_token: token_resp.access_token,
            refresh_token: token_resp
                .refresh_token
                .unwrap_or_else(|| tokens.refresh_token.clone()),
            expires_at: super::google_oauth::expires_at_from_now(token_resp.expires_in),
            email: tokens.email.clone(),
            project_id: tokens.project_id.clone(),
        };

        if refreshed.email.is_none() {
            refreshed.email = fetch_email(&refreshed.access_token).await.ok();
        }
        if refreshed.project_id.is_none() {
            refreshed.project_id = fetch_project_id(&refreshed.access_token).await.ok();
        }

        save_tokens(&refreshed)?;
        Ok(refreshed)
    }
    .await;

    match &result {
        Ok(_) => {
            let _ = crate::auth::refresh_state::record_success("antigravity");
        }
        Err(err) => {
            let _ = crate::auth::refresh_state::record_failure("antigravity", err.to_string());
        }
    }

    result
}

pub async fn login(no_browser: bool) -> Result<AntigravityTokens> {
    let (verifier, challenge) = crate::auth::oauth::generate_pkce_public();
    let state = crate::auth::oauth::generate_state_public();
    let redirect_uri = redirect_uri(DEFAULT_PORT);
    let auth_url = build_auth_url(&redirect_uri, &challenge, &state)?;

    if !crate::auth::browser_suppressed(no_browser)
        && let Ok(listener) = crate::auth::oauth::bind_callback_listener(DEFAULT_PORT)
    {
        eprintln!("\nOpening browser for Antigravity login...\n");
        eprintln!("If the browser didn't open, visit:\n{}\n", auth_url);
        if let Some(qr) = crate::login_qr::indented_section(
            &auth_url,
            "Scan this QR on another device if this machine has no browser:",
            "    ",
        ) {
            eprintln!("{qr}\n");
        }

        let browser_opened = open::that(&auth_url).is_ok();
        if browser_opened {
            eprintln!(
                "Waiting up to 300s for automatic callback on {}",
                redirect_uri
            );
            eprintln!(
                "If the browser lands on a loopback error page instead of returning to jcode, copy the full URL from the address bar and re-run with `--no-browser` to paste it manually."
            );
            match tokio::time::timeout(
                std::time::Duration::from_secs(300),
                crate::auth::oauth::wait_for_callback_async_on_listener(listener, &state),
            )
            .await
            {
                Ok(Ok(code)) => {
                    return exchange_callback_code(&code, &verifier, &redirect_uri).await;
                }
                Ok(Err(err)) => {
                    eprintln!(
                        "Automatic callback failed ({err}). Falling back to manual callback paste."
                    );
                }
                Err(_) => {
                    eprintln!(
                        "Timed out waiting for callback. Falling back to manual callback paste."
                    );
                }
            }
        } else {
            eprintln!(
                "Couldn't open a browser on this machine. Falling back to manual callback paste.\n"
            );
        }
    }

    manual_login(&verifier, &state, &redirect_uri, &auth_url, no_browser).await
}

async fn manual_login(
    verifier: &str,
    expected_state: &str,
    redirect_uri: &str,
    auth_url: &str,
    no_browser: bool,
) -> Result<AntigravityTokens> {
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "Antigravity login needs an interactive terminal for manual callback entry. Re-run in an interactive terminal."
        );
    }

    eprintln!("\nManual Antigravity auth required.\n");
    eprintln!("Open this URL in your browser:\n\n{}\n", auth_url);
    if let Some(qr) = crate::login_qr::indented_section(
        auth_url,
        "Scan this QR on another device if needed:",
        "    ",
    ) {
        eprintln!("{qr}\n");
    }
    if !crate::auth::browser_suppressed(no_browser) {
        let _ = open::that(auth_url);
    }
    eprintln!(
        "After approving access, paste the full callback URL (or query string) here so jcode can verify the login state.\n"
    );
    eprintln!(
        "If the browser shows a local callback error, copy the full URL from the address bar before closing the tab.\n"
    );
    eprint!("Callback URL: ");
    io::stdout().flush()?;
    let input = crate::cli::login::read_secret_line()?;
    if input.trim().is_empty() {
        anyhow::bail!("No callback URL provided.");
    }

    exchange_callback_input(verifier, &input, Some(expected_state), redirect_uri).await
}

pub async fn exchange_callback_input(
    verifier: &str,
    input: &str,
    expected_state: Option<&str>,
    redirect_uri: &str,
) -> Result<AntigravityTokens> {
    let code = if let Some(expected_state) = expected_state {
        let (code, callback_state) = crate::auth::oauth::parse_callback_input_with_state(input)?;
        if callback_state != expected_state {
            anyhow::bail!(
                "OAuth state mismatch. Start Antigravity login again and use the latest callback URL."
            );
        }
        code
    } else {
        input.trim().to_string()
    };

    let tokens = exchange_authorization_code(&code, verifier, redirect_uri).await?;
    save_tokens(&tokens)?;
    Ok(tokens)
}

pub async fn exchange_callback_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<AntigravityTokens> {
    let tokens = exchange_authorization_code(code, verifier, redirect_uri).await?;
    save_tokens(&tokens)?;
    Ok(tokens)
}

async fn exchange_authorization_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<AntigravityTokens> {
    let client = reqwest::Client::new();
    let client_id = antigravity_client_id();
    let client_secret = antigravity_client_secret();
    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .header(reqwest::header::USER_AGENT, GOOGLE_OAUTH_USER_AGENT)
        .form(&vec![
            ("grant_type", "authorization_code".to_string()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code.trim().to_string()),
            ("code_verifier", verifier.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
        ])
        .send()
        .await
        .context("Failed to exchange Antigravity authorization code")?;

    if !resp.status().is_success() {
        let body = crate::util::http_error_body(resp, "HTTP error").await;
        anyhow::bail!("Antigravity token exchange failed: {}", body.trim());
    }

    let token_resp: super::google_oauth::TokenResponse = resp
        .json()
        .await
        .context("Failed to parse Antigravity token exchange response")?;

    let refresh_token = token_resp.refresh_token.ok_or_else(|| {
        anyhow::anyhow!(
            "No refresh token received. Revoke access at https://myaccount.google.com/permissions and try again."
        )
    })?;

    let email = fetch_email(&token_resp.access_token).await.ok();
    let project_id = fetch_project_id(&token_resp.access_token).await.ok();

    Ok(AntigravityTokens {
        access_token: token_resp.access_token,
        refresh_token,
        expires_at: super::google_oauth::expires_at_from_now(token_resp.expires_in),
        email,
        project_id,
    })
}

pub async fn fetch_email(access_token: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(GOOGLE_USERINFO_URL)
        .header(reqwest::header::USER_AGENT, GOOGLE_OAUTH_USER_AGENT)
        .bearer_auth(access_token)
        .send()
        .await
        .context("Failed to fetch Antigravity Google profile")?;

    if !resp.status().is_success() {
        let body = crate::util::http_error_body(resp, "HTTP error").await;
        anyhow::bail!(
            "Failed to fetch Antigravity Google profile: {}",
            body.trim()
        );
    }

    let profile: GoogleUserInfo = resp
        .json()
        .await
        .context("Failed to parse Antigravity Google profile")?;
    profile
        .email
        .filter(|email| !email.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Google profile did not include an email address"))
}

pub async fn fetch_project_id(access_token: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let headers = antigravity_headers(access_token)?;
    let body = serde_json::json!({
        "metadata": {
            "ideType": "ANTIGRAVITY",
            "platform": metadata_platform(),
            "pluginType": "GEMINI"
        }
    });
    let mut errors = Vec::new();

    for base_url in LOAD_ENDPOINTS {
        let resp = match client
            .post(format!("{base_url}/v1internal:loadCodeAssist"))
            .headers(headers.clone())
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                errors.push(format!("{base_url}: {err}"));
                continue;
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = crate::util::http_error_body(resp, "HTTP error").await;
            errors.push(format!("{base_url}: HTTP {status} {}", text.trim()));
            continue;
        }

        let parsed: LoadCodeAssistResponse = resp
            .json()
            .await
            .with_context(|| format!("Failed to parse loadCodeAssist response from {base_url}"))?;
        if let Some(project_id) = extract_project_id(parsed.cloudaicompanion_project) {
            return Ok(project_id);
        }
        errors.push(format!("{base_url}: project id missing from response"));
    }

    anyhow::bail!(
        "Failed to resolve Antigravity project via loadCodeAssist: {}",
        errors.join("; ")
    )
}

pub fn build_auth_url(redirect_uri: &str, challenge: &str, state: &str) -> Result<String> {
    let scope = ANTIGRAVITY_SCOPES.join(" ");
    let client_id = antigravity_client_id();
    Ok(format!(
        "{base}?response_type=code&client_id={client_id}&redirect_uri={redirect_uri}&scope={scope}&code_challenge={challenge}&code_challenge_method=S256&state={state}&access_type=offline&prompt=consent",
        base = GOOGLE_AUTHORIZE_URL,
        client_id = urlencoding::encode(&client_id),
        redirect_uri = urlencoding::encode(redirect_uri),
        scope = urlencoding::encode(&scope),
        challenge = urlencoding::encode(challenge),
        state = urlencoding::encode(state),
    ))
}

pub fn redirect_uri(port: u16) -> String {
    format!("http://{LOOPBACK_HOST}:{port}{REDIRECT_PATH}")
}

fn antigravity_headers(access_token: &str) -> Result<reqwest::header::HeaderMap> {
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}"))
            .context("invalid Antigravity authorization header")?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(&user_agent()).context("invalid Antigravity user-agent header")?,
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("x-goog-api-client"),
        HeaderValue::from_static("google-cloud-sdk vscode_cloudshelleditor/0.1"),
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("client-metadata"),
        HeaderValue::from_str(&client_metadata_header())
            .context("invalid Antigravity client-metadata header")?,
    );
    Ok(headers)
}

fn extract_project_id(value: Option<serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(project_id)) => {
            let trimmed = project_id.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Some(serde_json::Value::Object(map)) => map
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::lock_test_env;

    #[test]
    fn build_auth_url_includes_antigravity_scope_and_redirect() {
        let _guard = lock_test_env();
        crate::env::set_var(
            CLIENT_ID_ENV,
            "test-antigravity-client-id.apps.googleusercontent.com",
        );
        let url = build_auth_url(
            "http://127.0.0.1:51121/oauth-callback",
            "challenge",
            "state",
        )
        .expect("build auth url");
        assert!(url.contains("client_id=test-antigravity-client-id.apps.googleusercontent.com"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A51121%2Foauth-callback"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("state=state"));
        assert!(url.contains("cloud-platform"));
        assert!(url.contains("experimentsandconfigs"));
        crate::env::remove_var(CLIENT_ID_ENV);
    }

    #[test]
    fn build_auth_url_uses_default_client_id_when_env_missing() {
        let _guard = lock_test_env();
        crate::env::remove_var(CLIENT_ID_ENV);
        let url = build_auth_url(
            "http://127.0.0.1:51121/oauth-callback",
            "challenge",
            "state",
        )
        .expect("missing env should use built-in client id");
        assert!(url.contains(&format!(
            "client_id={}",
            urlencoding::encode(ANTIGRAVITY_CLIENT_ID)
        )));
    }

    #[test]
    fn blank_env_vars_fall_back_to_built_in_credentials() {
        let _guard = lock_test_env();
        crate::env::set_var(CLIENT_ID_ENV, "   ");
        crate::env::set_var(CLIENT_SECRET_ENV, "   ");

        assert_eq!(antigravity_client_id(), ANTIGRAVITY_CLIENT_ID);
        assert_eq!(antigravity_client_secret(), ANTIGRAVITY_CLIENT_SECRET);

        crate::env::remove_var(CLIENT_ID_ENV);
        crate::env::remove_var(CLIENT_SECRET_ENV);
    }

    #[test]
    fn redirect_uri_uses_ipv4_loopback() {
        assert_eq!(
            redirect_uri(DEFAULT_PORT),
            "http://127.0.0.1:51121/oauth-callback"
        );
    }

    #[test]
    fn client_metadata_uses_backend_accepted_platform() {
        assert_eq!(metadata_platform(), "PLATFORM_UNSPECIFIED");
        assert!(client_metadata_header().contains("\"platform\":\"PLATFORM_UNSPECIFIED\""));
    }

    #[test]
    fn extract_project_id_supports_string_or_object() {
        assert_eq!(
            extract_project_id(Some(serde_json::Value::String("proj-123".to_string()))),
            Some("proj-123".to_string())
        );
        assert_eq!(
            extract_project_id(Some(serde_json::json!({ "id": "proj-456" }))),
            Some("proj-456".to_string())
        );
        assert_eq!(
            extract_project_id(Some(serde_json::json!({ "id": "   " }))),
            None
        );
    }
}
