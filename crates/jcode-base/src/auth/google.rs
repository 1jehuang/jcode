use anyhow::Result;
use serde::{Deserialize, Serialize};

pub mod setup;

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

    /// Google Cloud service name of this product's API, for `gcloud services
    /// enable` and console enable links.
    pub fn api_service_name(&self) -> &'static str {
        match self {
            GoogleService::Gmail => "gmail.googleapis.com",
            GoogleService::Calendar => "calendar-json.googleapis.com",
        }
    }

    /// Google Cloud Console library page for enabling this service's API.
    pub fn api_library_url(&self) -> String {
        format!(
            "https://console.cloud.google.com/apis/library/{}",
            self.api_service_name()
        )
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The OAuth client that issued this grant. Bundling it with the tokens
    /// makes client replacement a single atomic secret-file write. Older files
    /// fall back to google_credentials.json.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<GoogleCredentials>,
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
    load_credentials_from_paths(&credentials_path()?, &tokens_path()?)
}

fn load_credentials_from_paths(
    credentials_path: &std::path::Path,
    tokens_path: &std::path::Path,
) -> Result<GoogleCredentials> {
    crate::storage::harden_secret_file_permissions(tokens_path);
    if tokens_path.exists()
        && let Ok(tokens) = crate::storage::read_json::<GoogleTokens>(tokens_path)
        && let Some(creds) = tokens.credentials
    {
        return Ok(creds);
    }
    load_legacy_credentials(credentials_path)
}

fn load_legacy_credentials(path: &std::path::Path) -> Result<GoogleCredentials> {
    crate::storage::harden_secret_file_permissions(path);
    let data = match std::fs::read_to_string(path) {
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
    login_with_credentials(&load_credentials()?, services, tier, no_browser).await
}

/// Authorize a staged client without changing the active login until success.
pub async fn login_with_credentials(
    creds: &GoogleCredentials,
    services: &[GoogleService],
    tier: GmailAccessTier,
    no_browser: bool,
) -> Result<GoogleTokens> {
    let (verifier, challenge) = super::oauth::generate_pkce_public();
    let state = super::oauth::generate_state_public();

    let listener = super::oauth::bind_callback_listener(0).ok();
    let redirect_uri = listener
        .as_ref()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr| format!("http://127.0.0.1:{}", addr.port()))
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", DEFAULT_PORT));

    let auth_url = build_auth_url(creds, services, tier, &redirect_uri, &challenge, &state);

    eprintln!("\nOpening browser for Google login...\n");
    eprintln!("If the browser didn't open, visit:\n{}\n", auth_url);
    if let Some(qr) = crate::auth::login_ui::qr_section(
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
        crate::companion_client::open_url(&auth_url)
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
    persist_login(
        &tokens_path()?,
        creds,
        exchange_code(creds, &verifier, &code, &redirect_uri, services, tier),
    )
    .await
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
    persist_login(
        &tokens_path()?,
        creds,
        exchange_code(creds, verifier, &code, redirect_uri, services, tier),
    )
    .await
}

#[cfg(any(test, feature = "test-support"))]
tokio::task_local! {
    static TEST_TOKEN_EXCHANGE: (
        GoogleCredentials,
        std::result::Result<GoogleTokens, String>,
    );
}

/// Run a synthetic token endpoint response in this task only. The expected
/// client assertion catches accidental use of an old grant's bound client.
/// No endpoint override or global transport state exists in production.
#[cfg(any(test, feature = "test-support"))]
pub async fn with_test_token_exchange<F: std::future::Future>(
    expected_client: GoogleCredentials,
    response: std::result::Result<GoogleTokens, String>,
    future: F,
) -> F::Output {
    TEST_TOKEN_EXCHANGE
        .scope((expected_client, response), future)
        .await
}

async fn exchange_code(
    creds: &GoogleCredentials,
    verifier: &str,
    code: &str,
    redirect_uri: &str,
    services: &[GoogleService],
    tier: GmailAccessTier,
) -> Result<GoogleTokens> {
    #[cfg(any(test, feature = "test-support"))]
    if let Ok((expected_client, response)) = TEST_TOKEN_EXCHANGE.try_with(Clone::clone) {
        anyhow::ensure!(
            creds == &expected_client,
            "Unexpected synthetic Google OAuth client"
        );
        return response.map_err(anyhow::Error::msg);
    }

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
        credentials: None,
    };

    Ok(tokens)
}

