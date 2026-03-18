use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, IsTerminal, Write};

const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
const GEMINI_MANUAL_REDIRECT_URI: &str = "https://codeassist.google.com/authcode";
// OAuth credentials from Google's official Gemini CLI (@google/gemini-cli).
// These are for a "Desktop app" OAuth type where the client secret is safe to embed.
// See: https://developers.google.com/identity/protocols/oauth2#installed
// gitleaks:allow - public desktop OAuth credentials, safe to embed
const GEMINI_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com"; // gitleaks:allow
const GEMINI_CLIENT_SECRET: &str = "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"; // gitleaks:allow
// Env vars can override the hardcoded credentials if needed
const GEMINI_CLIENT_ID_ENV: &str = "GEMINI_CLIENT_ID";
const GEMINI_CLIENT_SECRET_ENV: &str = "GEMINI_CLIENT_SECRET";
const GEMINI_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
];

fn gemini_client_id() -> String {
    std::env::var(GEMINI_CLIENT_ID_ENV).unwrap_or_else(|_| GEMINI_CLIENT_ID.to_string())
}

fn gemini_client_secret() -> String {
    std::env::var(GEMINI_CLIENT_SECRET_ENV).unwrap_or_else(|_| GEMINI_CLIENT_SECRET.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeminiCliCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl GeminiCliCommand {
    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl GeminiTokens {
    pub fn is_expired(&self) -> bool {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.expires_at <= now_ms + 60_000
    }
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct GoogleUserInfo {
    #[allow(dead_code)]
    id: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiCliOAuthCredentials {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expiry_date: Option<i64>,
    expires_at: Option<i64>,
    expires_in: Option<i64>,
}

/// Resolve the Gemini CLI command from the environment or a sensible default.
///
/// Preference order:
/// 1. `JCODE_GEMINI_CLI_PATH` (supports a full command like `npx @google/gemini-cli`)
/// 2. `gemini` on PATH
/// 3. `npx @google/gemini-cli`
pub fn gemini_cli_command() -> GeminiCliCommand {
    resolve_gemini_cli_command_with(
        std::env::var("JCODE_GEMINI_CLI_PATH").ok().as_deref(),
        super::command_exists,
    )
}

/// Resolve just the executable portion for legacy callers.
pub fn gemini_cli_path() -> String {
    gemini_cli_command().program
}

/// Check if a usable Gemini CLI command is available.
pub fn has_gemini_cli() -> bool {
    let resolved = gemini_cli_command();
    super::command_exists(&resolved.program)
}

/// Check if native Gemini OAuth tokens are available (including imported Gemini CLI tokens).
pub fn has_cached_auth() -> bool {
    load_tokens().is_ok()
}

pub fn tokens_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("gemini_oauth.json"))
}

pub fn gemini_cli_oauth_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::user_home_path(".gemini/oauth_creds.json")?)
}

pub fn load_tokens() -> Result<GeminiTokens> {
    let native_path = tokens_path()?;
    if native_path.exists() {
        crate::storage::harden_secret_file_permissions(&native_path);
        return crate::storage::read_json(&native_path)
            .with_context(|| format!("Failed to read {}", native_path.display()));
    }

    let cli_path = gemini_cli_oauth_path()?;
    if cli_path.exists() {
        crate::storage::harden_secret_file_permissions(&cli_path);
        let raw = std::fs::read_to_string(&cli_path)
            .with_context(|| format!("Failed to read {}", cli_path.display()))?;
        let imported: GeminiCliOAuthCredentials = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse {}", cli_path.display()))?;
        let refresh_token = imported
            .refresh_token
            .filter(|value| !value.trim().is_empty());
        let access_token = imported
            .access_token
            .filter(|value| !value.trim().is_empty());
        if let (Some(refresh_token), Some(access_token)) = (refresh_token, access_token) {
            let expires_at = imported
                .expiry_date
                .or(imported.expires_at)
                .or_else(|| {
                    imported.expires_in.map(|expires_in| {
                        chrono::Utc::now().timestamp_millis() + (expires_in * 1000)
                    })
                })
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() - 1);
            return Ok(GeminiTokens {
                access_token,
                refresh_token,
                expires_at,
                email: None,
            });
        }
    }

    anyhow::bail!("No Gemini OAuth tokens found. Run `jcode login --provider gemini`.")
}

pub fn save_tokens(tokens: &GeminiTokens) -> Result<()> {
    let path = tokens_path()?;
    crate::storage::write_json_secret(&path, tokens)?;
    Ok(())
}

