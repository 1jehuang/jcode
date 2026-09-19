//! Jcode-managed Grok Build backend discovery, OIDC credentials, and CLI path.
//!
//! Subscription auth lives in the Grok CLI store (`GROK_HOME` or `~/.grok/auth.json`)
//! under the xAI issuer/client scope. HTTP chat uses that scoped access token
//! (with refresh); ACP still talks to the official `grok agent stdio` backend.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const CLI_PATH_ENV: &str = "JCODE_GROK_CLI_PATH";
const PRIMARY_BASE_URL: &str = "https://x.ai/cli";
const FALLBACK_BASE_URL: &str = "https://storage.googleapis.com/grok-build-public-artifacts/cli";
const OAUTH_ISSUER: &str = "https://auth.x.ai";
const OAUTH_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const OAUTH_SCOPES: &str = "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write workspaces:read workspaces:write";

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default = "default_poll_interval")]
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct JwtClaims {
    sub: Option<String>,
    email: Option<String>,
    given_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct StoredCredential {
    key: String,
    auth_mode: &'static str,
    create_time: String,
    user_id: String,
    email: Option<String>,
    coding_data_retention_opt_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
    oidc_issuer: &'static str,
    oidc_client_id: &'static str,
}

fn default_poll_interval() -> u64 {
    5
}

fn oauth_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("x-grok-client-version", "1.0.3")
        .header("x-grok-client-surface", "ui")
        .header(
            "user-agent",
            format!("jcode/{} grok-shell/1.0.3", env!("CARGO_PKG_VERSION")),
        )
}

pub async fn initiate_device_login(client: &reqwest::Client) -> Result<DeviceAuthorization> {
    oauth_headers(client.post(format!("{OAUTH_ISSUER}/oauth2/device/code")))
        .form(&[
            ("client_id", OAUTH_CLIENT_ID),
            ("scope", OAUTH_SCOPES),
            ("referrer", "grok-build"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("invalid xAI device authorization response")
}

pub async fn complete_device_login(
    client: &reqwest::Client,
    authorization: &DeviceAuthorization,
) -> Result<()> {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(authorization.expires_in.max(600));
    let mut interval = authorization.interval.max(1);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() >= deadline {
            bail!("xAI device authorization expired");
        }
        let response = oauth_headers(client.post(format!("{OAUTH_ISSUER}/oauth2/token")))
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", authorization.device_code.as_str()),
                ("client_id", OAUTH_CLIENT_ID),
            ])
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        if status.is_success() {
            let tokens: TokenResponse =
                serde_json::from_slice(&body).context("invalid xAI token response")?;
            return save_tokens(tokens);
        }
        let error: TokenError = serde_json::from_slice(&body)
            .with_context(|| format!("xAI token request failed with {status}"))?;
        match error.error.as_str() {
            "authorization_pending" => continue,
            "slow_down" => {
                interval += 5;
                continue;
            }
            _ => bail!(
                "xAI login failed: {}",
                error.error_description.unwrap_or(error.error)
            ),
        }
    }
}

fn save_tokens(tokens: TokenResponse) -> Result<()> {
    let claims = tokens
        .access_token
        .split('.')
        .nth(1)
        .and_then(|part| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part)
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<JwtClaims>(&bytes).ok())
        .unwrap_or_default();
    let now = chrono::Utc::now();
    let credential = StoredCredential {
        key: tokens.access_token,
        auth_mode: "oidc",
        create_time: now.to_rfc3339(),
        user_id: claims.sub.unwrap_or_default(),
        email: claims.email,
        coding_data_retention_opt_out: false,
        first_name: claims.given_name,
        refresh_token: tokens.refresh_token,
        expires_at: tokens
            .expires_in
            .map(|seconds| (now + chrono::Duration::seconds(seconds as i64)).to_rfc3339()),
        oidc_issuer: OAUTH_ISSUER,
        oidc_client_id: OAUTH_CLIENT_ID,
    };
    let home = grok_home(
        std::env::var_os("GROK_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
    )
    .context("No home directory available for Grok Build credentials")?;
    std::fs::create_dir_all(&home)?;
    let path = home.join("auth.json");
    let mut credentials = std::fs::read(&path)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes).ok()
        })
        .unwrap_or_default();
    credentials.insert(
        format!("{OAUTH_ISSUER}::{OAUTH_CLIENT_ID}"),
        serde_json::to_value(credential)?,
    );
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&credentials)?)?;
    crate::platform::set_permissions_owner_only(&temporary)?;
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

