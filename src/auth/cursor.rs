use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CURSOR_API_BASE: &str = "https://api2.cursor.sh";
const CURSOR_DIRECT_CLIENT_VERSION_DEFAULT: &str = "2.4.0";
const CURSOR_OAUTH_CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";
pub const CURSOR_AUTH_FILE_SOURCE_ID: &str = "cursor_auth_json";
pub const CURSOR_VSCDB_SOURCE_ID: &str = "cursor_vscdb";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalCursorAuthSource {
    CursorAuthFile,
    CursorVscdb,
}

impl ExternalCursorAuthSource {
    pub fn source_id(self) -> &'static str {
        match self {
            Self::CursorAuthFile => CURSOR_AUTH_FILE_SOURCE_ID,
            Self::CursorVscdb => CURSOR_VSCDB_SOURCE_ID,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::CursorAuthFile => "Cursor auth.json",
            Self::CursorVscdb => "Cursor IDE state.vscdb",
        }
    }

    pub fn path(self) -> Result<PathBuf> {
        match self {
            Self::CursorAuthFile => cursor_auth_file_path(),
            Self::CursorVscdb => find_cursor_vscdb(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorDirectTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub source: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorAuthFileData {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CursorRefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorApiKeyExchangeResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwtClaims {
    exp: Option<u64>,
}

#[derive(Debug, Serialize)]
struct CursorRefreshRequest<'a> {
    grant_type: &'static str,
    client_id: &'static str,
    refresh_token: &'a str,
}

/// Check if Cursor API key is available (env var or saved file).
pub fn has_cursor_api_key() -> bool {
    load_api_key().is_ok()
}

/// Whether direct Cursor native auth is available without relying on cursor-agent runtime.
pub fn has_cursor_native_auth() -> bool {
    load_access_token_from_env_or_file().is_ok() || has_cursor_vscdb_token() || has_cursor_api_key()
}

pub fn preferred_external_auth_source() -> Option<ExternalCursorAuthSource> {
    if cursor_auth_file_path()
        .map(|path| path.exists())
        .unwrap_or(false)
    {
        return Some(ExternalCursorAuthSource::CursorAuthFile);
    }

    if cursor_vscdb_paths().into_iter().any(|path| path.exists()) {
        return Some(ExternalCursorAuthSource::CursorVscdb);
    }

    None
}

pub fn has_unconsented_external_auth() -> Option<ExternalCursorAuthSource> {
    let source = preferred_external_auth_source()?;
    let allowed = source
        .path()
        .ok()
        .map(|path| {
            crate::config::Config::external_auth_source_allowed_for_path(source.source_id(), &path)
        })
        .unwrap_or(false);
    if allowed { None } else { Some(source) }
}

pub fn trust_external_auth_source(source: ExternalCursorAuthSource) -> Result<()> {
    crate::config::Config::allow_external_auth_source_for_path(
        source.source_id(),
        &source.path()?,
    )?;
    super::AuthStatus::invalidate_cache();
    Ok(())
}

/// Resolve the advertised client version for native Cursor API requests.
pub fn cursor_direct_client_version() -> String {
    std::env::var("JCODE_CURSOR_CLIENT_VERSION")
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
        .unwrap_or_else(|| CURSOR_DIRECT_CLIENT_VERSION_DEFAULT.to_string())
}

/// Resolve the Cursor Agent CLI path from the environment or default.
pub fn cursor_agent_cli_path() -> String {
    std::env::var("JCODE_CURSOR_CLI_PATH").unwrap_or_else(|_| "cursor-agent".to_string())
}

/// Check if `cursor-agent` CLI is available on PATH.
pub fn has_cursor_agent_cli() -> bool {
    super::command_available_from_env("JCODE_CURSOR_CLI_PATH", "cursor-agent")
}

/// Check whether Cursor Agent reports an authenticated local session.
pub fn has_cursor_agent_auth() -> bool {
    if !has_cursor_agent_cli() {
        return false;
    }

    let output = match std::process::Command::new(cursor_agent_cli_path())
        .arg("status")
        .output()
    {
        Ok(output) => output,
        Err(_) => return false,
    };

    status_output_indicates_authenticated(output.status.success(), &output.stdout, &output.stderr)
}

/// Check if Cursor IDE's local vscdb has an access token.
pub fn has_cursor_vscdb_token() -> bool {
    find_cursor_vscdb()
        .ok()
        .map(|path| {
            crate::config::Config::external_auth_source_allowed_for_path(
                CURSOR_VSCDB_SOURCE_ID,
                &path,
            )
        })
        .unwrap_or(false)
        && read_vscdb_token().is_ok()
}

/// Read access token from Cursor IDE's SQLite storage (state.vscdb).
/// Uses the `sqlite3` CLI to avoid adding a native dependency.
pub fn read_vscdb_token() -> Result<String> {
    let db_path = find_cursor_vscdb()?;
    read_vscdb_key(&db_path, "cursorAuth/accessToken")
}

/// Read refresh token from Cursor IDE's SQLite storage (state.vscdb).
pub fn read_vscdb_refresh_token() -> Result<String> {
    let db_path = find_cursor_vscdb()?;
    read_vscdb_key(&db_path, "cursorAuth/refreshToken")
}

/// Read the machine ID from Cursor's vscdb (needed for API checksum header).
pub fn read_vscdb_machine_id() -> Result<String> {
    let db_path = find_cursor_vscdb()?;
    read_vscdb_key(&db_path, "storage.serviceMachineId")
}

/// Find the Cursor vscdb file on this platform.
fn find_cursor_vscdb() -> Result<PathBuf> {
    let candidates = cursor_vscdb_paths();
    for path in &candidates {
        if path.exists() {
            return crate::storage::validate_external_auth_file(path);
        }
    }
    anyhow::bail!("Cursor state.vscdb not found (is Cursor IDE installed?)")
}

/// Platform-specific candidate paths for Cursor's state.vscdb.
fn cursor_vscdb_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    let relatives = [
        ".config/Cursor/User/globalStorage/state.vscdb",
        ".config/cursor/User/globalStorage/state.vscdb",
    ];
    #[cfg(target_os = "macos")]
    let relatives = [
        "Library/Application Support/Cursor/User/globalStorage/state.vscdb",
        "Library/Application Support/cursor/User/globalStorage/state.vscdb",
    ];
    #[cfg(target_os = "windows")]
    let relatives = [
        "AppData/Roaming/Cursor/User/globalStorage/state.vscdb",
        "AppData/Roaming/cursor/User/globalStorage/state.vscdb",
    ];

    relatives
        .into_iter()
        .filter_map(|relative| crate::storage::user_home_path(relative).ok())
        .collect()
}

/// Read a key from a vscdb file using the sqlite3 CLI.
fn read_vscdb_key(db_path: &PathBuf, key: &str) -> Result<String> {
    let mut cmd = std::process::Command::new("sqlite3");
    cmd.arg(db_path).arg(format!(
        "SELECT value FROM ItemTable WHERE key = '{}';",
        key
    ));
    crate::platform::suppress_child_console(&mut cmd);
    let output = cmd
        .output()
        .context("Failed to run sqlite3 (is it installed?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("sqlite3 failed: {}", stderr.trim());
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        anyhow::bail!("Key '{}' not found or empty in {}", key, db_path.display());
    }
    Ok(value)
}

/// Load Cursor API key. Checks in order:
/// 1. `CURSOR_API_KEY` env var
/// 2. Saved key in `~/.config/jcode/cursor.env`
pub fn load_api_key() -> Result<String> {
    if let Ok(key) = std::env::var("CURSOR_API_KEY") {
        let trimmed = key.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let file_path = config_file_path()?;
    if file_path.exists() {
        crate::storage::harden_secret_file_permissions(&file_path);
        let content = std::fs::read_to_string(&file_path)
            .with_context(|| format!("Failed to read {}", file_path.display()))?;
        for line in content.lines() {
            let line = line.trim();
            if let Some(key) = line.strip_prefix("CURSOR_API_KEY=") {
                let key = key.trim().trim_matches('"').trim_matches('\'');
                if !key.is_empty() {
                    return Ok(key.to_string());
                }
            }
        }
    }

    anyhow::bail!(
        "Cursor API key not found. Set CURSOR_API_KEY env var, \
         or run `/login cursor` to configure."
    )
}

/// Save a Cursor API key to `~/.config/jcode/cursor.env`.
pub fn save_api_key(key: &str) -> Result<()> {
    let file_path = config_file_path()?;
    crate::storage::upsert_env_file_value(&file_path, "CURSOR_API_KEY", Some(key))?;

    crate::env::set_var("CURSOR_API_KEY", key);
    Ok(())
}

fn config_file_path() -> Result<PathBuf> {
    let config_dir = crate::storage::app_config_dir()?;
    Ok(config_dir.join("cursor.env"))
}

/// Resolve Cursor CLI/device-login auth file path.
pub fn cursor_auth_file_path() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| crate::storage::user_home_path("AppData/Roaming").ok())
            .ok_or_else(|| anyhow::anyhow!("No APPDATA directory found"))?;
        Ok(appdata.join("Cursor").join("auth.json"))
    }

    #[cfg(target_os = "macos")]
    {
        crate::storage::user_home_path(".cursor/auth.json")
            .context("No home directory found for Cursor auth.json")
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow::anyhow!("No config directory found"))?;
        Ok(config_dir.join("cursor").join("auth.json"))
    }
}

