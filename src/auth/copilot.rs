use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, RwLock};

static GITHUB_TOKEN_CACHE: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

fn cached_github_token() -> Option<String> {
    GITHUB_TOKEN_CACHE
        .read()
        .ok()
        .and_then(|value| value.clone())
}

fn cache_github_token(token: &str) {
    if let Ok(mut cache) = GITHUB_TOKEN_CACHE.write() {
        *cache = Some(token.to_string());
    }
}

pub fn invalidate_github_token_cache() {
    if let Ok(mut cache) = GITHUB_TOKEN_CACHE.write() {
        *cache = None;
    }
}

/// VSCode's OAuth client ID for GitHub Copilot device flow.
/// This is the well-known client ID used by VS Code, OpenCode, and other tools.
pub const GITHUB_COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// GitHub endpoints for Copilot auth
pub const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// Copilot API base URL
pub const COPILOT_API_BASE: &str = "https://api.githubcopilot.com";
pub const COPILOT_CONFIG_JSON_SOURCE_ID: &str = "copilot_config_json";
pub const COPILOT_HOSTS_AUTH_SOURCE_ID: &str = "copilot_hosts_json";
pub const COPILOT_APPS_AUTH_SOURCE_ID: &str = "copilot_apps_json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalCopilotAuthSource {
    ConfigJson,
    HostsJson,
    AppsJson,
    OpenCodeAuth,
    PiAuth,
}

impl ExternalCopilotAuthSource {
    pub fn source_id(self) -> &'static str {
        match self {
            Self::ConfigJson => COPILOT_CONFIG_JSON_SOURCE_ID,
            Self::HostsJson => COPILOT_HOSTS_AUTH_SOURCE_ID,
            Self::AppsJson => COPILOT_APPS_AUTH_SOURCE_ID,
            Self::OpenCodeAuth => crate::auth::external::OPENCODE_AUTH_JSON_SOURCE_ID,
            Self::PiAuth => crate::auth::external::PI_AUTH_JSON_SOURCE_ID,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ConfigJson => "GitHub Copilot CLI ~/.copilot/config.json",
            Self::HostsJson => "GitHub Copilot CLI hosts.json",
            Self::AppsJson => "GitHub Copilot apps.json",
            Self::OpenCodeAuth => "OpenCode auth.json",
            Self::PiAuth => "pi auth.json",
        }
    }

    pub fn path(self) -> PathBuf {
        match self {
            Self::ConfigJson => copilot_cli_dir().join("config.json"),
            Self::HostsJson => legacy_copilot_config_dir().join("hosts.json"),
            Self::AppsJson => legacy_copilot_config_dir().join("apps.json"),
            Self::OpenCodeAuth => crate::auth::external::ExternalAuthSource::OpenCode
                .path()
                .unwrap_or_default(),
            Self::PiAuth => crate::auth::external::ExternalAuthSource::Pi
                .path()
                .unwrap_or_default(),
        }
    }
}

/// Required headers for Copilot API requests
pub const EDITOR_VERSION: &str = "jcode/1.0";
pub const EDITOR_PLUGIN_VERSION: &str = "jcode/1.0";
pub const COPILOT_INTEGRATION_ID: &str = "vscode-chat";

/// Response from GitHub device code endpoint
#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Response from GitHub access token endpoint
#[derive(Debug, Deserialize)]
pub struct AccessTokenResponse {
    pub access_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Response from Copilot token exchange endpoint
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CopilotTokenResponse {
    pub token: String,
    pub expires_at: i64,
}

/// Cached Copilot API token with expiry
#[derive(Debug, Clone)]
pub struct CopilotApiToken {
    pub token: String,
    pub expires_at: i64,
}

impl CopilotApiToken {
    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        // Refresh 60 seconds before actual expiry
        now >= self.expires_at - 60
    }
}