fn grok_home(
    grok_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    grok_home.map(PathBuf::from).or_else(|| {
        home.or(user_profile)
            .map(|home| PathBuf::from(home).join(".grok"))
    })
}

fn managed_cli_path() -> Result<PathBuf> {
    let name = if cfg!(windows) { "grok.exe" } else { "grok" };
    Ok(crate::storage::jcode_dir()?
        .join("provider-backends")
        .join("grok-build")
        .join(name))
}

pub fn cli_path() -> PathBuf {
    if let Some(path) = std::env::var_os(CLI_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return path;
    }
    if let Ok(path) = managed_cli_path()
        && path.is_file()
    {
        return path;
    }
    PathBuf::from("grok")
}

pub fn cli_available() -> bool {
    super::command_exists(cli_path().to_string_lossy().as_ref())
}

/// Version string the CLI chat proxy expects in `User-Agent: grok-cli/<ver>`.
/// Missing this header makes the proxy report version `(none)` and return 426.
pub fn cli_version_string() -> String {
    static CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            if let Ok(version) = std::env::var("JCODE_GROK_CLI_VERSION") {
                let version = version.trim();
                if !version.is_empty() {
                    return version.to_string();
                }
            }
            std::process::Command::new(cli_path())
                .arg("--version")
                .output()
                .ok()
                .and_then(|output| {
                    let text = String::from_utf8_lossy(&output.stdout);
                    text.split_whitespace()
                        .nth(1)
                        .map(|part| part.trim().to_string())
                        .filter(|part| !part.is_empty())
                })
                .unwrap_or_else(|| "1.0.34".to_string())
        })
        .clone()
}

fn credential_scope_key() -> String {
    format!("{OAUTH_ISSUER}::{OAUTH_CLIENT_ID}")
}