/// Load direct Cursor tokens from env or Cursor's auth.json.
pub fn load_access_token_from_env_or_file() -> Result<CursorDirectTokens> {
    if let Ok(access_token) = std::env::var("CURSOR_ACCESS_TOKEN") {
        let access_token = access_token.trim().to_string();
        if !access_token.is_empty() {
            let refresh_token = std::env::var("CURSOR_REFRESH_TOKEN")
                .ok()
                .map(|raw| raw.trim().to_string())
                .filter(|raw| !raw.is_empty());
            return Ok(CursorDirectTokens {
                access_token,
                refresh_token,
                source: "env",
            });
        }
    }

    let file_path = cursor_auth_file_path()?;
    if file_path.exists()
        && crate::config::Config::external_auth_source_allowed_for_path(
            CURSOR_AUTH_FILE_SOURCE_ID,
            &file_path,
        )
    {
        let safe_path = crate::storage::validate_external_auth_file(&file_path)?;
        let raw = std::fs::read_to_string(&safe_path)
            .with_context(|| format!("Failed to read {}", file_path.display()))?;
        let parsed: CursorAuthFileData = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse {}", file_path.display()))?;
        if let Some(access_token) = parsed
            .access_token
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
        {
            return Ok(CursorDirectTokens {
                access_token,
                refresh_token: parsed
                    .refresh_token
                    .map(|token| token.trim().to_string())
                    .filter(|token| !token.is_empty()),
                source: "cursor_auth_file",
            });
        }
    }

    anyhow::bail!(
        "Cursor direct access token not found. Set CURSOR_ACCESS_TOKEN, log in with Cursor, or configure CURSOR_API_KEY."
    )
}

