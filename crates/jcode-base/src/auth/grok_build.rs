//! Grok Build (Grok CLI subscription) credentials and client identity.
//!
//! Jcode talks to the Grok CLI chat proxy (`https://cli-chat-proxy.grok.com/v1`)
//! directly over HTTPS. It never launches the `grok` binary. Authentication is
//! the xAI OIDC session that the official Grok CLI stores in
//! `$GROK_HOME/auth.json` (default `~/.grok/auth.json`), keyed by
//! `<issuer>::<client_id>`. Jcode can create that entry itself with the native
//! OAuth device flow against `auth.x.ai` using the Grok CLI's public client id,
//! so the two tools share one login.
//!
//! The proxy only accepts requests that identify as the official CLI (it
//! answers `426 Upgrade Required` with version `(none)` otherwise), so every
//! request carries the Grok CLI identity from [`cli_version`] and the header
//! constants below. Override the advertised version with
//! `JCODE_GROK_CLI_VERSION` when xAI raises the minimum supported CLI version.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// OpenAI-compatible base URL used by the official Grok CLI for chat.
pub const CLI_CHAT_PROXY_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
/// Env override for [`CLI_CHAT_PROXY_BASE_URL`] (same name the Grok CLI honours).
pub const CLI_CHAT_PROXY_BASE_URL_ENV: &str = "GROK_CLI_CHAT_PROXY_BASE_URL";
/// Env override for the advertised Grok CLI version.
pub const CLI_VERSION_ENV: &str = "JCODE_GROK_CLI_VERSION";
/// Grok CLI release whose identity Jcode presents (`x.ai/cli/stable` on 2026-09-23).
pub const DEFAULT_CLI_VERSION: &str = "1.0.41";

/// Header names/values observed in the official Grok CLI 1.0.41 binary
/// (`xai-grok-shell/src/remote/model_source/oai.rs`, `xai-grok-sampler/src/client.rs`).
pub const HEADER_TOKEN_AUTH: &str = "X-XAI-Token-Auth";
pub const TOKEN_AUTH_VALUE: &str = "xai-grok-cli";
pub const HEADER_CLIENT_VERSION: &str = "x-grok-client-version";
pub const HEADER_CLIENT_IDENTIFIER: &str = "x-grok-client-identifier";
pub const CLIENT_IDENTIFIER_VALUE: &str = "grok-shell";
pub const HEADER_CLIENT_SURFACE: &str = "x-grok-client-surface";
pub const HEADER_MODEL_OVERRIDE: &str = "x-grok-model-override";
pub const HEADER_CONV_ID: &str = "x-grok-conv-id";
pub const HEADER_REQ_ID: &str = "x-grok-req-id";

pub const OAUTH_ISSUER: &str = "https://auth.x.ai";
pub const OAUTH_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const OAUTH_SCOPES: &str = "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write workspaces:read workspaces:write";
/// Refresh this long before `expires_at` so a turn never starts on a dying token.
const EXPIRY_SKEW_SECS: i64 = 60;

/// Advertised Grok CLI version (`JCODE_GROK_CLI_VERSION` or [`DEFAULT_CLI_VERSION`]).
pub fn cli_version() -> String {
    cli_version_from(std::env::var(CLI_VERSION_ENV).ok().as_deref())
}

fn cli_version_from(env_value: Option<&str>) -> String {
    env_value
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains(char::is_whitespace))
        .unwrap_or(DEFAULT_CLI_VERSION)
        .to_string()
}

/// `User-Agent` the official CLI sends: `grok-cli/<version>`.
pub fn cli_user_agent() -> String {
    format!("grok-cli/{}", cli_version())
}

/// Chat proxy base URL, honouring `GROK_CLI_CHAT_PROXY_BASE_URL`.
pub fn chat_proxy_base_url() -> String {
    std::env::var(CLI_CHAT_PROXY_BASE_URL_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| CLI_CHAT_PROXY_BASE_URL.to_string())
}