fn auth_json_path() -> Option<PathBuf> {
    grok_home(
        std::env::var_os("GROK_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
    )
    .map(|home| home.join("auth.json"))
}

fn is_supported_oidc_credential(scope_key: &str, credential: &serde_json::Value) -> bool {
    if scope_key != credential_scope_key() {
        return false;
    }
    match credential.get("auth_mode").and_then(serde_json::Value::as_str) {
        None | Some("") | Some("oidc") => {}
        Some(_) => return false,
    }
    let issuer_ok = credential
        .get("oidc_issuer")
        .and_then(serde_json::Value::as_str)
        .map(|issuer| issuer == OAUTH_ISSUER)
        .unwrap_or(true);
    let client_ok = credential
        .get("oidc_client_id")
        .and_then(serde_json::Value::as_str)
        .map(|client| client == OAUTH_CLIENT_ID)
        .unwrap_or(true);
    issuer_ok && client_ok
}

fn credential_access_key(credential: &serde_json::Value) -> Option<String> {
    credential
        .get("key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(ToOwned::to_owned)
}

fn scoped_credential_from_bytes(bytes: &[u8]) -> Option<serde_json::Value> {
    let serde_json::Value::Object(scopes) = serde_json::from_slice(bytes).ok()? else {
        return None;
    };
    let key = credential_scope_key();
    let credential = scopes.get(&key)?.clone();
    is_supported_oidc_credential(&key, &credential).then_some(credential)
}

fn load_scoped_credential() -> Option<serde_json::Value> {
    scoped_credential_from_bytes(&std::fs::read(auth_json_path()?).ok()?)
}

fn credential_is_expired(credential: &serde_json::Value) -> bool {
    let Some(expires_at) = credential
        .get("expires_at")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return true;
    };
    expiry.with_timezone(&chrono::Utc) <= chrono::Utc::now() + chrono::Duration::seconds(60)
}

/// Whether the managed backend has a credential that it can attempt to use.
/// Backend presence alone is not authentication and must not make `/login` or
/// `jcode auth status` claim that Grok Build is ready.
/// Bearer token from Grok CLI / Jcode Grok Build OIDC login.
/// This is the subscription session token, not `XAI_API_KEY`.
pub fn bearer_token() -> Option<String> {
    if let Ok(key) = std::env::var("GROK_DEPLOYMENT_KEY") {
        let key = key.trim();
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }
    credential_access_key(&load_scoped_credential()?)
}

/// Access token after honouring expiry / refresh. Falls back to ACP if refresh fails.
pub async fn live_bearer_token() -> Option<String> {
    live_bearer_token_inner(false).await
}

async fn live_bearer_token_inner(force_refresh: bool) -> Option<String> {
    if let Ok(key) = std::env::var("GROK_DEPLOYMENT_KEY") {
        let key = key.trim();
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }
    let credential = load_scoped_credential()?;
    if force_refresh || credential_is_expired(&credential) {
        refresh_scoped_oidc(&credential).await.ok()?;
        return credential_access_key(&load_scoped_credential()?);
    }
    credential_access_key(&credential)
}

/// Force a refresh after a 401 from the CLI chat proxy.
pub async fn refresh_bearer_token() -> Option<String> {
    live_bearer_token_inner(true).await
}

async fn refresh_scoped_oidc(credential: &serde_json::Value) -> Result<()> {
    let refresh = credential
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Grok Build OIDC credential has no refresh token"))?;
    let client = reqwest::Client::new();
    let response = oauth_headers(client.post(format!("{OAUTH_ISSUER}/oauth2/token")))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", OAUTH_CLIENT_ID),
        ])
        .send()
        .await?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        let error: TokenError = serde_json::from_slice(&body).unwrap_or(TokenError {
            error: status.to_string(),
            error_description: None,
        });
        bail!(
            "Grok Build token refresh failed: {}",
            error.error_description.unwrap_or(error.error)
        );
    }
    let mut tokens: TokenResponse =
        serde_json::from_slice(&body).context("invalid xAI refresh token response")?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh.to_string());
    }
    save_tokens(tokens)
}

pub fn has_cached_login() -> bool {
    if std::env::var("GROK_DEPLOYMENT_KEY")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    let Some(path) = auth_json_path() else {
        return false;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    credentials_json_has_login(&bytes)
}

fn credentials_json_has_login(bytes: &[u8]) -> bool {
    scoped_credential_from_bytes(bytes)
        .and_then(|credential| credential_access_key(&credential))
        .is_some()
}

fn platform_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("macos", "aarch64") => Ok("macos-aarch64"),
        ("windows", "x86_64") => Ok("windows-x86_64"),
        ("windows", "aarch64") => Ok("windows-aarch64"),
        (os, arch) => bail!("Grok Build is not available for {os}-{arch}"),
    }
}

fn valid_version(version: &str) -> bool {
    let mut core_and_suffix = version.splitn(2, '-');
    let core = core_and_suffix.next().unwrap_or_default();
    let suffix_ok = core_and_suffix.next().is_none_or(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_'))
    });
    suffix_ok
        && core.split('.').count() == 3
        && core
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