pub fn clear_tokens() -> Result<()> {
    let path = tokens_path()?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub async fn load_or_refresh_tokens() -> Result<GeminiTokens> {
    let tokens = load_tokens()?;
    if tokens.is_expired() {
        refresh_tokens(&tokens).await
    } else {
        Ok(tokens)
    }
}

pub async fn refresh_tokens(tokens: &GeminiTokens) -> Result<GeminiTokens> {
    let client_id = gemini_client_id();
    let client_secret = gemini_client_secret();
    let client = reqwest::Client::new();
    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .form(&vec![
            ("grant_type", "refresh_token".to_string()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", tokens.refresh_token.clone()),
        ])
        .send()
        .await
        .context("Failed to refresh Gemini OAuth token")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Gemini token refresh failed: {}", body.trim());
    }

    let token_resp: GoogleTokenResponse = resp
        .json()
        .await
        .context("Failed to parse Gemini refresh response")?;
    let refreshed = GeminiTokens {
        access_token: token_resp.access_token,
        refresh_token: token_resp
            .refresh_token
            .unwrap_or_else(|| tokens.refresh_token.clone()),
        expires_at: chrono::Utc::now().timestamp_millis() + (token_resp.expires_in * 1000),
        email: tokens.email.clone(),
    };
    save_tokens(&refreshed)?;
    Ok(refreshed)
}

pub async fn login() -> Result<GeminiTokens> {
    let (verifier, challenge) = super::oauth::generate_pkce_public();
    let state = super::oauth::generate_state_public();

    if !browser_suppressed() {
        if let Ok(listener) = super::oauth::bind_callback_listener(0) {
            let port = listener.local_addr()?.port();
            let redirect_uri = format!("http://127.0.0.1:{port}/oauth2callback");
            let auth_url = build_web_auth_url(&redirect_uri, &challenge, &state)?;

            eprintln!("\nOpening browser for Gemini login...\n");
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
                    "If the page says sign-in succeeded but jcode does not continue within a few seconds, press Ctrl+C and retry with NO_BROWSER=true to use the manual code flow."
                );
                match tokio::time::timeout(
                    std::time::Duration::from_secs(300),
                    super::oauth::wait_for_callback_async_on_listener(listener, &state),
                )
                .await
                {
                    Ok(Ok(code)) => {
                        let tokens =
                            exchange_authorization_code(&code, Some(&verifier), &redirect_uri)
                                .await
                                .context("Gemini token exchange failed")?;
                        save_tokens(&tokens)?;
                        return Ok(tokens);
                    }
                    Ok(Err(err)) => {
                        eprintln!(
                            "Automatic callback failed ({err}). Falling back to manual auth code entry."
                        );
                    }
                    Err(_) => {
                        eprintln!(
                            "Timed out waiting for callback. Falling back to manual auth code entry."
                        );
                    }
                }
            } else {
                eprintln!(
                    "Couldn't open a browser on this machine. Falling back to manual auth code entry.\n"
                );
            }
        }
    }

    manual_login(&verifier, &challenge, &state).await
}

async fn manual_login(verifier: &str, challenge: &str, state: &str) -> Result<GeminiTokens> {
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "Gemini login needs an interactive terminal for manual code entry. Re-run in an interactive terminal."
        );
    }

    let auth_url = build_manual_auth_url(GEMINI_MANUAL_REDIRECT_URI, challenge, state)?;
    eprintln!("\nManual Gemini auth required.\n");
    eprintln!("Open this URL in your browser:\n\n{}\n", auth_url);
    if let Some(qr) = crate::login_qr::indented_section(
        &auth_url,
        "Scan this QR on another device if needed:",
        "    ",
    ) {
        eprintln!("{qr}\n");
    }
    if !browser_suppressed() {
        let _ = open::that(&auth_url);
    }
    eprintln!("After approving access, Google will show an authorization code. Paste it below.\n");
    eprint!("Authorization code: ");
    io::stdout().flush()?;
    let code = crate::cli::login::read_secret_line()?;
    if code.trim().is_empty() {
        anyhow::bail!("No authorization code provided.");
    }

    let tokens = exchange_authorization_code(&code, Some(verifier), GEMINI_MANUAL_REDIRECT_URI)
        .await
        .context("Gemini token exchange failed")?;
    save_tokens(&tokens)?;
    Ok(tokens)
}

pub async fn exchange_callback_input(
    verifier: &str,
    input: &str,
    expected_state: Option<&str>,
    redirect_uri: &str,
) -> Result<GeminiTokens> {
    let (code, verifier) = if let Some(expected_state) = expected_state {
        let (code, callback_state) = crate::auth::oauth::parse_callback_input_with_state(input)?;
        if callback_state != expected_state {
            anyhow::bail!(
                "OAuth state mismatch. Start Gemini login again and use the latest callback URL."
            );
        }
        (code, None)
    } else {
        (input.trim().to_string(), Some(verifier))
    };

    let tokens = exchange_authorization_code(&code, verifier, redirect_uri).await?;
    save_tokens(&tokens)?;
    Ok(tokens)
}

