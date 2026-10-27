use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, RwLock};

/// Cached GitHub token resolved from file or subprocess sources, with the
/// time it was cached. Env vars are intentionally NOT served from this cache:
/// they are cheap to read and must take effect immediately when they change.
/// The TTL bounds how long a deleted/changed credential file keeps working.
static GITHUB_TOKEN_CACHE: LazyLock<RwLock<Option<CachedGithubToken>>> =
    LazyLock::new(|| RwLock::new(None));
const GITHUB_TOKEN_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
const FAILED_VALIDATION_AUTO_USE_TTL_MS: i64 = 24 * 60 * 60 * 1000;

struct CachedGithubToken {
    host: String,
    token: String,
    cached_at: std::time::Instant,
}

fn cached_github_token(host: &str) -> Option<String> {
    GITHUB_TOKEN_CACHE.read().ok().and_then(|value| {
        value.as_ref().and_then(|cached| {
            (cached.host == host && cached.cached_at.elapsed() < GITHUB_TOKEN_CACHE_TTL)
                .then(|| cached.token.clone())
        })
    })
}

fn cache_github_token(host: &str, token: &str) {
    if let Ok(mut cache) = GITHUB_TOKEN_CACHE.write() {
        *cache = Some(CachedGithubToken {
            host: host.to_string(),
            token: token.to_string(),
            cached_at: std::time::Instant::now(),
        });
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
const PUBLIC_GITHUB_HOST: &str = "github.com";

/// Host-scoped endpoints shared by login, token refresh, catalogs, and inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotEndpoints {
    pub host: String,
    pub device_code_url: String,
    pub access_token_url: String,
    pub token_url: String,
    pub github_api_base: String,
    pub api_base: String,
}

impl CopilotEndpoints {
    pub fn load() -> Result<Self> {
        let config = crate::config::Config::load_strict()?;
        Self::from_enterprise_url(config.provider.copilot_enterprise_url.as_deref())
    }

    pub fn from_enterprise_url(enterprise_url: Option<&str>) -> Result<Self> {
        let Some(raw) = enterprise_url else {
            return Ok(Self {
                host: PUBLIC_GITHUB_HOST.to_string(),
                device_code_url: GITHUB_DEVICE_CODE_URL.to_string(),
                access_token_url: GITHUB_ACCESS_TOKEN_URL.to_string(),
                token_url: COPILOT_TOKEN_URL.to_string(),
                github_api_base: "https://api.github.com".to_string(),
                api_base: COPILOT_API_BASE.to_string(),
            });
        };
        let host = raw
            .strip_prefix("https://")
            .unwrap_or(raw)
            .trim_end_matches('/');
        anyhow::ensure!(
            !host.is_empty()
                && !host.chars().any(|c| c.is_whitespace() || c.is_control())
                && !host.contains(['/', '\\', '@', '?', '#', ':']),
            "provider.copilot_enterprise_url must be an HTTPS hostname without a path or port"
        );
        let url = reqwest::Url::parse(&format!("https://{host}"))
            .context("Invalid provider.copilot_enterprise_url")?;
        let host = url
            .host_str()
            .context("Missing Copilot enterprise hostname")?;
        anyhow::ensure!(
            host.contains('.') && matches!(url::Host::parse(host)?, url::Host::Domain(_)),
            "provider.copilot_enterprise_url must be a DNS hostname"
        );
        if host == PUBLIC_GITHUB_HOST {
            return Self::from_enterprise_url(None);
        }
        let github_base = format!("https://{host}");
        let github_api_base = format!("{github_base}/api/v3");
        Ok(Self {
            host: host.to_string(),
            device_code_url: format!("{github_base}/login/device/code"),
            access_token_url: format!("{github_base}/login/oauth/access_token"),
            token_url: format!("{github_api_base}/copilot_internal/v2/token"),
            github_api_base,
            api_base: format!("https://copilot-api.{host}"),
        })
    }

    pub fn is_enterprise(&self) -> bool {
        self.host != PUBLIC_GITHUB_HOST
    }
}

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
/// 8. optional `gh auth token` fallback when JCODE_COPILOT_ALLOW_GH_AUTH_TOKEN=1
pub fn load_github_token() -> Result<String> {
    load_github_token_with_endpoints(&CopilotEndpoints::load()?)
}

pub fn load_github_token_with_endpoints(endpoints: &CopilotEndpoints) -> Result<String> {
    load_github_token_inner(endpoints, true)
}

fn load_github_token_inner(
    endpoints: &CopilotEndpoints,
    should_allow_gh_cli: bool,
) -> Result<String> {
    // Env vars first: cheap to read and they must win immediately when the
    // user changes them, so they are never served from (or shadowed by) the
    // file-source cache below.
    for env_key in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(env_key)
            && !token.trim().is_empty()
        {
            return Ok(token.trim().to_string());
        }
    }

    if let Some(token) = cached_github_token(&endpoints.host) {
        return Ok(token);
    }

    let config_path = ExternalCopilotAuthSource::ConfigJson.path();
    if crate::config::Config::external_auth_source_allowed_for_path(
        COPILOT_CONFIG_JSON_SOURCE_ID,
        &config_path,
    ) && let Ok(token) = load_token_from_config_json_with_host(&config_path, endpoints)
    {
        cache_github_token(&endpoints.host, &token);
        return Ok(token);
    }

    let hosts_path = ExternalCopilotAuthSource::HostsJson.path();
    if crate::config::Config::external_auth_source_allowed_for_path(
        COPILOT_HOSTS_AUTH_SOURCE_ID,
        &hosts_path,
    ) && let Ok(token) = load_token_from_json_with_host(&hosts_path, &endpoints.host)
    {
        cache_github_token(&endpoints.host, &token);
        return Ok(token);
    }

    let apps_path = ExternalCopilotAuthSource::AppsJson.path();
    if crate::config::Config::external_auth_source_allowed_for_path(
        COPILOT_APPS_AUTH_SOURCE_ID,
        &apps_path,
    ) && let Ok(token) = load_token_from_json_with_host(&apps_path, &endpoints.host)
    {
        cache_github_token(&endpoints.host, &token);
        return Ok(token);
    }

    if !endpoints.is_enterprise()
        && let Some(token) = crate::auth::external::load_copilot_oauth_token()
    {
        cache_github_token(&endpoints.host, &token);
        return Ok(token);
    }

    if should_allow_gh_cli
        && allow_gh_cli_fallback()
        && let Some(token) = load_token_from_gh_cli(&endpoints.host)
    {
        cache_github_token(&endpoints.host, &token);
        return Ok(token);
    }

    anyhow::bail!(
        "GitHub Copilot token not found. \
         Set COPILOT_GITHUB_TOKEN/GH_TOKEN/GITHUB_TOKEN, run `jcode login --provider copilot`, \
         or set JCODE_COPILOT_ALLOW_GH_AUTH_TOKEN=1 to explicitly reuse `gh auth token`."
    )
}