/// Grok CLI identity headers sent on every chat-proxy request (chat and
/// `/models`). `Authorization: Bearer <oidc token>` is added by the caller.
pub fn chat_proxy_identity_headers() -> Vec<(&'static str, String)> {
    let version = cli_version();
    vec![
        ("User-Agent", format!("grok-cli/{version}")),
        (HEADER_TOKEN_AUTH, TOKEN_AUTH_VALUE.to_string()),
        (HEADER_CLIENT_VERSION, version),
        (
            HEADER_CLIENT_IDENTIFIER,
            CLIENT_IDENTIFIER_VALUE.to_string(),
        ),
        (HEADER_CLIENT_SURFACE, "cli".to_string()),
    ]
}

/// Per-turn sampler headers the CLI adds to chat completions.
/// `conversation_id` should be stable for one Jcode session.
pub fn chat_proxy_turn_headers(
    model: &str,
    conversation_id: &str,
    request_id: &str,
) -> Vec<(&'static str, String)> {
    vec![
        (HEADER_MODEL_OVERRIDE, model.to_string()),
        (HEADER_CONV_ID, conversation_id.to_string()),
        (HEADER_REQ_ID, request_id.to_string()),
    ]
}

fn oauth_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("User-Agent", cli_user_agent())
        .header(HEADER_CLIENT_VERSION, cli_version())
        .header(HEADER_CLIENT_IDENTIFIER, CLIENT_IDENTIFIER_VALUE)
}

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
    exp: Option<i64>,
}

fn default_poll_interval() -> u64 {
    5
}

/// Start the xAI OAuth device flow with the Grok CLI client id.
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

/// Poll the device flow until approved and persist the resulting tokens.
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
            let path = auth_json_path().context("No home directory for Grok credentials")?;
            return store_tokens(&path, tokens, None);
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

fn jwt_claims(token: &str) -> JwtClaims {
    token
        .split('.')
        .nth(1)
        .and_then(|part| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part.trim_end_matches('='))
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<JwtClaims>(&bytes).ok())
        .unwrap_or_default()
}

/// Write tokens into the scoped `auth.json` entry. Existing fields on that
/// entry (team, retention flags written by the Grok CLI) and all other scopes
/// are preserved. `previous_refresh` is kept when the token endpoint does not
/// rotate the refresh token.
fn store_tokens(path: &Path, tokens: TokenResponse, previous_refresh: Option<&str>) -> Result<()> {
    store_tokens_at(path, tokens, previous_refresh, chrono::Utc::now())
}

fn store_tokens_at(
    path: &Path,
    tokens: TokenResponse,
    previous_refresh: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    let claims = jwt_claims(&tokens.access_token);
    let mut scopes = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok())
        .unwrap_or_default();
    let scope = credential_scope_key();
    let mut entry = match scopes.remove(&scope) {
        Some(Value::Object(entry)) => entry,
        _ => Map::new(),
    };
    entry.insert("key".into(), Value::String(tokens.access_token));
    entry.insert("auth_mode".into(), Value::String("oidc".into()));
    entry.insert("oidc_issuer".into(), Value::String(OAUTH_ISSUER.into()));
    entry.insert(
        "oidc_client_id".into(),
        Value::String(OAUTH_CLIENT_ID.into()),
    );
    entry
        .entry("create_time")
        .or_insert_with(|| Value::String(now.to_rfc3339()));
    if let Some(sub) = claims.sub {
        entry.insert("user_id".into(), Value::String(sub));
    }
    if let Some(email) = claims.email {
        entry.insert("email".into(), Value::String(email));
    }
    if let Some(name) = claims.given_name {
        entry.insert("first_name".into(), Value::String(name));
    }
    entry
        .entry("coding_data_retention_opt_out")
        .or_insert(Value::Bool(false));
    match tokens.refresh_token.as_deref().or(previous_refresh) {
        Some(refresh) => {
            entry.insert("refresh_token".into(), Value::String(refresh.to_string()));
        }
        None => {
            entry.remove("refresh_token");
        }
    }
    let expiry = tokens
        .expires_in
        .map(|seconds| now + chrono::Duration::seconds(seconds as i64))
        .or_else(|| {
            claims
                .exp
                .and_then(|exp| chrono::DateTime::from_timestamp(exp, 0))
        });
    match expiry {
        Some(expiry) => {
            entry.insert("expires_at".into(), Value::String(expiry.to_rfc3339()));
        }
        None => {
            entry.remove("expires_at");
        }
    }
    scopes.insert(scope, Value::Object(entry));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&scopes)?)?;
    crate::platform::set_permissions_owner_only(&temporary)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn grok_home(
    grok_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    grok_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.or(user_profile)
                .map(|home| PathBuf::from(home).join(".grok"))
        })
}