async fn download_from_base(client: &reqwest::Client, base: &str) -> Result<Vec<u8>> {
    let version = client
        .get(format!("{base}/stable"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let version = version.trim();
    if !valid_version(version) {
        bail!("xAI returned an invalid Grok Build version: {version:?}");
    }
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let url = format!("{base}/grok-{version}-{}{extension}", platform_name()?);
    Ok(client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("failed to download {url}"))?
        .bytes()
        .await?
        .to_vec())
}

/// Return a usable Grok Build ACP backend, downloading the official binary
/// into Jcode's private data directory when no explicit/system binary exists.
pub async fn ensure_cli() -> Result<PathBuf> {
    let existing = cli_path();
    if super::command_exists(existing.to_string_lossy().as_ref()) {
        return Ok(existing);
    }

    let destination = managed_cli_path()?;
    let parent = destination
        .parent()
        .context("managed Grok Build path has no parent")?;
    std::fs::create_dir_all(parent)?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("jcode/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let bytes = match download_from_base(&client, PRIMARY_BASE_URL).await {
        Ok(bytes) => bytes,
        Err(primary) => download_from_base(&client, FALLBACK_BASE_URL)
            .await
            .with_context(|| format!("x.ai download failed first: {primary:#}"))?,
    };
    if bytes.is_empty() {
        bail!("downloaded Grok Build backend was empty");
    }

    let temporary = destination.with_extension(format!("download-{}", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::rename(&temporary, &destination)?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::{credentials_json_has_login, ensure_cli, grok_home, valid_version};
    use std::path::PathBuf;

    #[test]
    fn accepts_only_safe_release_versions() {
        assert!(valid_version("1.2.3"));
        assert!(valid_version("1.2.3-alpha.1"));
        assert!(!valid_version("latest"));
        assert!(!valid_version("1.2.3/../../bad"));
        assert!(!valid_version("1.2"));
    }

    #[test]
    fn cli_version_string_is_a_single_token() {
        let version = super::cli_version_string();
        assert!(!version.is_empty(), "CLI chat proxy treats empty as (none)");
        assert!(
            !version.contains(char::is_whitespace),
            "User-Agent grok-cli/<ver> cannot contain whitespace: {version:?}"
        );
        assert_ne!(version, "none");
    }

    #[test]
    fn backend_presence_is_not_mistaken_for_login() {
        assert!(!credentials_json_has_login(br#"{}"#));
        assert!(!credentials_json_has_login(
            br#"{"https://auth.x.ai::client":{"key":"token"}}"#
        ));
        let scoped = format!(
            r#"{{"{}":{{"key":"token","auth_mode":"oidc"}}}}"#,
            super::credential_scope_key()
        );
        assert!(credentials_json_has_login(scoped.as_bytes()));
        let empty_scoped = format!(
            r#"{{"{}":{{"key":"","auth_mode":"oidc"}}}}"#,
            super::credential_scope_key()
        );
        assert!(!credentials_json_has_login(empty_scoped.as_bytes()));
    }

    #[test]
    fn bearer_token_ignores_unrelated_auth_store_entries() {
        let _lock = crate::storage::lock_test_env();
        let dir = tempfile::tempdir().unwrap();
        let scope = super::credential_scope_key();
        std::fs::write(
            dir.path().join("auth.json"),
            format!(
                r#"{{
                    "https://other.example::abc": {{"key":"wrong","auth_mode":"oidc"}},
                    "{scope}": {{"key":"right","auth_mode":"oidc","oidc_issuer":"https://auth.x.ai","oidc_client_id":"b1a00492-073a-47ea-816f-4c329264a828"}}
                }}"#
            ),
        )
        .unwrap();
        crate::env::set_var("GROK_HOME", dir.path());
        crate::env::remove_var("GROK_DEPLOYMENT_KEY");
        assert_eq!(super::bearer_token().as_deref(), Some("right"));
    }

    #[test]
    fn grok_home_prefers_grok_home_env() {
        assert_eq!(
            grok_home(Some("/tmp/grok-store".into()), Some("/home/me".into()), None),
            Some(PathBuf::from("/tmp/grok-store"))
        );
    }

    #[test]
    fn credential_home_falls_back_to_user_profile() {
        assert_eq!(
            grok_home(None, None, Some("C:\\Users\\jcode".into())),
            Some(PathBuf::from("C:\\Users\\jcode").join(".grok"))
        );
    }

    #[tokio::test]
    #[ignore = "downloads the official ~160 MB Grok Build provider backend"]
    async fn provisions_working_backend_without_system_cli() {
        let path = ensure_cli().await.expect("managed backend should download");
        assert!(path.is_file());
        let status = std::process::Command::new(path)
            .arg("--version")
            .status()
            .expect("managed backend should launch");
        assert!(status.success());
    }
}