/// Load a GitHub OAuth token from standard Copilot/CLI config locations.
///
/// Checks in order:
/// 1. COPILOT_GITHUB_TOKEN environment variable
/// 2. GH_TOKEN environment variable
/// 3. GITHUB_TOKEN environment variable
/// 4. ~/.copilot/config.json (official Copilot CLI plaintext fallback)
/// 5. ~/.config/github-copilot/hosts.json (legacy Copilot CLI)
/// 6. ~/.config/github-copilot/apps.json (legacy VS Code)
/// 7. trusted OpenCode/pi auth.json OAuth entries
/// 8. gh auth token fallback
pub fn load_github_token() -> Result<String> {
    if let Some(token) = cached_github_token() {
        return Ok(token);
    }

    for env_key in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(env_key)
            && !token.trim().is_empty()
        {
            let token = token.trim().to_string();
            cache_github_token(&token);
            return Ok(token);
        }
    }

    let config_path = ExternalCopilotAuthSource::ConfigJson.path();
    if crate::config::Config::external_auth_source_allowed_for_path(
        COPILOT_CONFIG_JSON_SOURCE_ID,
        &config_path,
    ) && let Ok(token) = load_token_from_config_json(&config_path)
    {
        cache_github_token(&token);
        return Ok(token);
    }

    let hosts_path = ExternalCopilotAuthSource::HostsJson.path();
    if crate::config::Config::external_auth_source_allowed_for_path(
        COPILOT_HOSTS_AUTH_SOURCE_ID,
        &hosts_path,
    ) && let Ok(token) = load_token_from_json(&hosts_path)
    {
        cache_github_token(&token);
        return Ok(token);
    }

    let apps_path = ExternalCopilotAuthSource::AppsJson.path();
    if crate::config::Config::external_auth_source_allowed_for_path(
        COPILOT_APPS_AUTH_SOURCE_ID,
        &apps_path,
    ) && let Ok(token) = load_token_from_json(&apps_path)
    {
        cache_github_token(&token);
        return Ok(token);
    }

    if let Some(token) = crate::auth::external::load_copilot_oauth_token() {
        cache_github_token(&token);
        return Ok(token);
    }

    if let Some(token) = load_token_from_gh_cli() {
        cache_github_token(&token);
        return Ok(token);
    }

    anyhow::bail!(
        "GitHub Copilot token not found. \
         Set COPILOT_GITHUB_TOKEN/GH_TOKEN/GITHUB_TOKEN, or run `gh auth login` / `gh extension install github/gh-copilot && gh copilot` \
         to authenticate."
    )
}

/// Check if Copilot credentials are available (without loading the full token)
pub fn has_copilot_credentials() -> bool {
    load_github_token().is_ok()
}

/// Fast local Copilot credential probe for startup-sensitive paths.
///
/// This intentionally avoids the `gh auth token` fallback because spawning the
/// GitHub CLI is too expensive for the fast auth snapshot.
pub fn has_copilot_credentials_fast() -> bool {
    use crate::auth::external::{ExternalAuthSource, source_has_copilot_oauth};

    for env_key in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(env_key)
            && !token.trim().is_empty()
        {
            cache_github_token(token.trim());
            return true;
        }
    }

    let config_path = ExternalCopilotAuthSource::ConfigJson.path();
    if config_path.exists()
        && crate::config::Config::external_auth_source_allowed_for_path_cached(
            COPILOT_CONFIG_JSON_SOURCE_ID,
            &config_path,
        )
        && let Ok(token) = load_token_from_config_json(&config_path)
    {
        cache_github_token(&token);
        return true;
    }

    let hosts_path = ExternalCopilotAuthSource::HostsJson.path();
    if hosts_path.exists()
        && crate::config::Config::external_auth_source_allowed_for_path_cached(
            COPILOT_HOSTS_AUTH_SOURCE_ID,
            &hosts_path,
        )
        && let Ok(token) = load_token_from_json(&hosts_path)
    {
        cache_github_token(&token);
        return true;
    }

    let apps_path = ExternalCopilotAuthSource::AppsJson.path();
    if apps_path.exists()
        && crate::config::Config::external_auth_source_allowed_for_path_cached(
            COPILOT_APPS_AUTH_SOURCE_ID,
            &apps_path,
        )
        && let Ok(token) = load_token_from_json(&apps_path)
    {
        cache_github_token(&token);
        return true;
    }

    for source in [ExternalAuthSource::OpenCode, ExternalAuthSource::Pi] {
        let Ok(path) = source.path() else {
            continue;
        };
        if !path.exists() {
            continue;
        }
        if crate::config::Config::external_auth_source_allowed_for_path_cached(
            source.source_id(),
            &path,
        ) && source_has_copilot_oauth(source)
        {
            if let Some(token) = crate::auth::external::load_copilot_oauth_token() {
                cache_github_token(&token);
            }
            return true;
        }
    }

    false
}