/// `$GROK_HOME/auth.json`, falling back to `~/.grok/auth.json`.
pub fn auth_json_path() -> Option<PathBuf> {
    grok_home(
        std::env::var_os("GROK_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
    )
    .map(|home| home.join("auth.json"))
}

fn credential_scope_key() -> String {
    format!("{OAUTH_ISSUER}::{OAUTH_CLIENT_ID}")
}

/// The Grok CLI OIDC entry selected from `auth.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrokCredential {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// `None` when neither `expires_at` nor the JWT `exp` claim is present.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl GrokCredential {
    pub fn needs_refresh_at(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.expires_at
            .is_some_and(|expiry| expiry <= now + chrono::Duration::seconds(EXPIRY_SKEW_SECS))
    }
}

/// Select only the `https://auth.x.ai::<grok-cli-client-id>` OIDC entry.
/// API-key entries, other issuers, and other clients are ignored.
pub fn parse_credential(bytes: &[u8]) -> Option<GrokCredential> {
    let Value::Object(scopes) = serde_json::from_slice(bytes).ok()? else {
        return None;
    };
    let entry = scopes.get(&credential_scope_key())?.as_object()?;
    let str_field = |name: &str| {
        entry
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    if !matches!(str_field("auth_mode"), None | Some("oidc")) {
        return None;
    }
    if str_field("oidc_issuer").is_some_and(|issuer| issuer != OAUTH_ISSUER)
        || str_field("oidc_client_id").is_some_and(|client| client != OAUTH_CLIENT_ID)
    {
        return None;
    }
    let access_token = str_field("key")?.to_string();
    let expires_at = match str_field("expires_at") {
        // An unparseable expiry is treated as already expired so we refresh.
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .map(|value| value.with_timezone(&chrono::Utc))
                .unwrap_or(chrono::DateTime::UNIX_EPOCH),
        ),
        None => jwt_claims(&access_token)
            .exp
            .and_then(|exp| chrono::DateTime::from_timestamp(exp, 0)),
    };
    Some(GrokCredential {
        access_token,
        refresh_token: str_field("refresh_token").map(ToOwned::to_owned),
        expires_at,
    })
}

pub fn load_credential() -> Option<GrokCredential> {
    parse_credential(&std::fs::read(auth_json_path()?).ok()?)
}

