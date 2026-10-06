use anyhow::Result;
use serde::{Deserialize, Serialize};

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const DEFAULT_PORT: u16 = 8456;

pub const SCOPE_READONLY: &str = "https://www.googleapis.com/auth/gmail.readonly";
pub const SCOPE_COMPOSE: &str = "https://www.googleapis.com/auth/gmail.compose";
pub const SCOPE_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
pub const SCOPE_MODIFY: &str = "https://www.googleapis.com/auth/gmail.modify";
/// Read/write access to events on all calendars the user can access.
pub const SCOPE_CALENDAR_EVENTS: &str = "https://www.googleapis.com/auth/calendar.events";
/// Read-only access to the user's calendar list (names, ids, time zones).
pub const SCOPE_CALENDAR_LIST_READONLY: &str =
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly";

/// A Google product jcode can be granted access to through `jcode login google`.
///
/// The selected services decide which OAuth scopes are requested. Gmail keeps
/// its separate [`GmailAccessTier`] so read-only Gmail logins stay restricted
/// at the API level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoogleService {
    Gmail,
    Calendar,
}

impl GoogleService {
    pub const ALL: [GoogleService; 2] = [GoogleService::Gmail, GoogleService::Calendar];

    pub fn id(&self) -> &'static str {
        match self {
            GoogleService::Gmail => "gmail",
            GoogleService::Calendar => "calendar",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            GoogleService::Gmail => "Gmail",
            GoogleService::Calendar => "Google Calendar",
        }
    }

    /// Google Cloud Console library page for enabling this service's API.
    pub fn api_library_url(&self) -> &'static str {
        match self {
            GoogleService::Gmail => {
                "https://console.cloud.google.com/apis/library/gmail.googleapis.com"
            }
            GoogleService::Calendar => {
                "https://console.cloud.google.com/apis/library/calendar-json.googleapis.com"
            }
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "gmail" | "mail" | "email" => Some(GoogleService::Gmail),
            "calendar" | "cal" | "gcal" => Some(GoogleService::Calendar),
            _ => None,
        }
    }

    /// Parse a comma-separated list such as `gmail,calendar` or `all`.
    pub fn parse_list(value: &str) -> Result<Vec<GoogleService>> {
        let mut services = Vec::new();
        for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if part.eq_ignore_ascii_case("all") {
                services.extend(GoogleService::ALL);
                continue;
            }
            let service = GoogleService::parse(part).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown Google service '{}'. Supported: gmail, calendar, all.",
                    part
                )
            })?;
            services.push(service);
        }
        let services = normalize_services(services);
        if services.is_empty() {
            anyhow::bail!("No Google services selected. Supported: gmail, calendar, all.");
        }
        Ok(services)
    }
}

/// Sort and dedupe a service list so stored tokens and scope strings are stable.
pub fn normalize_services(mut services: Vec<GoogleService>) -> Vec<GoogleService> {
    services.sort();
    services.dedup();
    services
}

/// Services granted by token files written before service selection existed.
fn default_services() -> Vec<GoogleService> {
    vec![GoogleService::Gmail]
}

/// OAuth scopes for a set of services. Gmail uses the tier's scopes.
pub fn scopes_for(services: &[GoogleService], tier: GmailAccessTier) -> Vec<&'static str> {
    let mut scopes = Vec::new();
    for service in normalize_services(services.to_vec()) {
        match service {
            GoogleService::Gmail => scopes.extend(tier.scopes()),
            GoogleService::Calendar => {
                scopes.extend([SCOPE_CALENDAR_EVENTS, SCOPE_CALENDAR_LIST_READONLY])
            }
        }
    }
    scopes.dedup();
    scopes
}