pub fn preferred_external_auth_source() -> Option<ExternalCopilotAuthSource> {
    [
        ExternalCopilotAuthSource::ConfigJson,
        ExternalCopilotAuthSource::HostsJson,
        ExternalCopilotAuthSource::AppsJson,
        ExternalCopilotAuthSource::OpenCodeAuth,
        ExternalCopilotAuthSource::PiAuth,
    ]
    .into_iter()
    .find(|source| match source {
        ExternalCopilotAuthSource::OpenCodeAuth => {
            let path = source.path();
            path.exists()
                && !crate::auth::external::source_allowed(
                    crate::auth::external::ExternalAuthSource::OpenCode,
                )
                && crate::auth::external::source_has_copilot_oauth(
                    crate::auth::external::ExternalAuthSource::OpenCode,
                )
        }
        ExternalCopilotAuthSource::PiAuth => {
            let path = source.path();
            path.exists()
                && !crate::auth::external::source_allowed(
                    crate::auth::external::ExternalAuthSource::Pi,
                )
                && crate::auth::external::source_has_copilot_oauth(
                    crate::auth::external::ExternalAuthSource::Pi,
                )
        }
        _ => source.path().exists(),
    })
}

pub fn has_unconsented_external_auth() -> Option<ExternalCopilotAuthSource> {
    let source = preferred_external_auth_source()?;
    let allowed = match source {
        ExternalCopilotAuthSource::OpenCodeAuth => crate::auth::external::source_allowed(
            crate::auth::external::ExternalAuthSource::OpenCode,
        ),
        ExternalCopilotAuthSource::PiAuth => {
            crate::auth::external::source_allowed(crate::auth::external::ExternalAuthSource::Pi)
        }
        _ => crate::config::Config::external_auth_source_allowed_for_path(
            source.source_id(),
            &source.path(),
        ),
    };
    if allowed { None } else { Some(source) }
}

pub fn trust_external_auth_source(source: ExternalCopilotAuthSource) -> Result<()> {
    match source {
        ExternalCopilotAuthSource::OpenCodeAuth => {
            crate::auth::external::trust_external_auth_source(
                crate::auth::external::ExternalAuthSource::OpenCode,
            )?;
        }
        ExternalCopilotAuthSource::PiAuth => {
            crate::auth::external::trust_external_auth_source(
                crate::auth::external::ExternalAuthSource::Pi,
            )?;
        }
        _ => {
            crate::config::Config::allow_external_auth_source_for_path(
                source.source_id(),
                &source.path(),
            )?;
        }
    }
    super::AuthStatus::invalidate_cache();
    Ok(())
}

fn copilot_cli_dir() -> PathBuf {
    if let Ok(path) = std::env::var("JCODE_HOME") {
        return PathBuf::from(path).join("external").join(".copilot");
    }

    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".copilot")
}

fn legacy_copilot_config_dir() -> PathBuf {
    if let Ok(path) = std::env::var("JCODE_HOME") {
        return PathBuf::from(path)
            .join("external")
            .join(".config")
            .join("github-copilot");
    }

    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("github-copilot")
    } else if cfg!(windows) {
        let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{}/AppData/Local", home)
        });
        PathBuf::from(local_app_data).join("github-copilot")
    } else {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".config").join("github-copilot")
    }
}

pub fn saved_hosts_path() -> PathBuf {
    legacy_copilot_config_dir().join("hosts.json")
}