fn deployment_key() -> Option<String> {
    std::env::var("GROK_DEPLOYMENT_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Whether a Grok Build subscription credential is present (it may still need
/// a refresh). This is a local presence check, not a network validation.
pub fn has_cached_login() -> bool {
    deployment_key().is_some() || load_credential().is_some()
}

/// Bearer token for the chat proxy, refreshing first when expired or when
/// `force_refresh` is set (after a 401).
pub async fn bearer_token(force_refresh: bool) -> Result<String> {
    if let Some(key) = deployment_key() {
        return Ok(key);
    }
    let path = auth_json_path().context("No home directory for Grok credentials")?;
    let credential = std::fs::read(&path)
        .ok()
        .and_then(|bytes| parse_credential(&bytes))
        .with_context(|| {
            format!(
                "No Grok Build login found in {}. Run `jcode login --provider grok-build`",
                path.display()
            )
        })?;
    if !force_refresh && !credential.needs_refresh_at(chrono::Utc::now()) {
        return Ok(credential.access_token);
    }
    let refresh = credential.refresh_token.as_deref().with_context(|| {
        "Grok Build login expired and has no refresh token. Run `jcode login --provider grok-build`"
    })?;
    refresh_tokens(&crate::provider::shared_http_client(), &path, refresh).await?;
    load_credential()
        .map(|credential| credential.access_token)
        .context("Grok Build credential missing after refresh")
}

async fn refresh_tokens(client: &reqwest::Client, path: &Path, refresh: &str) -> Result<()> {
    refresh_tokens_at(
        client,
        &format!("{OAUTH_ISSUER}/oauth2/token"),
        path,
        refresh,
    )
    .await
}

async fn refresh_tokens_at(
    client: &reqwest::Client,
    token_url: &str,
    path: &Path,
    refresh: &str,
) -> Result<()> {
    let response = oauth_headers(client.post(token_url))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", OAUTH_CLIENT_ID),
        ])
        .send()
        .await
        .context("Grok Build token refresh request failed")?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        let error: TokenError = serde_json::from_slice(&body).unwrap_or(TokenError {
            error: status.to_string(),
            error_description: None,
        });
        bail!(
            "Grok Build token refresh failed: {}. Run `jcode login --provider grok-build`",
            error.error_description.unwrap_or(error.error)
        );
    }
    let tokens: TokenResponse =
        serde_json::from_slice(&body).context("invalid xAI refresh token response")?;
    store_tokens(path, tokens, Some(refresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scoped(entry: &str) -> String {
        format!(r#"{{"{}": {entry}}}"#, credential_scope_key())
    }

    fn fake_jwt(claims: &str) -> String {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!(
            "{}.{}.sig",
            engine.encode(r#"{"alg":"none"}"#),
            engine.encode(claims)
        )
    }

    #[test]
    fn selects_only_the_grok_cli_oidc_scope() {
        let json = format!(
            r#"{{
                "https://other.example::abc": {{"key":"wrong","auth_mode":"oidc"}},
                "https://auth.x.ai::some-other-client": {{"key":"wrong2"}},
                "{}": {{"key":"right","auth_mode":"oidc","refresh_token":"r1",
                        "oidc_issuer":"https://auth.x.ai","oidc_client_id":"{OAUTH_CLIENT_ID}"}}
            }}"#,
            credential_scope_key()
        );
        let credential = parse_credential(json.as_bytes()).unwrap();
        assert_eq!(credential.access_token, "right");
        assert_eq!(credential.refresh_token.as_deref(), Some("r1"));
        assert_eq!(credential.expires_at, None);
    }

    #[test]
    fn rejects_api_key_mode_mismatched_issuer_and_empty_keys() {
        assert!(parse_credential(b"{}").is_none());
        assert!(parse_credential(b"not json").is_none());
        assert!(parse_credential(br#"{"https://auth.x.ai::client":{"key":"token"}}"#).is_none());
        assert!(parse_credential(scoped(r#"{"key":""}"#).as_bytes()).is_none());
        assert!(
            parse_credential(scoped(r#"{"key":"k","auth_mode":"api_key"}"#).as_bytes()).is_none()
        );
        assert!(
            parse_credential(scoped(r#"{"key":"k","oidc_issuer":"https://evil"}"#).as_bytes())
                .is_none()
        );
        assert!(parse_credential(scoped(r#"{"key":"k"}"#).as_bytes()).is_some());
    }

    #[test]
    fn honours_expires_at_with_skew_and_jwt_exp_fallback() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-23T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let fresh = parse_credential(
            scoped(r#"{"key":"k","expires_at":"2026-09-23T13:00:00Z"}"#).as_bytes(),
        )
        .unwrap();
        assert!(!fresh.needs_refresh_at(now));
        let nearly = parse_credential(
            scoped(r#"{"key":"k","expires_at":"2026-09-23T12:00:30+00:00"}"#).as_bytes(),
        )
        .unwrap();
        assert!(nearly.needs_refresh_at(now), "inside the refresh skew");
        let garbage =
            parse_credential(scoped(r#"{"key":"k","expires_at":"soon"}"#).as_bytes()).unwrap();
        assert!(
            garbage.needs_refresh_at(now),
            "unparseable expiry refreshes"
        );

        let jwt = fake_jwt(&format!(r#"{{"exp":{}}}"#, now.timestamp() - 5));
        let from_jwt =
            parse_credential(scoped(&format!(r#"{{"key":"{jwt}"}}"#)).as_bytes()).unwrap();
        assert!(from_jwt.needs_refresh_at(now));
    }

    #[test]
    fn store_tokens_preserves_other_scopes_and_cli_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"https://other::x": {{"key":"keep"}},
                    "{}": {{"key":"old","refresh_token":"r-old","team_id":"t1"}}}}"#,
                credential_scope_key()
            ),
        )
        .unwrap();
        let now = chrono::Utc::now();
        store_tokens_at(
            &path,
            TokenResponse {
                access_token: fake_jwt(r#"{"sub":"u1","email":"a@b.c"}"#),
                refresh_token: None,
                expires_in: Some(3600),
            },
            Some("r-old"),
            now,
        )
        .unwrap();
        let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["https://other::x"]["key"], "keep");
        let entry = &raw[credential_scope_key().as_str()];
        assert_eq!(entry["team_id"], "t1");
        assert_eq!(entry["refresh_token"], "r-old");
        assert_eq!(entry["user_id"], "u1");
        assert_eq!(entry["auth_mode"], "oidc");
        let credential = parse_credential(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!credential.needs_refresh_at(now));
        assert!(credential.needs_refresh_at(now + chrono::Duration::seconds(3600)));
    }

    #[tokio::test]
    async fn refresh_posts_refresh_grant_and_rotates_tokens() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let mut request = String::new();
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                request.push_str(&String::from_utf8_lossy(&buf[..n]));
                if let Some(head_end) = request.find("\r\n\r\n") {
                    let length = request[..head_end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length {
                        break;
                    }
                }
            }
            let body = r#"{"access_token":"new-access","refresh_token":"r-new","expires_in":600}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, scoped(r#"{"key":"old","refresh_token":"r-old"}"#)).unwrap();
        refresh_tokens_at(
            &reqwest::Client::new(),
            &format!("http://{addr}/oauth2/token"),
            &path,
            "r-old",
        )
        .await
        .unwrap();
        let request = server.await.unwrap();
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=r-old"));
        assert!(request.contains(&format!("client_id={OAUTH_CLIENT_ID}")));
        assert!(
            request
                .to_ascii_lowercase()
                .contains(&format!("user-agent: grok-cli/{}", cli_version()))
        );
        let credential = parse_credential(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(credential.access_token, "new-access");
        assert_eq!(credential.refresh_token.as_deref(), Some("r-new"));
    }

    #[test]
    fn cli_version_env_override_is_a_single_token() {
        assert_eq!(cli_version_from(None), DEFAULT_CLI_VERSION);
        assert_eq!(cli_version_from(Some("  ")), DEFAULT_CLI_VERSION);
        assert_eq!(cli_version_from(Some("1.2 3")), DEFAULT_CLI_VERSION);
        assert_eq!(cli_version_from(Some(" 1.1.0 ")), "1.1.0");
    }

    #[test]
    fn chat_proxy_headers_identify_as_grok_cli() {
        let mut headers = chat_proxy_identity_headers();
        headers.extend(chat_proxy_turn_headers("grok-4.6", "conv-1", "req-1"));
        let get = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        };
        let version = cli_version();
        assert_eq!(
            get("user-agent"),
            Some(format!("grok-cli/{version}").as_str())
        );
        assert_eq!(get("x-xai-token-auth"), Some("xai-grok-cli"));
        assert_eq!(get("x-grok-client-version"), Some(version.as_str()));
        assert_eq!(get("x-grok-client-identifier"), Some("grok-shell"));
        assert_eq!(get("x-grok-model-override"), Some("grok-4.6"));
        assert_eq!(get("x-grok-conv-id"), Some("conv-1"));
        assert_eq!(get("x-grok-req-id"), Some("req-1"));
        assert!(get("authorization").is_none());
    }

    #[test]
    fn grok_home_prefers_env_then_home_then_user_profile() {
        assert_eq!(
            grok_home(
                Some("/tmp/grok-store".into()),
                Some("/home/me".into()),
                None
            ),
            Some(PathBuf::from("/tmp/grok-store"))
        );
        assert_eq!(
            grok_home(Some("".into()), Some("/home/me".into()), None),
            Some(PathBuf::from("/home/me/.grok"))
        );
        assert_eq!(
            grok_home(None, None, Some("C:\\Users\\jcode".into())),
            Some(PathBuf::from("C:\\Users\\jcode").join(".grok"))
        );
    }
}