/// No active file is touched while the exchange is pending, fails, or is
/// cancelled. Once it succeeds there is no further await before the atomic
/// write of the matching client and token pair.
async fn persist_login(
    path: &std::path::Path,
    creds: &GoogleCredentials,
    exchange: impl std::future::Future<Output = Result<GoogleTokens>>,
) -> Result<GoogleTokens> {
    let mut tokens = exchange.await?;
    tokens.credentials = Some(creds.clone());
    crate::storage::write_json_secret(path, &tokens)?;
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

/// Refresh must use the client attached to this exact grant, not whichever
/// client a concurrent login selected. Unbound legacy files retain their old
/// separate-credentials fallback.
fn credentials_for_tokens(
    tokens: &GoogleTokens,
    legacy_path: &std::path::Path,
) -> Result<GoogleCredentials> {
    match &tokens.credentials {
        Some(creds) => Ok(creds.clone()),
        None => load_legacy_credentials(legacy_path),
    }
}

async fn refresh_tokens_uncoordinated(tokens: &GoogleTokens) -> Result<GoogleTokens> {
    let result: Result<GoogleTokens> = async {
        let creds = credentials_for_tokens(tokens, &credentials_path()?)?;
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
            credentials: Some(creds),
        };

        save_tokens(&new_tokens)?;
        Ok(new_tokens)
    }
    .await;

    // Shared recorder: a permanently rejected refresh token becomes terminal
    // so background sweeps stop retrying it; transient failures stay retryable.
    crate::auth::refresh_state::record_refresh_outcome("google", &tokens.refresh_token, &result);

    // An expired grant on a self-made app almost always means it was left in
    // Testing mode, where Google expires logins after 7 days.
    result.map_err(|err| {
        if setup::looks_like_expired_grant(&format!("{err:#}")) {
            err.context(format!(
                "Google login expired or was revoked. {} Then run `jcode login google`. {}",
                setup::PUBLISH_APP_NOTE,
                setup::audience_url(None)
            ))
        } else {
            err
        }
    })
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

    fn synthetic_client(project: &str) -> GoogleCredentials {
        GoogleCredentials {
            client_id: format!("synthetic-{project}.apps.googleusercontent.com"),
            client_secret: format!("synthetic-{project}-secret"),
        }
    }

    fn synthetic_tokens(project: &str) -> GoogleTokens {
        GoogleTokens {
            access_token: format!("synthetic-{project}-access"),
            refresh_token: format!("synthetic-{project}-refresh"),
            expires_at: 4_102_444_800_000,
            tier: GmailAccessTier::Full,
            email: None,
            services: vec![GoogleService::Gmail],
            credentials: None,
        }
    }

    struct LoginFiles {
        _dir: tempfile::TempDir,
        client: std::path::PathBuf,
        tokens: std::path::PathBuf,
        original_client: Vec<u8>,
        original_tokens: Vec<u8>,
    }

    impl LoginFiles {
        fn new(bound: bool) -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            let client = dir.path().join("google_credentials.json");
            let tokens = dir.path().join("google_oauth.json");
            let old_client = synthetic_client("old-project");
            let mut old_tokens = synthetic_tokens("old-project");
            if bound {
                old_tokens.credentials = Some(old_client.clone());
            }
            crate::storage::write_json_secret(&client, &old_client).unwrap();
            crate::storage::write_json_secret(&tokens, &old_tokens).unwrap();
            let original_client = std::fs::read(&client).unwrap();
            let original_tokens = std::fs::read(&tokens).unwrap();
            Self {
                _dir: dir,
                client,
                tokens,
                original_client,
                original_tokens,
            }
        }

        fn assert_unchanged(&self) {
            assert_eq!(std::fs::read(&self.client).unwrap(), self.original_client);
            assert_eq!(std::fs::read(&self.tokens).unwrap(), self.original_tokens);
            assert_eq!(
                load_credentials_from_paths(&self.client, &self.tokens).unwrap(),
                synthetic_client("old-project")
            );
        }
    }

    #[tokio::test]
    async fn transactional_google_login_exchange_failure_preserves_active_files() {
        for bound in [false, true] {
            let files = LoginFiles::new(bound);
            let new_client = synthetic_client("other-project");
            // Fake transport: an OAuth denial or token-endpoint failure. No HTTP.
            let exchange = std::future::ready(Err(anyhow::anyhow!("synthetic access_denied")));
            let error = persist_login(&files.tokens, &new_client, exchange)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("access_denied"));
            files.assert_unchanged();
        }
    }

    #[tokio::test]
    async fn transactional_google_login_cancel_preserves_active_files() {
        for bound in [false, true] {
            let files = LoginFiles::new(bound);
            let new_client = synthetic_client("other-project");
            {
                // Poll the real commit wrapper while a fake transport is waiting,
                // then drop it, exactly as task cancellation does.
                let exchange = std::future::pending::<Result<GoogleTokens>>();
                let login = persist_login(&files.tokens, &new_client, exchange);
                tokio::pin!(login);
                tokio::select! {
                    biased;
                    _ = &mut login => panic!("pending exchange completed"),
                    _ = std::future::ready(()) => {}
                }
                files.assert_unchanged();
            }
            files.assert_unchanged();
        }
    }

    #[tokio::test]
    async fn transactional_google_login_success_commits_matching_pair() {
        for bound in [false, true] {
            let files = LoginFiles::new(bound);
            let new_client = synthetic_client("other-project");
            let exchange = std::future::ready(Ok(synthetic_tokens("other-project")));
            let committed = persist_login(&files.tokens, &new_client, exchange)
                .await
                .unwrap();
            let stored: GoogleTokens = crate::storage::read_json(&files.tokens).unwrap();
            assert_eq!(stored.refresh_token, "synthetic-other-project-refresh");
            assert_eq!(stored.credentials.as_ref(), Some(&new_client));
            assert_eq!(committed.credentials.as_ref(), Some(&new_client));
            assert_eq!(
                load_credentials_from_paths(&files.client, &files.tokens).unwrap(),
                new_client
            );
            // The old separate client file need not change: the authoritative
            // matching pair is now in one atomically replaced secret file.
            assert_eq!(std::fs::read(&files.client).unwrap(), files.original_client);
            assert_eq!(
                credentials_for_tokens(&stored, &files.client).unwrap(),
                new_client
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&files.tokens)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
    }

    #[test]
    fn transactional_google_login_refresh_uses_bound_client_or_legacy_fallback() {
        let files = LoginFiles::new(false);
        let mut tokens = synthetic_tokens("other-project");
        assert_eq!(
            credentials_for_tokens(&tokens, &files.client).unwrap(),
            synthetic_client("old-project")
        );
        tokens.credentials = Some(synthetic_client("other-project"));
        assert_eq!(
            credentials_for_tokens(&tokens, &files.client).unwrap(),
            synthetic_client("other-project")
        );
        // A bound grant remains usable even if the legacy client file is gone.
        assert_eq!(
            credentials_for_tokens(&tokens, &files._dir.path().join("absent.json")).unwrap(),
            synthetic_client("other-project")
        );
    }

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