/// Human-readable summary like `Gmail (Full Access), Google Calendar`.
pub fn describe_services(services: &[GoogleService], tier: GmailAccessTier) -> String {
    services
        .iter()
        .map(|service| match service {
            GoogleService::Gmail => format!("Gmail ({})", tier.label()),
            other => other.label().to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

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
    /// Services this grant covers. Token files from before service selection
    /// only ever requested Gmail scopes, so a missing field means Gmail.
    #[serde(default = "default_services")]
    pub services: Vec<GoogleService>,
}

impl GoogleTokens {
    pub fn is_expired(&self) -> bool {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.expires_at <= now_ms + 60_000
    }

    pub fn has_service(&self, service: GoogleService) -> bool {
        self.services.contains(&service)
    }
}

/// Whether the saved Google login grants `service`.
pub fn has_service(service: GoogleService) -> bool {
    load_tokens()
        .map(|tokens| tokens.has_service(service))
        .unwrap_or(false)
}

/// Command that re-runs Google login keeping the currently granted services
/// and adding `service`.
pub fn login_command_adding(service: GoogleService) -> String {
    let mut services = load_tokens()
        .map(|tokens| tokens.services)
        .unwrap_or_default();
    services.push(service);
    let services = normalize_services(services);
    format!(
        "jcode login google --google-services {}",
        services
            .iter()
            .map(GoogleService::id)
            .collect::<Vec<_>>()
            .join(",")
    )
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
    services: &[GoogleService],
    tier: GmailAccessTier,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    let scopes = scopes_for(services, tier).join(" ");
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent&include_granted_scopes=true",
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

pub async fn login(
    services: &[GoogleService],
    tier: GmailAccessTier,
    no_browser: bool,
) -> Result<GoogleTokens> {
    let creds = load_credentials()?;
    let (verifier, challenge) = super::oauth::generate_pkce_public();
    let state = super::oauth::generate_state_public();

    let listener = super::oauth::bind_callback_listener(0).ok();
    let redirect_uri = listener
        .as_ref()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr| format!("http://127.0.0.1:{}", addr.port()))
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", DEFAULT_PORT));

    let auth_url = build_auth_url(&creds, services, tier, &redirect_uri, &challenge, &state);

    eprintln!("\nOpening browser for Google login...\n");
    eprintln!("If the browser didn't open, visit:\n{}\n", auth_url);
    if let Some(qr) = crate::login_qr::indented_section(
        &auth_url,
        "Scan this QR on another device if this machine has no browser:",
        "    ",
        crate::auth::browser_suppressed(no_browser),
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
    exchange_code(&creds, &verifier, &code, &redirect_uri, services, tier).await
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
    services: &[GoogleService],
    tier: GmailAccessTier,
) -> Result<GoogleTokens> {
    let (code, callback_state) = crate::auth::oauth::parse_callback_input_with_state(input)?;
    if callback_state != expected_state {
        anyhow::bail!("OAuth state mismatch. Start login again and use the latest callback URL.");
    }
    exchange_code(creds, verifier, &code, redirect_uri, services, tier).await
}

async fn exchange_code(
    creds: &GoogleCredentials,
    verifier: &str,
    code: &str,
    redirect_uri: &str,
    services: &[GoogleService],
    tier: GmailAccessTier,
) -> Result<GoogleTokens> {
    let client = crate::provider::shared_http_client();
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

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: i64,
        #[serde(default)]
        scope: Option<String>,
    }

    let token_resp: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (token_resp.expires_in * 1000);

    let refresh_token = token_resp.refresh_token.ok_or_else(|| {
        anyhow::anyhow!("No refresh token received. Try revoking access at https://myaccount.google.com/permissions and logging in again.")
    })?;

    // Google lets users untick individual scopes on the consent screen, so
    // record only the services whose scopes were actually granted.
    let services = match token_resp.scope.as_deref() {
        Some(granted) => granted_services(services, tier, granted),
        None => normalize_services(services.to_vec()),
    };
    if services.is_empty() {
        anyhow::bail!(
            "Google login did not grant access to any requested service. Run the login again and allow the requested permissions."
        );
    }

    let email = fetch_email(&token_resp.access_token, &services).await.ok();

    let tokens = GoogleTokens {
        access_token: token_resp.access_token,
        refresh_token,
        expires_at,
        tier,
        email,
        services,
    };

    save_tokens(&tokens)?;
    Ok(tokens)
}

/// Refresh Google OAuth tokens, serialized via the refresh coordinator so
/// concurrent callers do not race the token endpoint and the stored file.
pub async fn refresh_tokens(tokens: &GoogleTokens) -> Result<GoogleTokens> {
    crate::auth::refresh_coordinator::single_flight(
        "google".to_string(),
        || load_tokens().ok(),
        |stored: &GoogleTokens| !stored.is_expired(),
        {
            let observed = tokens.clone();
            move |stored: Option<GoogleTokens>| async move {
                let source = stored.unwrap_or(observed);
                refresh_tokens_uncoordinated(&source).await
            }
        },
    )
    .await
}

async fn refresh_tokens_uncoordinated(tokens: &GoogleTokens) -> Result<GoogleTokens> {
    let result: Result<GoogleTokens> = async {
        let creds = load_credentials()?;
        let refreshed = crate::auth::google_oauth::refresh_access_token(
            "Google",
            &creds.client_id,
            &creds.client_secret,
            &tokens.refresh_token,
            None,
        )
        .await?;

        let new_tokens = GoogleTokens {
            access_token: refreshed.access_token,
            refresh_token: refreshed.refresh_token,
            expires_at: refreshed.expires_at_ms,
            tier: tokens.tier,
            email: tokens.email.clone(),
            services: tokens.services.clone(),
        };

        save_tokens(&new_tokens)?;
        Ok(new_tokens)
    }
    .await;

    // Shared recorder: a permanently rejected refresh token becomes terminal
    // so background sweeps stop retrying it; transient failures stay retryable.
    crate::auth::refresh_state::record_refresh_outcome("google", &tokens.refresh_token, &result);

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

/// Services from `requested` whose scopes all appear in the granted scope string.
pub fn granted_services(
    requested: &[GoogleService],
    tier: GmailAccessTier,
    granted: &str,
) -> Vec<GoogleService> {
    let granted: std::collections::HashSet<&str> = granted.split_whitespace().collect();
    normalize_services(
        requested
            .iter()
            .copied()
            .filter(|service| {
                scopes_for(&[*service], tier)
                    .iter()
                    .all(|scope| granted.contains(scope))
            })
            .collect(),
    )
}

async fn fetch_email(access_token: &str, services: &[GoogleService]) -> Result<String> {
    if services.contains(&GoogleService::Gmail) {
        return fetch_gmail_email(access_token).await;
    }
    fetch_primary_calendar_id(access_token).await
}

/// The primary calendar's id is the account's email address.
async fn fetch_primary_calendar_id(access_token: &str) -> Result<String> {
    let client = crate::provider::shared_http_client();
    let resp = client
        .get("https://www.googleapis.com/calendar/v3/users/me/calendarList/primary")
        .bearer_auth(access_token)
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Failed to fetch primary calendar");
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }
    Ok(resp.json::<Entry>().await?.id)
}

async fn fetch_gmail_email(access_token: &str) -> Result<String> {
    let client = crate::provider::shared_http_client();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_token_file_defaults_to_gmail_only() {
        let json =
            r#"{"access_token":"a","refresh_token":"r","expires_at":0,"tier":"full","email":null}"#;
        let tokens: GoogleTokens = serde_json::from_str(json).unwrap();
        assert_eq!(tokens.services, vec![GoogleService::Gmail]);
        assert!(!tokens.has_service(GoogleService::Calendar));
    }

    #[test]
    fn parse_list_accepts_aliases_all_and_dedupes() {
        assert_eq!(
            GoogleService::parse_list("calendar, gmail,cal").unwrap(),
            vec![GoogleService::Gmail, GoogleService::Calendar]
        );
        assert_eq!(
            GoogleService::parse_list("all").unwrap(),
            GoogleService::ALL.to_vec()
        );
        assert!(GoogleService::parse_list("drive").is_err());
        assert!(GoogleService::parse_list(" , ").is_err());
    }

    #[test]
    fn scopes_combine_gmail_tier_and_calendar() {
        let scopes = scopes_for(
            &[GoogleService::Calendar, GoogleService::Gmail],
            GmailAccessTier::ReadOnly,
        );
        assert_eq!(
            scopes,
            vec![
                SCOPE_READONLY,
                SCOPE_COMPOSE,
                SCOPE_CALENDAR_EVENTS,
                SCOPE_CALENDAR_LIST_READONLY
            ]
        );
        let calendar_only = scopes_for(&[GoogleService::Calendar], GmailAccessTier::Full);
        assert!(!calendar_only.iter().any(|s| s.contains("gmail")));
    }

    #[test]
    fn auth_url_requests_selected_scopes() {
        let creds = GoogleCredentials {
            client_id: "id".into(),
            client_secret: "secret".into(),
        };
        let url = build_auth_url(
            &creds,
            &[GoogleService::Calendar],
            GmailAccessTier::Full,
            "http://127.0.0.1:1",
            "c",
            "s",
        );
        assert!(url.contains(&urlencoding::encode(SCOPE_CALENDAR_EVENTS).to_string()));
        assert!(!url.contains("gmail"));
        assert!(url.contains("include_granted_scopes=true"));
    }

    #[test]
    fn granted_services_drops_services_whose_scopes_were_unticked() {
        let requested = [GoogleService::Gmail, GoogleService::Calendar];
        let only_gmail = GmailAccessTier::Full.scopes().join(" ");
        assert_eq!(
            granted_services(&requested, GmailAccessTier::Full, &only_gmail),
            vec![GoogleService::Gmail]
        );
        let all = scopes_for(&requested, GmailAccessTier::Full).join(" ");
        assert_eq!(
            granted_services(&requested, GmailAccessTier::Full, &all),
            requested.to_vec()
        );
    }
}