/// Resolve the best available direct-auth credentials for Cursor's native API.
pub async fn resolve_direct_tokens(client: &Client) -> Result<CursorDirectTokens> {
    if let Ok(tokens) = load_access_token_from_env_or_file() {
        if !token_is_expiring_soon(&tokens.access_token) {
            return Ok(tokens);
        }
        if let Some(refresh_token) = tokens.refresh_token.as_deref()
            && let Ok(refreshed) = refresh_direct_access_token(client, refresh_token).await
        {
            return Ok(CursorDirectTokens {
                source: tokens.source,
                ..refreshed
            });
        }
    }

    if find_cursor_vscdb()
        .ok()
        .map(|path| {
            crate::config::Config::external_auth_source_allowed_for_path(
                CURSOR_VSCDB_SOURCE_ID,
                &path,
            )
        })
        .unwrap_or(false)
        && let Ok(access_token) = read_vscdb_token()
    {
        let refresh_token = read_vscdb_refresh_token().ok();
        if !token_is_expiring_soon(&access_token) {
            return Ok(CursorDirectTokens {
                access_token,
                refresh_token,
                source: "cursor_vscdb",
            });
        }
        if let Some(refresh_token) = refresh_token.as_deref()
            && let Ok(refreshed) = refresh_direct_access_token(client, refresh_token).await
        {
            return Ok(CursorDirectTokens {
                source: "cursor_vscdb",
                ..refreshed
            });
        }
    }

    let api_key = load_api_key()?;
    let exchanged = exchange_api_key_for_tokens(client, &api_key).await?;
    Ok(CursorDirectTokens {
        source: "cursor_api_key",
        ..exchanged
    })
}

