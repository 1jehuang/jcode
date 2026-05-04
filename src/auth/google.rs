use anyhow::Result;
use serde::{Deserialize, Serialize};

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const DEFAULT_PORT: u16 = 8456;

pub const SCOPE_READONLY: &str = "https://www.googleapis.com/auth/gmail.readonly";
pub const SCOPE_COMPOSE: &str = "https://www.googleapis.com/auth/gmail.compose";
pub const SCOPE_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
pub const SCOPE_MODIFY: &str = "https://www.googleapis.com/auth/gmail.modify";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GmailAccessTier {
    #[serde(rename = "full")]
    Full,
    #[serde(rename = "readonly")]
    ReadOnly,
}

impl GmailAccessTier {
    pub fn scopes(&self) -> Vec<&'static str> {
        match self {
            GmailAccessTier::Full => vec![SCOPE_READONLY, SCOPE_COMPOSE, SCOPE_SEND, SCOPE_MODIFY],
            GmailAccessTier::ReadOnly => vec![SCOPE_READONLY, SCOPE_COMPOSE],
        }
    }

    pub fn can_send(&self) -> bool {
        matches!(self, GmailAccessTier::Full)
    }

    pub fn can_delete(&self) -> bool {
        matches!(self, GmailAccessTier::Full)
    }

    pub fn label(&self) -> &'static str {
        match self {
            GmailAccessTier::Full => "Full Access",
            GmailAccessTier::ReadOnly => "Read & Draft Only",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleCredentials {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub tier: GmailAccessTier,
    pub email: Option<String>,
}

impl GoogleTokens {
    pub fn is_expired(&self) -> bool {
        super::google_oauth::token_is_expired(self.expires_at)
    }
}

pub fn credentials_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("google_credentials.json"))
}

pub fn tokens_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("google_oauth.json"))
}

pub fn load_credentials() -> Result<GoogleCredentials> {
    let path = credentials_path()?;
    crate::storage::harden_secret_file_permissions(&path);
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return Err(anyhow::anyhow!("no_credentials")),
    };

    if let Ok(creds) = serde_json::from_str::<GoogleCredentials>(&data) {
        return Ok(creds);
    }

    #[derive(Deserialize)]
    struct GCloudFormat {
        installed: Option<GCloudInstalled>,
        web: Option<GCloudInstalled>,
    }
    #[derive(Deserialize)]
    struct GCloudInstalled {
        client_id: String,
        client_secret: String,
    }

    let gcloud: GCloudFormat = serde_json::from_str(&data)?;
    let inner = gcloud
        .installed
        .or(gcloud.web)
        .ok_or_else(|| anyhow::anyhow!("Invalid Google credentials format"))?;

    Ok(GoogleCredentials {
        client_id: inner.client_id,
        client_secret: inner.client_secret,
    })
}

pub fn save_credentials(creds: &GoogleCredentials) -> Result<()> {
    let path = credentials_path()?;
    crate::storage::write_json_secret(&path, creds)
}

pub fn load_tokens() -> Result<GoogleTokens> {
    let path = tokens_path()?;
    if !path.exists() {
        anyhow::bail!("No Google tokens found. Run `jcode login google` first.");
    }
    crate::storage::harden_secret_file_permissions(&path);
    crate::storage::read_json(&path)
        .map_err(|_| anyhow::anyhow!("No Google tokens found. Run `jcode login google` first."))
}

pub fn save_tokens(tokens: &GoogleTokens) -> Result<()> {
    let path = tokens_path()?;
    crate::storage::write_json_secret(&path, tokens)
}

pub fn build_auth_url(
    creds: &GoogleCredentials,
    tier: GmailAccessTier,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    let scopes = tier.scopes().join(" ");
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        AUTHORIZE_URL,
        urlencoding::encode(&creds.client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(&scopes),
        challenge,
        state
    )
}

pub fn has_tokens() -> bool {
    tokens_path().map(|path| path.exists()).unwrap_or(false)
}