fn load_token_from_config_json(path: &Path) -> Result<String> {
    let path = crate::storage::validate_external_auth_file(path)?;
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&data)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    find_token_in_value(&value)
        .ok_or_else(|| anyhow::anyhow!("No GitHub token found in {}", path.display()))
}

fn find_token_in_value(value: &Value) -> Option<String> {
    match value {
        Value::String(token) => normalize_candidate_token(token),
        Value::Array(items) => items.iter().find_map(find_token_in_value),
        Value::Object(map) => {
            for key in ["oauth_token", "token", "github_token", "access_token"] {
                if let Some(token) = map.get(key).and_then(find_token_in_value) {
                    return Some(token);
                }
            }
            map.values().find_map(find_token_in_value)
        }
        _ => None,
    }
}

fn normalize_candidate_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }

    if token.starts_with("gho_")
        || token.starts_with("ghu_")
        || token.starts_with("github_pat_")
        || token.starts_with("ghs_")
    {
        return Some(token.to_string());
    }

    None
}

fn load_token_from_gh_cli() -> Option<String> {
    if !crate::auth::command_exists("gh") {
        return None;
    }

    let output = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let token = String::from_utf8(output.stdout).ok()?;
    normalize_candidate_token(&token)
}

/// Parse a Copilot config JSON file to extract the oauth_token.
/// Format: { "github.com": { "oauth_token": "gho_xxxx", "user": "..." } }
fn load_token_from_json(path: &Path) -> Result<String> {
    let path = crate::storage::validate_external_auth_file(path)?;
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let config: HashMap<String, HashMap<String, serde_json::Value>> =
        serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse {}", path.display()))?;

    let token = select_preferred_token(&config)
        .ok_or_else(|| anyhow::anyhow!("No oauth_token found in {}", path.display()))?;

    Ok(token.clone())
}

fn select_preferred_token(
    config: &HashMap<String, HashMap<String, serde_json::Value>>,
) -> Option<&String> {
    config
        .iter()
        .filter_map(|(host, value)| {
            let token = match value.get("oauth_token") {
                Some(serde_json::Value::String(token)) if !token.is_empty() => token,
                _ => return None,
            };

            let normalized_host = normalize_github_host_key(host)?;
            let raw_host = host.trim().to_ascii_lowercase();
            Some((
                github_host_priority(&raw_host, &normalized_host),
                normalized_host,
                raw_host,
                token,
            ))
        })
        .min_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        })
        .map(|(_, _, _, token)| token)
}

fn github_host_priority(raw_host: &str, normalized_host: &str) -> u8 {
    if raw_host == "github.com" {
        0
    } else if normalized_host == "github.com" {
        1
    } else if raw_host == "api.github.com" {
        2
    } else if normalized_host == "api.github.com" {
        3
    } else {
        4
    }
}

fn normalize_github_host_key(host: &str) -> Option<String> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }

    let host = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))
        .unwrap_or(host)
        .trim_end_matches('/');
    let host = host.split('/').next().unwrap_or_default().trim();
    let host = host.to_ascii_lowercase();

    if host == "github.com" || host == "api.github.com" || host.ends_with(".github.com") {
        Some(host)
    } else {
        None
    }
}

/// Exchange a GitHub OAuth token for a short-lived Copilot API bearer token.
pub async fn exchange_github_token(
    client: &reqwest::Client,
    github_token: &str,
) -> Result<CopilotApiToken> {
    let resp = client
        .get(COPILOT_TOKEN_URL)
        .header("Authorization", format!("Token {}", github_token))
        .header("User-Agent", EDITOR_VERSION)
        .send()
        .await
        .context("Failed to exchange GitHub token for Copilot token")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = crate::util::http_error_body(resp, "HTTP error").await;
        anyhow::bail!(format_copilot_exchange_failure(status, &body));
    }

    let token_resp: CopilotTokenResponse = resp
        .json()
        .await
        .context("Failed to parse Copilot token response")?;

    Ok(CopilotApiToken {
        token: token_resp.token,
        expires_at: token_resp.expires_at,
    })
}