/// Build the `x-client-key` header expected by Cursor's native API.
pub fn client_key_for_access_token(access_token: &str) -> String {
    sha256_hex(access_token)
}

/// Build the `x-session-id` header expected by Cursor's native API.
pub fn session_id_for_access_token(access_token: &str) -> String {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, access_token.as_bytes()).to_string()
}

/// Build the `x-cursor-checksum` header expected by Cursor's native API.
pub fn checksum_for_access_token(access_token: &str) -> String {
    let machine_id =
        read_vscdb_machine_id().unwrap_or_else(|_| sha256_hex(&format!("{access_token}machineId")));
    format!("{}{}", timestamp_header_now(), machine_id)
}

async fn refresh_direct_access_token(
    client: &Client,
    refresh_token: &str,
) -> Result<CursorDirectTokens> {
    let result: Result<CursorDirectTokens> = async {
        let request = CursorRefreshRequest {
            grant_type: "refresh_token",
            client_id: CURSOR_OAUTH_CLIENT_ID,
            refresh_token,
        };

        let response = client
            .post(format!("{CURSOR_API_BASE}/oauth/token"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to refresh Cursor access token")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = crate::util::http_error_body(response, "HTTP error").await;
            anyhow::bail!(
                "Cursor access token refresh failed ({}): {}",
                status,
                body.trim()
            );
        }

        let parsed: CursorRefreshResponse = response
            .json()
            .await
            .context("Failed to decode Cursor token refresh response")?;
        Ok(CursorDirectTokens {
            access_token: parsed.access_token,
            refresh_token: parsed
                .refresh_token
                .or_else(|| Some(refresh_token.to_string())),
            source: "cursor_refresh",
        })
    }
    .await;

    match &result {
        Ok(_) => {
            let _ = crate::auth::refresh_state::record_success("cursor");
        }
        Err(err) => {
            let _ = crate::auth::refresh_state::record_failure("cursor", err.to_string());
        }
    }

    result
}

async fn exchange_api_key_for_tokens(client: &Client, api_key: &str) -> Result<CursorDirectTokens> {
    let response = client
        .post(format!("{CURSOR_API_BASE}/auth/exchange_user_api_key"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .bearer_auth(api_key)
        .body("{}")
        .send()
        .await
        .context("Failed to exchange Cursor API key for access token")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = crate::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!(
            "Cursor API key exchange failed ({}): {}",
            status,
            body.trim()
        );
    }

    let parsed: CursorApiKeyExchangeResponse = response
        .json()
        .await
        .context("Failed to decode Cursor API key exchange response")?;
    Ok(CursorDirectTokens {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        source: "cursor_api_key",
    })
}

fn token_is_expiring_soon(token: &str) -> bool {
    let Some(exp) = token_expiry_epoch_secs(token) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    exp <= now.saturating_add(60)
}

fn token_expiry_epoch_secs(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice::<JwtClaims>(&decoded).ok()?.exp
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn timestamp_header_now() -> String {
    let epoch_kiloseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() / 1_000_000)
        .unwrap_or(0);
    let mut bytes = [
        ((epoch_kiloseconds >> 40) & 0xFF) as u8,
        ((epoch_kiloseconds >> 32) & 0xFF) as u8,
        ((epoch_kiloseconds >> 24) & 0xFF) as u8,
        ((epoch_kiloseconds >> 16) & 0xFF) as u8,
        ((epoch_kiloseconds >> 8) & 0xFF) as u8,
        (epoch_kiloseconds & 0xFF) as u8,
    ];
    let mut prev = 165u8;
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (*byte ^ prev).wrapping_add(index as u8);
        prev = *byte;
    }
    URL_SAFE_NO_PAD.encode(bytes)
}

fn status_output_indicates_authenticated(success: bool, stdout: &[u8], stderr: &[u8]) -> bool {
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .to_ascii_lowercase();

    if combined.contains("not authenticated")
        || combined.contains("login required")
        || combined.contains("not logged in")
        || combined.contains("unauthenticated")
    {
        return false;
    }

    if !success {
        return false;
    }

    if combined.contains("authenticated")
        || combined.contains("account")
        || combined.contains("email")
        || combined.contains("endpoint")
    {
        return true;
    }

    success
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod tests;