pub async fn exchange_callback_code(code: &str, redirect_uri: &str) -> Result<GeminiTokens> {
    let tokens = exchange_authorization_code(code, None, redirect_uri).await?;
    save_tokens(&tokens)?;
    Ok(tokens)
}

async fn exchange_authorization_code(
    code: &str,
    verifier: Option<&str>,
    redirect_uri: &str,
) -> Result<GeminiTokens> {
    let client_id = gemini_client_id();
    let client_secret = gemini_client_secret();
    let client = reqwest::Client::new();
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code.trim().to_string()),
        ("redirect_uri", redirect_uri.to_string()),
    ];
    if let Some(verifier) = verifier {
        form.push(("code_verifier", verifier.to_string()));
    }
    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .form(&form)
        .send()
        .await
        .context("Failed to exchange Gemini authorization code")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Gemini token exchange failed: {}", body.trim());
    }

    let token_resp: GoogleTokenResponse = resp
        .json()
        .await
        .context("Failed to parse Gemini token exchange response")?;

    let refresh_token = token_resp.refresh_token.ok_or_else(|| {
        anyhow::anyhow!(
            "No refresh token received. Revoke access at https://myaccount.google.com/permissions and try again."
        )
    })?;

    let email = fetch_email(&token_resp.access_token).await.ok();
    Ok(GeminiTokens {
        access_token: token_resp.access_token,
        refresh_token,
        expires_at: chrono::Utc::now().timestamp_millis() + (token_resp.expires_in * 1000),
        email,
    })
}

pub async fn fetch_email(access_token: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .context("Failed to fetch Gemini Google profile")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Failed to fetch Gemini Google profile: {}", body.trim());
    }

    let profile: GoogleUserInfo = resp
        .json()
        .await
        .context("Failed to parse Gemini Google profile")?;
    profile
        .email
        .filter(|email| !email.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Google profile did not include an email address"))
}