fn allow_gh_cli_fallback() -> bool {
    std::env::var("JCODE_COPILOT_ALLOW_GH_AUTH_TOKEN")
        .ok()
        .map(|value| {
            let value = value.trim();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

fn copilot_env_token_present() -> bool {
    ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"]
        .into_iter()
        .any(|env_key| {
            std::env::var(env_key)
                .ok()
                .map(|token| !token.trim().is_empty())
                .unwrap_or(false)
        })
}

/// Return true when a recent `auth-test` proved the discovered Copilot token is
/// not exchangeable for a Copilot API token.
///
/// Copilot is unusual because a local GitHub OAuth token can exist while the
/// account is not entitled to Copilot, or the token is otherwise rejected by the
/// Copilot token service. Presence-only checks are still useful for explicit
/// diagnostics, but they must not cause startup/default-provider selection to
/// silently choose Copilot as a usable provider after a known token-exchange
/// failure. Environment tokens are treated as an explicit override because they
/// may be a newly supplied credential that is not represented by the saved
/// validation record.
pub fn validation_failure_blocks_auto_use() -> bool {
    if copilot_env_token_present() {
        return false;
    }

    let Some(record) = crate::auth::validation::get("copilot") else {
        return false;
    };
    if record.success {
        return false;
    }

    let age_ms = chrono::Utc::now()
        .timestamp_millis()
        .saturating_sub(record.checked_at_ms);
    if age_ms > FAILED_VALIDATION_AUTO_USE_TTL_MS {
        return false;
    }

    let summary = record.summary.to_ascii_lowercase();
    summary.contains("copilot token exchange failed")
        && (summary.contains("http 401")
            || summary.contains("http 403")
            || summary.contains("feature_flag_blocked")
            || summary.contains("resource not accessible"))
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
    CopilotEndpoints::load()
        .and_then(|endpoints| load_github_token_inner(&endpoints, false))
        .is_ok()
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

#[cfg(test)]
fn load_token_from_config_json(path: &Path) -> Result<String> {
    load_token_from_config_json_with_host(path, &CopilotEndpoints::from_enterprise_url(None)?)
}

fn load_token_from_config_json_with_host(
    path: &Path,
    endpoints: &CopilotEndpoints,
) -> Result<String> {
    let path = crate::storage::validate_external_auth_file(path)?;
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&data)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    let token = if endpoints.is_enterprise() {
        find_enterprise_token(&value, &endpoints.host)
    } else {
        find_token_in_value(&value)
    };
    token.ok_or_else(|| anyhow::anyhow!("No GitHub token found in {}", path.display()))
}

fn find_enterprise_token(value: &Value, host: &str) -> Option<String> {
    if let Some(entry) = value.get(host) {
        return find_token_in_value(entry);
    }
    value
        .get("logged_in_users")?
        .as_array()?
        .iter()
        .find_map(|entry| {
            let entry_host = entry.get("github_host")?.as_str()?;
            (entry_host.trim_end_matches('/') == host
                || entry_host.trim_end_matches('/') == format!("https://{host}"))
            .then(|| find_token_in_value(entry))
            .flatten()
        })
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

fn load_token_from_gh_cli(host: &str) -> Option<String> {
    if !crate::auth::command_exists("gh") {
        return None;
    }

    let output = Command::new("gh")
        .args(["auth", "token", "--hostname", host])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let token = String::from_utf8(output.stdout).ok()?;
    normalize_candidate_token(&token)
}

/// Parse a Copilot config JSON file to extract the oauth_token.
/// Format: { "github.com": { "oauth_token": "gho_xxxx", "user": "..." } }
#[cfg(test)]
fn load_token_from_json(path: &Path) -> Result<String> {
    load_token_from_json_with_host(path, PUBLIC_GITHUB_HOST)
}

fn load_token_from_json_with_host(path: &Path, host: &str) -> Result<String> {
    let path = crate::storage::validate_external_auth_file(path)?;
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let config: HashMap<String, HashMap<String, serde_json::Value>> =
        serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse {}", path.display()))?;

    let token = select_preferred_token(&config, host)
        .ok_or_else(|| anyhow::anyhow!("No oauth_token found in {}", path.display()))?;

    Ok(token.clone())
}

fn select_preferred_token<'a>(
    config: &'a HashMap<String, HashMap<String, serde_json::Value>>,
    selected_host: &str,
) -> Option<&'a String> {
    config
        .iter()
        .filter_map(|(host, value)| {
            let token = match value.get("oauth_token") {
                Some(serde_json::Value::String(token)) if !token.is_empty() => token,
                _ => return None,
            };

            let normalized_host = if selected_host == PUBLIC_GITHUB_HOST {
                normalize_github_host_key(host)?
            } else {
                let key = host.trim().strip_prefix("https://").unwrap_or(host.trim());
                let key = key.trim_end_matches('/');
                let key = key
                    .strip_suffix(&format!(":{GITHUB_COPILOT_CLIENT_ID}"))
                    .unwrap_or(key);
                if !key.eq_ignore_ascii_case(selected_host) {
                    return None;
                }
                selected_host.to_string()
            };
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
    // GitHub's `apps.json` keys carry a client-id suffix
    // (`github.com:Iv1.b507a08c87ecfe98`), so the host must also be split on
    // ':' or valid credentials are silently discarded (issue #641).
    // `split` always yields at least one element, so this cannot lose the host.
    let host = host.split(':').next().unwrap_or(host).trim();
    let host = host.to_ascii_lowercase();

    if host == "github.com" || host == "api.github.com" || host.ends_with(".github.com") {
        Some(host)
    } else {
        None
    }
}

/// Maximum attempts for the Copilot token exchange when GitHub returns a
/// transient 5xx (e.g. a 502 from a bad gateway).
const TOKEN_EXCHANGE_MAX_ATTEMPTS: u32 = 5;

/// Backoff delay (milliseconds) before retry attempt `attempt` (1-based count
/// of completed attempts). Exponential: 500ms, 1s, 2s, capped at 4s.
pub(crate) fn token_exchange_backoff_ms(attempt: u32) -> u64 {
    let base: u64 = 500;
    base.saturating_mul(1u64 << attempt.saturating_sub(1).min(6))
        .min(4_000)
}

/// Whether an HTTP status from the token exchange is worth retrying.
pub(crate) fn token_exchange_retryable_status(status: u16) -> bool {
    (500..600).contains(&status)
}

/// Exchange a GitHub OAuth token for a short-lived Copilot API bearer token.
///
/// GitHub's token service occasionally returns transient 5xx responses
/// (see issue #548); those are retried with exponential backoff before
/// failing. 4xx responses (bad/unauthorized token) fail immediately.
pub async fn exchange_github_token(
    client: &reqwest::Client,
    github_token: &str,
) -> Result<CopilotApiToken> {
    exchange_github_token_with_endpoints(client, github_token, &CopilotEndpoints::load()?).await
}

pub async fn exchange_github_token_with_endpoints(
    client: &reqwest::Client,
    github_token: &str,
    endpoints: &CopilotEndpoints,
) -> Result<CopilotApiToken> {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let resp = client
            .get(&endpoints.token_url)
            .header("Authorization", format!("Token {}", github_token))
            .header("User-Agent", EDITOR_VERSION)
            .header("Editor-Version", EDITOR_VERSION)
            .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to exchange GitHub token for Copilot token")?;

        let status = resp.status();
        if status.is_success() {
            let token_resp: CopilotTokenResponse = resp
                .json()
                .await
                .context("Failed to parse Copilot token response")?;

            return Ok(CopilotApiToken {
                token: token_resp.token,
                expires_at: token_resp.expires_at,
            });
        }

        let retryable = token_exchange_retryable_status(status.as_u16());
        if retryable && attempt < TOKEN_EXCHANGE_MAX_ATTEMPTS {
            let delay_ms = token_exchange_backoff_ms(attempt);
            crate::logging::warn(&format!(
                "Copilot token exchange got transient HTTP {}, retrying in {}ms (attempt {}/{})",
                status, delay_ms, attempt, TOKEN_EXCHANGE_MAX_ATTEMPTS
            ));
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            continue;
        }

        let body = crate::util::http_error_body(resp, "HTTP error").await;
        if retryable {
            anyhow::bail!(
                "Copilot token exchange failed after {} attempts (HTTP {}): {}",
                attempt,
                status,
                body
            );
        }
        anyhow::bail!("Copilot token exchange failed (HTTP {}): {}", status, body);
    }
}

/// Run a live Copilot auth check and persist the result as a validation record.
///
/// This is the only definitive way to know a discovered GitHub token is actually
/// usable for Copilot: a token can exist locally while the account is banned,
/// not entitled, or otherwise rejected by the Copilot token service. We exchange
/// the GitHub OAuth token for a Copilot bearer token (the same call the live
/// provider makes) and record success/failure so presence-based readiness
/// surfaces (`validation_failure_blocks_auto_use`, `check_fast`) reflect reality.
///
/// Returns `Ok(())` when the token exchange succeeds, or the underlying error
/// (whose message embeds the HTTP status, e.g. `HTTP 401`/`HTTP 403`) otherwise.
pub async fn verify_copilot_credentials_live(client: &reqwest::Client) -> Result<()> {
    let github_token = load_github_token()?;
    let result = exchange_github_token(client, &github_token).await;

    let summary = match &result {
        Ok(_) => "copilot token exchange ok".to_string(),
        Err(err) => format!("{err}"),
    };
    let record = crate::auth::validation::ProviderValidationRecord {
        checked_at_ms: chrono::Utc::now().timestamp_millis(),
        success: result.is_ok(),
        provider_smoke_ok: Some(result.is_ok()),
        tool_smoke_ok: None,
        summary,
    };
    // Best-effort: a failure to persist must not change the live result.
    let _ = crate::auth::validation::save("copilot", record);
    // Refresh the auth snapshot so readiness surfaces pick up the new record.
    crate::auth::AuthStatus::invalidate_cache();

    result.map(|_| ())
}

/// Convenience wrapper around [`verify_copilot_credentials_live`] that builds a
/// short-lived HTTP client. Useful for callers (e.g. the TUI crate) that do not
/// depend on `reqwest` directly.
pub async fn verify_copilot_credentials_live_default() -> Result<()> {
    let client = reqwest::Client::new();
    verify_copilot_credentials_live(&client).await
}

/// Initiate GitHub OAuth device flow for Copilot authentication.
/// Returns the device code response with user instructions.
pub async fn initiate_device_flow(client: &reqwest::Client) -> Result<DeviceCodeResponse> {
    let endpoints = CopilotEndpoints::load()?;
    let resp = client
        .post(&endpoints.device_code_url)
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
    let endpoints = CopilotEndpoints::load()?;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

        let resp = client
            .post(&endpoints.access_token_url)
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
    let endpoints = CopilotEndpoints::load()?;
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
    config.insert(endpoints.host, entry);

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
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    pub supported_endpoints: Vec<String>,
    #[serde(default)]
    pub capabilities: Option<CopilotModelCapabilities>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CopilotModelCapabilities {
    #[serde(default)]
    pub limits: Option<CopilotModelLimits>,
    #[serde(default)]
    pub supports: Option<CopilotModelSupports>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CopilotModelSupports {
    /// Advertised reasoning effort levels, e.g. `["low","medium","high"]`.
    #[serde(default)]
    pub reasoning_effort: Option<Value>,
    #[serde(default)]
    pub vision: Option<bool>,
    #[serde(default)]
    pub tool_calls: Option<bool>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub streaming: Option<bool>,
    #[serde(default)]
    pub structured_outputs: Option<bool>,
    #[serde(default)]
    pub adaptive_thinking: Option<bool>,
}

impl CopilotModelInfo {
    pub fn metadata(&self) -> jcode_provider_core::ModelMetadata {
        let limits = self
            .capabilities
            .as_ref()
            .and_then(|caps| caps.limits.as_ref());
        let supports = self
            .capabilities
            .as_ref()
            .and_then(|caps| caps.supports.as_ref());
        jcode_provider_core::ModelMetadata {
            context_window: limits
                .and_then(|limits| limits.max_context_window_tokens)
                .filter(|value| *value > 0),
            max_input_tokens: limits
                .and_then(|limits| limits.max_prompt_tokens)
                .filter(|value| *value > 0),
            max_output_tokens: limits
                .and_then(|limits| limits.max_output_tokens)
                .filter(|value| *value > 0),
            max_non_streaming_output_tokens: limits
                .and_then(|limits| limits.max_non_streaming_output_tokens)
                .filter(|value| *value > 0),
            reasoning_efforts: self.reasoning_efforts(),
            supports_image_input: supports.and_then(|supports| supports.vision),
            supports_tool_calls: supports.and_then(|supports| supports.tool_calls),
            supports_parallel_tool_calls: supports
                .and_then(|supports| supports.parallel_tool_calls),
            supports_streaming: supports.and_then(|supports| supports.streaming),
            supports_structured_outputs: supports.and_then(|supports| supports.structured_outputs),
            supports_adaptive_thinking: supports.and_then(|supports| supports.adaptive_thinking),
            supported_endpoints: self.supported_endpoints.clone(),
            ..Default::default()
        }
    }

    /// Reasoning effort levels advertised in the catalog, lowercased.
    /// Empty when the model does not advertise any.
    pub fn reasoning_efforts(&self) -> Vec<String> {
        let Some(Value::Array(levels)) = self
            .capabilities
            .as_ref()
            .and_then(|c| c.supports.as_ref())
            .and_then(|s| s.reasoning_effort.as_ref())
        else {
            return Vec::new();
        };
        levels
            .iter()
            .filter_map(|v| v.as_str())
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CopilotModelLimits {
    #[serde(default)]
    pub max_context_window_tokens: Option<usize>,
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
    #[serde(default)]
    pub max_prompt_tokens: Option<usize>,
    #[serde(default)]
    pub max_non_streaming_output_tokens: Option<usize>,
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
    fetch_available_models_with_endpoints(client, bearer_token, &CopilotEndpoints::load()?).await
}

pub async fn fetch_available_models_with_endpoints(
    client: &reqwest::Client,
    bearer_token: &str,
    endpoints: &CopilotEndpoints,
) -> Result<Vec<CopilotModelInfo>> {
    let resp = client
        .get(format!("{}/models", endpoints.api_base))
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

/// Determine the best default model from the live Copilot catalog.
///
/// Uses the same flagship-first ranking and new-release auto-promotion as every
/// other provider's post-login selection, so the newest Claude/GPT flagship the
/// account can access wins (e.g. `claude-opus-5.5` over `claude-opus-4.6`) without
/// a code change per release. Picker-enabled models are preferred when the
/// catalog marks any, since the rest are often embeddings or internal ids.
/// Returns the Copilot catalog id (dot-separated), which is what the Copilot API
/// accepts. Falls back to Sonnet, then the live picker's first model, when
/// nothing in the catalog is recognized.
pub fn choose_default_model(available_models: &[CopilotModelInfo]) -> String {
    let picker_enabled = available_models.iter().any(|m| m.model_picker_enabled);
    let candidates: Vec<&str> = available_models
        .iter()
        .filter(|m| !picker_enabled || m.model_picker_enabled)
        .map(|m| m.id.as_str())
        .collect();

    // Copilot ids are dot-separated (`claude-opus-4.6`); the shared ranking is
    // keyed on canonical hyphenated ids (`claude-opus-4-6`). Rank canonical
    // forms, then map the winner back to the original Copilot id.
    let canonical_of = |id: &str| -> String {
        jcode_provider_core::normalize_copilot_model_name(id)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if id.starts_with("claude-") {
                    id.replace('.', "-")
                } else {
                    id.to_string()
                }
            })
    };
    let routes: Vec<crate::provider::ModelRoute> = candidates
        .iter()
        .filter(|id| id.starts_with("claude-") || id.starts_with("gpt-"))
        .map(|id| crate::provider::ModelRoute {
            catalog_metadata: None,
            model: canonical_of(id),
            provider: "Copilot".to_string(),
            api_method: "copilot".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        })
        .collect();
    let activation = crate::auth::lifecycle::AuthActivationResult {
        provider_id: Some("copilot".to_string()),
        ..Default::default()
    };
    if let Some(best) =
        crate::auth::lifecycle::provider_model_to_select_after_auth(&activation, None, &routes)
        && let Some(original) = candidates.iter().find(|id| canonical_of(id) == best)
    {
        return (*original).to_string();
    }

    let model_ids: Vec<&str> = available_models.iter().map(|m| m.id.as_str()).collect();
    if model_ids.contains(&"claude-sonnet-4.6") {
        "claude-sonnet-4.6".to_string()
    } else if model_ids.contains(&"claude-sonnet-4") {
        "claude-sonnet-4".to_string()
    } else if let Some(model) = available_models
        .iter()
        .find(|model| model.model_picker_enabled)
    {
        model.id.clone()
    } else {
        "claude-sonnet-4".to_string()
    }
}

/// Fetch the authenticated GitHub username using an OAuth token.
pub async fn fetch_github_username(client: &reqwest::Client, token: &str) -> Result<String> {
    let endpoints = CopilotEndpoints::load()?;
    let resp = client
        .get(format!("{}/user", endpoints.github_api_base))
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