/// Build the user-facing error string for a non-success Copilot token exchange.
///
/// GitHub deliberately returns 404 (not 403) on `copilot_internal/v2/token` and
/// `user/copilot_billing` when the account has no active Copilot subscription —
/// the raw `{"message":"Not Found"}` body is opaque, so we surface the actual
/// likely cause and remediation. Other statuses fall through to a generic
/// message that still includes the standalone status code so the failover
/// machinery (which scans for "401"/"403"/"token exchange failed") keeps
/// behaving correctly.
fn format_copilot_exchange_failure(status: reqwest::StatusCode, body: &str) -> String {
    let trimmed = body.trim();
    let body_for_display = if trimmed.is_empty() {
        "(empty body)".to_string()
    } else {
        trimmed.to_string()
    };

    if status == reqwest::StatusCode::NOT_FOUND && trimmed.contains("\"message\":\"Not Found\"") {
        return format!(
            "Copilot token exchange failed (HTTP 404 Not Found): the GitHub account this token \
             belongs to does not have an active Copilot subscription. GitHub returns 404 instead \
             of 403 here to hide the endpoint from non-Copilot users. \
             Subscribe at https://github.com/settings/copilot, switch accounts via \
             `gh auth login` (or set GH_TOKEN/COPILOT_GITHUB_TOKEN), or pick another provider \
             with `jcode --provider claude` / `--provider openai`. Raw response: {}",
            body_for_display
        );
    }

    format!(
        "Copilot token exchange failed (HTTP {}): {}",
        status, body_for_display
    )
}

/// Initiate GitHub OAuth device flow for Copilot authentication.
/// Returns the device code response with user instructions.
pub async fn initiate_device_flow(client: &reqwest::Client) -> Result<DeviceCodeResponse> {
    let resp = client
        .post(GITHUB_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", GITHUB_COPILOT_CLIENT_ID),
            ("scope", "read:user"),
        ])
        .send()
        .await
        .context("Failed to initiate GitHub device flow")?;

    if !resp.status().is_success() {
        let body = crate::util::http_error_body(resp, "HTTP error").await;
        anyhow::bail!("GitHub device flow failed: {}", body);
    }

    resp.json::<DeviceCodeResponse>()
        .await
        .context("Failed to parse device code response")
}

/// Poll for the access token after user has authorized the device.
/// Returns the GitHub OAuth token (gho_xxx format).
pub async fn poll_for_access_token(
    client: &reqwest::Client,
    device_code: &str,
    interval: u64,
) -> Result<String> {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

        let resp = client
            .post(GITHUB_ACCESS_TOKEN_URL)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", GITHUB_COPILOT_CLIENT_ID),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .context("Failed to poll for access token")?;

        let token_resp: AccessTokenResponse = resp
            .json()
            .await
            .context("Failed to parse access token response")?;

        if let Some(token) = token_resp.access_token {
            return Ok(token);
        }

        match token_resp.error.as_deref() {
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            Some("expired_token") => {
                anyhow::bail!("Device code expired. Please try again.");
            }
            Some("access_denied") => {
                anyhow::bail!("Authorization was denied by the user.");
            }
            Some(err) => {
                let desc = token_resp.error_description.unwrap_or_default();
                anyhow::bail!("GitHub auth error: {} - {}", err, desc);
            }
            None => {
                anyhow::bail!("Unexpected response from GitHub");
            }
        }
    }
}