pub fn build_web_auth_url(redirect_uri: &str, challenge: &str, state: &str) -> Result<String> {
    let scope = GEMINI_SCOPES.join(" ");
    let client_id = gemini_client_id();
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

pub fn build_manual_auth_url(redirect_uri: &str, challenge: &str, state: &str) -> Result<String> {
    let scope = GEMINI_SCOPES.join(" ");
    let client_id = gemini_client_id();
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

fn browser_suppressed() -> bool {
    env_truthy("NO_BROWSER") || env_truthy("JCODE_NO_BROWSER")
}

fn env_truthy(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            let trimmed = value.trim();
            !trimmed.is_empty() && trimmed != "0" && !trimmed.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

fn resolve_gemini_cli_command_with<F>(env_spec: Option<&str>, command_exists: F) -> GeminiCliCommand
where
    F: Fn(&str) -> bool,
{
    if let Some(spec) = env_spec.and_then(parse_command_spec) {
        return GeminiCliCommand {
            program: spec[0].clone(),
            args: spec[1..].to_vec(),
        };
    }

    if command_exists("gemini") {
        return GeminiCliCommand {
            program: "gemini".to_string(),
            args: Vec::new(),
        };
    }

    if command_exists("npx") {
        return GeminiCliCommand {
            program: "npx".to_string(),
            args: vec!["@google/gemini-cli".to_string()],
        };
    }

    GeminiCliCommand {
        program: "gemini".to_string(),
        args: Vec::new(),
    }
}

fn parse_command_spec(raw: &str) -> Option<Vec<String>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    for ch in raw.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }

        match ch {
            '\\' if !in_single => escape = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if escape {
        current.push('\\');
    }

    if !current.is_empty() {
        parts.push(current);
    }

    if parts.is_empty() { None } else { Some(parts) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::lock_test_env;

    struct GeminiOauthEnvReset {
        prev_client_id: Option<String>,
        prev_client_secret: Option<String>,
    }

    impl Drop for GeminiOauthEnvReset {
        fn drop(&mut self) {
            if let Some(value) = &self.prev_client_id {
                crate::env::set_var(GEMINI_CLIENT_ID_ENV, value);
            } else {
                crate::env::remove_var(GEMINI_CLIENT_ID_ENV);
            }

            if let Some(value) = &self.prev_client_secret {
                crate::env::set_var(GEMINI_CLIENT_SECRET_ENV, value);
            } else {
                crate::env::remove_var(GEMINI_CLIENT_SECRET_ENV);
            }
        }
    }

    fn set_test_gemini_oauth_env() -> GeminiOauthEnvReset {
        let reset = GeminiOauthEnvReset {
            prev_client_id: std::env::var(GEMINI_CLIENT_ID_ENV).ok(),
            prev_client_secret: std::env::var(GEMINI_CLIENT_SECRET_ENV).ok(),
        };
        crate::env::set_var(
            GEMINI_CLIENT_ID_ENV,
            "test-gemini-client.apps.googleusercontent.com",
        );
        crate::env::set_var(GEMINI_CLIENT_SECRET_ENV, "test-gemini-client-secret");
        reset
    }

    #[test]
    fn parses_env_command_with_args() {
        let resolved =
            resolve_gemini_cli_command_with(Some("npx @google/gemini-cli --proxy test"), |_| false);
        assert_eq!(
            resolved,
            GeminiCliCommand {
                program: "npx".to_string(),
                args: vec![
                    "@google/gemini-cli".to_string(),
                    "--proxy".to_string(),
                    "test".to_string(),
                ],
            }
        );
    }

    #[test]
    fn falls_back_to_gemini_binary_when_available() {
        let resolved = resolve_gemini_cli_command_with(None, |cmd| cmd == "gemini");
        assert_eq!(resolved.program, "gemini");
        assert!(resolved.args.is_empty());
    }

    #[test]
    fn falls_back_to_npx_when_gemini_binary_missing() {
        let resolved = resolve_gemini_cli_command_with(None, |cmd| cmd == "npx");
        assert_eq!(resolved.program, "npx");
        assert_eq!(resolved.args, vec!["@google/gemini-cli"]);
    }

    #[test]
    fn display_includes_args_when_present() {
        let command = GeminiCliCommand {
            program: "npx".to_string(),
            args: vec!["@google/gemini-cli".to_string()],
        };
        assert_eq!(command.display(), "npx @google/gemini-cli");
    }

    #[test]
    fn build_manual_auth_url_contains_expected_redirect_uri() {
        let _guard = lock_test_env();
        let _env = set_test_gemini_oauth_env();
        let url = build_manual_auth_url(GEMINI_MANUAL_REDIRECT_URI, "challenge-123", "state-123")
            .expect("manual auth url");
        assert!(url.contains("codeassist.google.com%2Fauthcode"));
        assert!(url.contains("code_challenge=challenge-123"));
        assert!(url.contains("state=state-123"));
    }

    #[test]
    fn build_web_auth_url_includes_pkce_parameters() {
        let _guard = lock_test_env();
        let _env = set_test_gemini_oauth_env();
        let url = build_web_auth_url(
            "http://127.0.0.1:45619/oauth2callback",
            "challenge-123",
            "state-123",
        )
        .expect("web auth url");
        assert!(url.contains("127.0.0.1%3A45619%2Foauth2callback"));
        assert!(url.contains("state=state-123"));
        assert!(url.contains("code_challenge=challenge-123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(!url.contains("code_verifier="));
    }

    #[test]
    fn uses_hardcoded_credentials_when_env_missing() {
        let _guard = lock_test_env();
        crate::env::remove_var(GEMINI_CLIENT_ID_ENV);
        crate::env::remove_var(GEMINI_CLIENT_SECRET_ENV);

        // Should succeed with hardcoded credentials
        let url = build_manual_auth_url(GEMINI_MANUAL_REDIRECT_URI, "challenge-123", "state-123")
            .expect("should use hardcoded credentials");
        assert!(url.contains("codeassist.google.com%2Fauthcode"));
        assert!(url.contains("code_challenge=challenge-123"));
        // Should contain the hardcoded client ID
        assert!(
            url.contains(
                "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com"
            )
        );
    }

    #[test]
    fn imports_cli_oauth_tokens_when_native_tokens_missing() {
        let _guard = lock_test_env();
        let temp = tempfile::TempDir::new().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let cli_path = gemini_cli_oauth_path().expect("cli path");
        std::fs::create_dir_all(cli_path.parent().unwrap()).expect("create cli dir");
        std::fs::write(
            &cli_path,
            r#"{"access_token":"at-123","refresh_token":"rt-456","expiry_date":4102444800000}"#,
        )
        .expect("write cli token file");

        let tokens = load_tokens().expect("load tokens");
        assert_eq!(tokens.access_token, "at-123");
        assert_eq!(tokens.refresh_token, "rt-456");
        assert_eq!(tokens.expires_at, 4102444800000);

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
