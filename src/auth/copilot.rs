use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// VSCode's OAuth client ID for GitHub Copilot device flow.
/// This is the well-known client ID used by VS Code, OpenCode, and other tools.
pub const GITHUB_COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// GitHub endpoints for Copilot auth
pub const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// Copilot API base URL
pub const COPILOT_API_BASE: &str = "https://api.githubcopilot.com";

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
/// 1. GITHUB_TOKEN environment variable
/// 2. ~/.config/github-copilot/hosts.json (Copilot CLI)
/// 3. ~/.config/github-copilot/apps.json (VS Code)
pub fn load_github_token() -> Result<String> {
    // 1. Environment variable
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(token.trim().to_string());
        }
    }

    // Get config directory
    let config_dir = copilot_config_dir();

    // 2. hosts.json (Copilot CLI login)
    let hosts_path = config_dir.join("hosts.json");
    if let Ok(token) = load_token_from_json(&hosts_path) {
        return Ok(token);
    }

    // 3. apps.json (VS Code)
    let apps_path = config_dir.join("apps.json");
    if let Ok(token) = load_token_from_json(&apps_path) {
        return Ok(token);
    }

    anyhow::bail!(
        "GitHub Copilot token not found. \
         Set GITHUB_TOKEN, or run `gh auth login` / `gh extension install github/gh-copilot && gh copilot` \
         to authenticate."
    )
}

/// Check if Copilot credentials are available (without loading the full token)
pub fn has_copilot_credentials() -> bool {
    load_github_token().is_ok()
}

fn copilot_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("github-copilot")
    } else if cfg!(windows) {
        let local_app_data =
            std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{}/AppData/Local", home)
            });
        PathBuf::from(local_app_data).join("github-copilot")
    } else {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".config").join("github-copilot")
    }
}

/// Parse a Copilot config JSON file to extract the oauth_token.
/// Format: { "github.com": { "oauth_token": "gho_xxxx", "user": "..." } }
fn load_token_from_json(path: &PathBuf) -> Result<String> {
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let config: HashMap<String, HashMap<String, serde_json::Value>> =
        serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse {}", path.display()))?;

    for (key, value) in &config {
        if key.contains("github.com") {
            if let Some(serde_json::Value::String(token)) = value.get("oauth_token") {
                if !token.is_empty() {
                    return Ok(token.clone());
                }
            }
        }
    }

    anyhow::bail!("No oauth_token found in {}", path.display())
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
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Copilot token exchange failed (HTTP {}): {}",
            status,
            body
        );
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
        let body = resp.text().await.unwrap_or_default();
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
    let config_dir = copilot_config_dir();
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("Failed to create {}", config_dir.display()))?;

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
    std::fs::write(&hosts_path, json)
        .with_context(|| format!("Failed to write {}", hosts_path.display()))?;

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
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Copilot models fetch failed (HTTP {}): {}", status, body);
    }

    let models_resp: ModelsResponse = resp
        .json()
        .await
        .context("Failed to parse Copilot models response")?;

    Ok(models_resp.data)
}

/// Determine the best default model based on available models.
/// - If claude-opus-4-6 is available -> paid tier -> use claude-opus-4-6
/// - Otherwise -> free/basic tier -> use claude-sonnet-4-6
pub fn choose_default_model(available_models: &[CopilotModelInfo]) -> String {
    let model_ids: Vec<&str> = available_models
        .iter()
        .map(|m| m.id.as_str())
        .collect();

    if model_ids.contains(&"claude-opus-4-6") {
        "claude-opus-4-6".to_string()
    } else if model_ids.contains(&"claude-sonnet-4-6") {
        "claude-sonnet-4-6".to_string()
    } else if model_ids.contains(&"claude-sonnet-4") {
        "claude-sonnet-4".to_string()
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