/// Save a GitHub OAuth token to the standard Copilot config location.
pub fn save_github_token(token: &str, username: &str) -> Result<()> {
    let config_dir = legacy_copilot_config_dir();
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("Failed to create {}", config_dir.display()))?;
    crate::platform::set_directory_permissions_owner_only(&config_dir)
        .with_context(|| format!("Failed to secure {}", config_dir.display()))?;

    let hosts_path = config_dir.join("hosts.json");

    let mut config: HashMap<String, HashMap<String, String>> =
        if let Ok(data) = std::fs::read_to_string(&hosts_path) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

    let mut entry = HashMap::new();
    entry.insert("user".to_string(), username.to_string());
    entry.insert("oauth_token".to_string(), token.to_string());
    config.insert("github.com".to_string(), entry);

    let json = serde_json::to_string_pretty(&config)?;
    crate::storage::write_text_secret(&hosts_path, &json)
        .with_context(|| format!("Failed to write {}", hosts_path.display()))?;

    // A token written by jcode's own device-login flow should be immediately
    // usable in future sessions. Without this, later reads treat the saved
    // hosts.json as an untrusted external auth source and appear to "lose"
    // the Copilot login after restart/new session.
    crate::config::Config::allow_external_auth_source_for_path(
        COPILOT_HOSTS_AUTH_SOURCE_ID,
        &hosts_path,
    )?;
    super::AuthStatus::invalidate_cache();

    Ok(())
}

/// Copilot account type - determines API base URL and available models
#[derive(Debug, Clone, PartialEq)]
pub enum CopilotAccountType {
    Individual,
    Business,
    Enterprise,
    Unknown,
}

impl std::fmt::Display for CopilotAccountType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopilotAccountType::Individual => write!(f, "individual"),
            CopilotAccountType::Business => write!(f, "business"),
            CopilotAccountType::Enterprise => write!(f, "enterprise"),
            CopilotAccountType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Information about the user's Copilot subscription
#[derive(Debug, Clone)]
pub struct CopilotSubscriptionInfo {
    pub account_type: CopilotAccountType,
    pub available_models: Vec<CopilotModelInfo>,
}

/// Model info from the Copilot /models endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct CopilotModelInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub model_picker_enabled: bool,
    #[serde(default)]
    pub capabilities: Option<CopilotModelCapabilities>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CopilotModelCapabilities {
    #[serde(default)]
    pub limits: Option<CopilotModelLimits>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CopilotModelLimits {
    #[serde(default)]
    pub max_context_window_tokens: Option<usize>,
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<CopilotModelInfo>,
}

/// Fetch available models from the Copilot API.
pub async fn fetch_available_models(
    client: &reqwest::Client,
    bearer_token: &str,
) -> Result<Vec<CopilotModelInfo>> {
    let resp = client
        .get(format!("{}/models", COPILOT_API_BASE))
        .header("Authorization", format!("Bearer {}", bearer_token))
        .header("Editor-Version", EDITOR_VERSION)
        .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
        .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
        .send()
        .await
        .context("Failed to fetch Copilot models")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = crate::util::http_error_body(resp, "HTTP error").await;
        anyhow::bail!("Copilot models fetch failed (HTTP {}): {}", status, body);
    }

    let models_resp: ModelsResponse = resp
        .json()
        .await
        .context("Failed to parse Copilot models response")?;

    Ok(models_resp.data)
}

/// Determine the best default model based on available models.
/// - If claude-opus-4.6 is available -> paid tier -> use claude-opus-4.6
/// - Otherwise -> free/basic tier -> use claude-sonnet-4.6 or claude-sonnet-4
pub fn choose_default_model(available_models: &[CopilotModelInfo]) -> String {
    let model_ids: Vec<&str> = available_models.iter().map(|m| m.id.as_str()).collect();

    if model_ids.contains(&"claude-opus-4.6") {
        "claude-opus-4.6".to_string()
    } else if model_ids.contains(&"claude-sonnet-4.6") {
        "claude-sonnet-4.6".to_string()
    } else {
        "claude-sonnet-4".to_string()
    }
}

/// Fetch the authenticated GitHub username using an OAuth token.
pub async fn fetch_github_username(client: &reqwest::Client, token: &str) -> Result<String> {
    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", EDITOR_VERSION)
        .send()
        .await
        .context("Failed to fetch GitHub user")?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to fetch GitHub user (HTTP {})", resp.status());
    }

    #[derive(Deserialize)]
    struct GithubUser {
        login: String,
    }

    let user: GithubUser = resp.json().await.context("Failed to parse GitHub user")?;
    Ok(user.login)
}

#[cfg(test)]
#[path = "copilot_auth_tests.rs"]
mod tests;