pub async fn login(tier: GmailAccessTier, no_browser: bool) -> Result<GoogleTokens> {
    let creds = load_credentials()?;
    let (verifier, challenge) = super::oauth::generate_pkce_public();
    let state = super::oauth::generate_state_public();

    let listener = super::oauth::bind_callback_listener(0).ok();
    let redirect_uri = listener
        .as_ref()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr| format!("http://127.0.0.1:{}", addr.port()))
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", DEFAULT_PORT));

    let auth_url = build_auth_url(&creds, tier, &redirect_uri, &challenge, &state);

    eprintln!("\nOpening browser for Google login...\n");
    eprintln!("If the browser didn't open, visit:\n{}\n", auth_url);
    if let Some(qr) = crate::login_qr::indented_section(
        &auth_url,
        "Scan this QR on another device if this machine has no browser:",
        "    ",
    ) {
        eprintln!("{qr}\n");
    }

    let browser_opened = if crate::auth::browser_suppressed(no_browser) {
        false
    } else {
        open::that(&auth_url).is_ok()
    };

    let code = if browser_opened {
        eprintln!(
            "Waiting up to 300s for automatic callback on {}",
            redirect_uri
        );
        if let Some(listener) = listener {
            match tokio::time::timeout(
                std::time::Duration::from_secs(300),
                super::oauth::wait_for_callback_async_on_listener(listener, &state),
            )
            .await
            {
                Ok(Ok(code)) => code,
                Ok(Err(err)) => {
                    eprintln!("Automatic callback failed ({err}). Falling back to manual paste.");
                    read_manual_callback_code(&state)?
                }
                Err(_) => {
                    eprintln!("Timed out waiting for callback. Falling back to manual paste.");
                    read_manual_callback_code(&state)?
                }
            }
        } else {
            eprintln!(
                "Couldn't start a local callback listener. Finish login in any browser, then paste the full callback URL here.\n"
            );
            read_manual_callback_code(&state)?
        }
    } else {
        eprintln!(
            "Couldn't open a browser on this machine. Use the QR code above, then paste the full callback URL here.\n"
        );
        read_manual_callback_code(&state)?
    };

    eprintln!("Exchanging code for tokens...");
    exchange_code(&creds, &verifier, &code, &redirect_uri, tier).await
}

fn read_manual_callback_code(expected_state: &str) -> Result<String> {
    use std::io::Write;

    eprintln!("Paste the full callback URL (or query string) here:\n");
    eprint!("> ");
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("No callback URL entered.");
    }

    let (code, callback_state) = crate::auth::oauth::parse_callback_input_with_state(trimmed)?;
    if callback_state != expected_state {
        anyhow::bail!("OAuth state mismatch. Start login again and use the latest callback URL.");
    }
    Ok(code)
}

pub async fn exchange_callback_input(
    creds: &GoogleCredentials,
    verifier: &str,
    input: &str,
    expected_state: &str,
    redirect_uri: &str,
    tier: GmailAccessTier,
) -> Result<GoogleTokens> {
    let (code, callback_state) = crate::auth::oauth::parse_callback_input_with_state(input)?;
    if callback_state != expected_state {
        anyhow::bail!("OAuth state mismatch. Start login again and use the latest callback URL.");
    }
    exchange_code(creds, verifier, &code, redirect_uri, tier).await
}

async fn exchange_code(
    creds: &GoogleCredentials,
    verifier: &str,
    code: &str,
    redirect_uri: &str,
    tier: GmailAccessTier,
) -> Result<GoogleTokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", &creds.client_id),
            ("client_secret", &creds.client_secret),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("Google token exchange failed: {}", text);
    }

    let token_resp: super::google_oauth::TokenResponse = resp.json().await?;
    let expires_at = super::google_oauth::expires_at_from_now(token_resp.expires_in);

    let refresh_token = token_resp.refresh_token.ok_or_else(|| {
        anyhow::anyhow!("No refresh token received. Try revoking access at https://myaccount.google.com/permissions and logging in again.")
    })?;

    let email = fetch_email(&token_resp.access_token).await.ok();

    let tokens = GoogleTokens {
        access_token: token_resp.access_token,
        refresh_token,
        expires_at,
        tier,
        email,
    };

    save_tokens(&tokens)?;
    Ok(tokens)
}

pub async fn refresh_tokens(tokens: &GoogleTokens) -> Result<GoogleTokens> {
    let result: Result<GoogleTokens> = async {
        let creds = load_credentials()?;
        let client = reqwest::Client::new();

        let resp = client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &creds.client_id),
                ("client_secret", &creds.client_secret),
                ("refresh_token", &tokens.refresh_token),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await?;
            anyhow::bail!("Google token refresh failed: {}", text);
        }

        let refresh_resp: super::google_oauth::TokenResponse = resp.json().await?;
        let expires_at = super::google_oauth::expires_at_from_now(refresh_resp.expires_in);

        let new_tokens = GoogleTokens {
            access_token: refresh_resp.access_token,
            refresh_token: tokens.refresh_token.clone(),
            expires_at,
            tier: tokens.tier,
            email: tokens.email.clone(),
        };

        save_tokens(&new_tokens)?;
        Ok(new_tokens)
    }
    .await;

    match &result {
        Ok(_) => {
            let _ = crate::auth::refresh_state::record_success("google");
        }
        Err(err) => {
            let _ = crate::auth::refresh_state::record_failure("google", err.to_string());
        }
    }

    result
}

pub async fn get_valid_token() -> Result<String> {
    let tokens = load_tokens()?;
    if tokens.is_expired() {
        let new_tokens = refresh_tokens(&tokens).await?;
        Ok(new_tokens.access_token)
    } else {
        Ok(tokens.access_token)
    }
}

async fn fetch_email(access_token: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://gmail.googleapis.com/gmail/v1/users/me/profile")
        .bearer_auth(access_token)
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to fetch Gmail profile");
    }

    #[derive(Deserialize)]
    struct Profile {
        #[serde(rename = "emailAddress")]
        email_address: String,
    }

    let profile: Profile = resp.json().await?;
    Ok(profile.email_address)
}
