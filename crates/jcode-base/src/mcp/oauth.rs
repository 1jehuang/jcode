//! OAuth 2.1 for remote MCP servers (MCP authorization spec 2025-06-18).
//!
//! Protected-resource metadata binds discovery to the requested MCP resource.
//! Validated RFC 8414 metadata (or OpenID discovery) supplies endpoints. Login
//! uses public dynamic registration, an ephemeral loopback callback, state and
//! PKCE S256. Only explicit `login` opens a browser. Background access merely
//! reads or refreshes credentials, including tokens rejected before expiry.
//!
//! All network requests refuse redirects. HTTPS is required, with a development
//! exception for loopback HTTP when the MCP resource itself is loopback. Remote
//! metadata cannot opt into that exception. Credentials live in an atomically
//! replaced owner-only file, without backups, with a separate cross-process lock.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const STORE_FILE: &str = "mcp-oauth.json";
const STORE_VERSION: u32 = 1;
const EXPIRY_SKEW_SECS: i64 = 60;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const STORE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_CALLBACK_BYTES: usize = 16 * 1024;

/// One server's OAuth state. `Debug` deliberately does not expose credentials.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOAuth {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix seconds; `None` means the server did not report an expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

impl std::fmt::Debug for StoredOAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredOAuth")
            .field("has_access_token", &self.access_token.is_some())
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("has_client_secret", &self.client_secret.is_some())
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Secret-free state suitable for CLI/tool status output. No network requests.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OAuthStatus {
    pub logged_in: bool,
    pub access_token_fresh: bool,
    pub can_refresh: bool,
    pub expires_at: Option<i64>,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    version: u32,
    #[serde(default)]
    servers: BTreeMap<String, StoredOAuth>,
    /// Logout tombstones prevent in-flight login/refresh from resurrecting tokens.
    #[serde(default)]
    generations: BTreeMap<String, u64>,
}

fn generation(store: &Store, key: &str) -> u64 {
    store.generations.get(key).copied().unwrap_or(0)
}

fn advance_generation(store: &mut Store, key: &str) -> Result<()> {
    let next = generation(store, key)
        .checked_add(1)
        .context("OAuth credential revision overflow")?;
    store.generations.insert(key.to_string(), next);
    Ok(())
}

/// Canonical storage/resource key, without query, fragment or userinfo.
/// Invalid URLs have no key. Public credential APIs additionally validate URLs.
pub fn canonical_resource(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url.trim()) else {
        return String::new();
    };
    parsed.set_query(None);
    parsed.set_fragment(None);
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.to_string().trim_end_matches('/').to_string()
}

fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn checked_url(value: &str, allow_loopback: bool) -> Result<url::Url> {
    // Never include the supplied URL in errors: its query may be a credential.
    let parsed = url::Url::parse(value).map_err(|_| anyhow::anyhow!("invalid OAuth URL"))?;
    if parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        bail!("OAuth URLs must be absolute and must not contain userinfo or fragments");
    }
    if is_loopback(&parsed) && !allow_loopback {
        bail!("remote OAuth metadata must not target loopback endpoints");
    }
    if parsed.scheme() != "https"
        && !(allow_loopback && parsed.scheme() == "http" && is_loopback(&parsed))
    {
        bail!("OAuth requires HTTPS, except for explicitly configured loopback resources");
    }
    Ok(parsed)
}

/// Validate an MCP OAuth resource before login or credential access. HTTP is
/// accepted only for literal loopback addresses or `localhost`, not LAN hosts.
pub fn validate_mcp_url(url: &str) -> Result<()> {
    checked_url(url, true).map(|_| ())
}

fn allow_loopback(url: &str) -> Result<bool> {
    Ok(is_loopback(&checked_url(url, true)?))
}

fn store_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(STORE_FILE))
}

fn private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).context("could not create OAuth credential directory")?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("OAuth credential directory must be a real directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("OAuth credential directory is not owned by the current user");
        }
    }
    jcode_core::fs::set_directory_permissions_owner_only(path)
        .context("could not secure OAuth credential directory")
}

fn secure_file(file: &File, path: &Path) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("OAuth credential path must be a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1 {
            bail!("OAuth credential file must be owned by this user and not hard-linked");
        }
        // Repair legacy permissive files through the already-open descriptor.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        let _ = path;
    }
    #[cfg(not(unix))]
    jcode_core::fs::set_permissions_owner_only(path)?;
    Ok(())
}

fn secret_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options
}

fn load_store_at(path: &Path) -> Result<Store> {
    // Windows has no O_NOFOLLOW. Reject obvious symlinks before opening there.
    #[cfg(not(unix))]
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        bail!("OAuth credential file must not be a symlink");
    }
    let mut file = match secret_options().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Store {
                version: STORE_VERSION,
                ..Default::default()
            });
        }
        Err(_) => bail!("could not open OAuth credential store"),
    };
    secure_file(&file, path)?;
    if file.metadata()?.len() > MAX_JSON_BYTES as u64 {
        bail!("OAuth credential store is too large");
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_JSON_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_JSON_BYTES {
        bail!("OAuth credential store is too large");
    }
    let store: Store = serde_json::from_slice(&bytes).map_err(|_| {
        anyhow::anyhow!("OAuth credential store is invalid; refusing to overwrite it")
    })?;
    if store.version != STORE_VERSION {
        bail!("unsupported OAuth credential store version");
    }
    Ok(store)
}

fn load_store() -> Result<Store> {
    load_store_at(&store_path()?)
}

fn save_store_at(path: &Path, store: &Store) -> Result<()> {
    let parent = path.parent().context("OAuth store has no directory")?;
    private_directory(parent)?;
    // The general storage writer keeps .bak copies. OAuth must not leave old
    // refresh tokens or logged-out credentials in such a backup.
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    jcode_core::fs::set_permissions_owner_only(temp.path())?;
    let bytes = serde_json::to_vec(store)?;
    if bytes.len() > MAX_JSON_BYTES {
        bail!("OAuth credential store is too large");
    }
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|_| anyhow::anyhow!("could not replace OAuth credential store"))?;
    let backup = path.with_extension("bak");
    match std::fs::remove_file(backup) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => bail!("could not remove legacy OAuth credential backup"),
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn open_store_lock(path: &Path) -> Result<File> {
    open_lock_file(&path.with_extension("lock"))
}

fn open_lock_file(lock_path: &Path) -> Result<File> {
    private_directory(lock_path.parent().context("OAuth lock has no directory")?)?;
    #[cfg(not(unix))]
    if let Ok(meta) = std::fs::symlink_metadata(lock_path)
        && meta.file_type().is_symlink()
    {
        bail!("OAuth lock must not be a symlink");
    }
    let lock = secret_options()
        .read(true)
        .write(true)
        .create(true)
        .open(lock_path)
        .context("could not open OAuth credential lock")?;
    secure_file(&lock, lock_path)?;
    Ok(lock)
}

async fn lock_store(path: &Path) -> Result<File> {
    wait_lock(open_store_lock(path)?).await
}

async fn lock_refresh(path: &Path, resource: &str) -> Result<File> {
    let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(resource.as_bytes()));
    let lock_path = path.with_file_name(format!("mcp-oauth-refresh-{digest}.lock"));
    wait_lock(open_lock_file(&lock_path)?).await
}

async fn wait_lock(file: File) -> Result<File> {
    tokio::time::timeout(STORE_LOCK_TIMEOUT, async {
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(()),
                Err(TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(TryLockError::Error(_)) => bail!("could not lock OAuth credential store"),
            }
        }
    })
    .await
    .context("timed out locking OAuth credential store")??;
    // Dropping the handle releases the lock, including on future cancellation.
    Ok(file)
}

fn checked_stored(url: &str) -> Result<Option<StoredOAuth>> {
    validate_mcp_url(url)?;
    Ok(load_store()?.servers.get(&canonical_resource(url)).cloned())
}

/// Compatibility accessor. Use `status` when a malformed/unreadable store must
/// be distinguished from being logged out. Never print this object's fields.
pub fn stored(url: &str) -> Option<StoredOAuth> {
    checked_stored(url).ok().flatten()
}

pub fn status(url: &str) -> Result<OAuthStatus> {
    let Some(entry) = checked_stored(url)? else {
        return Ok(OAuthStatus::default());
    };
    Ok(OAuthStatus {
        logged_in: entry.access_token.is_some() || entry.refresh_token.is_some(),
        access_token_fresh: is_fresh(&entry),
        can_refresh: entry.refresh_token.is_some(),
        expires_at: entry.expires_at,
    })
}

/// Forget stored credentials. Local deletion does not revoke provider tokens.
/// A concurrent refresh/login causes a retryable error, never credential
/// resurrection or blocking a single-threaded async runtime on its own refresh.
pub fn logout(url: &str) -> Result<bool> {
    validate_mcp_url(url)?;
    let path = store_path()?;
    let file = open_store_lock(&path)?;
    file.try_lock()
        .map_err(|_| anyhow::anyhow!("OAuth credential store is busy; retry logout"))?;
    let mut store = load_store_at(&path)?;
    let key = canonical_resource(url);
    let removed = store.servers.remove(&key).is_some();
    advance_generation(&mut store, &key)?;
    // Also removes a legacy backup even when the primary has no such entry.
    save_store_at(&path, &store)?;
    Ok(removed)
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn is_fresh(entry: &StoredOAuth) -> bool {
    entry
        .access_token
        .as_deref()
        .is_some_and(|token| !token.is_empty())
        && entry
            .expires_at
            .is_none_or(|exp| exp.saturating_sub(EXPIRY_SKEW_SECS) > now_secs())
}

/// Current token, refreshing when near expiry. Known-expired credentials are
/// never returned without a successful refresh. Never opens a browser.
pub async fn access_token(url: &str) -> Result<Option<String>> {
    let Some(entry) = checked_stored(url)? else {
        return Ok(None);
    };
    if is_fresh(&entry) {
        return Ok(entry.access_token);
    }
    if entry.refresh_token.is_some() {
        return refresh(url, RefreshReason::Expired).await;
    }
    Ok(entry
        .access_token
        .filter(|token| !token.is_empty() && entry.expires_at.is_none_or(|exp| exp > now_secs())))
}

/// A 401 overrides the expiry hint. Under the cross-process lock, reuse a token
/// only if it differs from the rejected one and is fresh, otherwise force a
/// refresh. `Ok(None)` means login is needed. Never retries the rejected token.
pub async fn token_after_unauthorized(url: &str, failed: Option<&str>) -> Result<Option<String>> {
    refresh(url, RefreshReason::Rejected(failed.map(str::to_owned))).await
}

enum RefreshReason {
    Expired,
    Rejected(Option<String>),
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

fn apply_token(entry: &mut StoredOAuth, token: TokenResponse) -> Result<()> {
    if !token.token_type.eq_ignore_ascii_case("Bearer")
        || token.access_token.is_empty()
        || token
            .access_token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || token.refresh_token.as_deref().is_some_and(str::is_empty)
        || token.expires_in.is_some_and(|secs| secs < 0)
    {
        bail!("token endpoint returned invalid bearer credentials");
    }
    let expires_at = token
        .expires_in
        .map(|secs| {
            now_secs()
                .checked_add(secs)
                .context("token endpoint returned invalid expiry")
        })
        .transpose()?;
    entry.access_token = Some(token.access_token);
    if token.refresh_token.is_some() {
        entry.refresh_token = token.refresh_token;
    }
    entry.expires_at = expires_at;
    if token.scope.is_some() {
        entry.scope = token.scope;
    }
    Ok(())
}

async fn response_bytes(mut response: reqwest::Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_JSON_BYTES as u64)
    {
        bail!("OAuth response is too large");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("could not read OAuth response"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_JSON_BYTES {
            bail!("OAuth response is too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn post_token(
    entry: &StoredOAuth,
    resource: &str,
    mut form: Vec<(&'static str, String)>,
) -> Result<TokenResponse> {
    let endpoint = checked_url(&entry.token_endpoint, allow_loopback(resource)?)?;
    if entry
        .resource
        .as_deref()
        .is_some_and(|stored| canonical_resource(stored) != canonical_resource(resource))
    {
        bail!("stored OAuth credentials belong to a different resource");
    }
    if entry.client_id.is_empty() {
        bail!("stored OAuth client identifier is empty");
    }
    form.push(("client_id", entry.client_id.clone()));
    // Legacy stored registrations may have used client_secret_post. New login
    // explicitly registers a public client and never stores a returned secret.
    if let Some(secret) = &entry.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    form.push((
        "resource",
        entry
            .resource
            .clone()
            .unwrap_or_else(|| canonical_resource(resource)),
    ));
    let response = http_for(&endpoint)
        .post(endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("OAuth token request failed"))?;
    let status = response.status();
    if !status.is_success() {
        // Do not echo a body, redirect Location, error_description or reqwest
        // URL. Providers can reflect any of our secret form fields there.
        bail!("OAuth token endpoint returned HTTP {status}; login may be required");
    }
    serde_json::from_slice(&response_bytes(response).await?)
        .map_err(|_| anyhow::anyhow!("OAuth token endpoint returned invalid JSON"))
}

async fn refresh(url: &str, reason: RefreshReason) -> Result<Option<String>> {
    validate_mcp_url(url)?;
    if checked_stored(url)?.is_none() {
        return Ok(None);
    }
    let path = store_path()?;
    let key = canonical_resource(url);
    // Only async refreshers take this per-resource lock. Sync logout never
    // waits on it, and the transaction lock is not held over network I/O.
    let _refresh_guard = lock_refresh(&path, &key).await?;
    let (mut entry, revision) = {
        let _transaction = lock_store(&path).await?;
        let store = load_store_at(&path)?;
        let Some(entry) = store.servers.get(&key).cloned() else {
            return Ok(None);
        };
        let reuse = match &reason {
            RefreshReason::Expired => is_fresh(&entry),
            RefreshReason::Rejected(failed) => entry.access_token != *failed && is_fresh(&entry),
        };
        if reuse {
            return Ok(entry.access_token);
        }
        (entry, generation(&store, &key))
    };
    let Some(refresh_token) = entry.refresh_token.clone() else {
        return Ok(None);
    };
    let snapshot = entry.clone();
    let token = post_token(
        &entry,
        url,
        vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token),
        ],
    )
    .await?;
    apply_token(&mut entry, token)?;
    let _transaction = lock_store(&path).await?;
    let mut store = load_store_at(&path)?;
    if generation(&store, &key) != revision || store.servers.get(&key) != Some(&snapshot) {
        // Logout or a newer login won while the grant was in flight.
        return Ok(store
            .servers
            .get(&key)
            .filter(|current| is_fresh(current))
            .and_then(|current| current.access_token.clone())
            .filter(|current| match &reason {
                RefreshReason::Rejected(failed) => Some(current) != failed.as_ref(),
                RefreshReason::Expired => true,
            }));
    }
    let access_token = entry
        .access_token
        .clone()
        .filter(|_| entry.expires_at.is_none_or(|expiry| expiry > now_secs()));
    store.servers.insert(key.clone(), entry);
    advance_generation(&mut store, &key)?;
    save_store_at(&path, &store)?;
    if let RefreshReason::Rejected(failed) = reason
        && access_token == failed
    {
        return Ok(None);
    }
    Ok(access_token)
}

fn http_for(endpoint: &url::Url) -> reqwest::Client {
    if is_loopback(endpoint) {
        return http();
    }
    static REMOTE_HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    REMOTE_HTTP
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(DISCOVERY_TIMEOUT)
                .build()
                .expect("OAuth HTTP client configuration is valid")
        })
        .clone()
}

fn http() -> reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // Never proxy plaintext loopback credentials via environment proxies.
            .no_proxy()
            // Pin localhost rather than trusting a DNS lookup for this exception.
            .resolve("localhost", std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .timeout(DISCOVERY_TIMEOUT)
            .build()
            .expect("OAuth HTTP client configuration is valid")
    })
    .clone()
}

// Discovery -----------------------------------------------------------------

#[derive(Deserialize)]
struct ProtectedResourceMetadata {
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
    resource: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    pub authorization_response_iss_parameter_supported: bool,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
}

/// Parse parameters only from a Bearer challenge, respecting quoted commas and
/// quoted-pair escaping, optional whitespace, and case-insensitive names. A
/// duplicate requested parameter is ambiguous and is deliberately rejected.
pub(crate) fn www_authenticate_param(header: &str, key: &str) -> Option<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in header.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'"' {
            quoted = !quoted;
        }
        if byte == b',' && !quoted {
            chunks.push(&header[start..index]);
            start = index + 1;
        }
    }
    if quoted || escaped {
        return None;
    }
    chunks.push(&header[start..]);
    let mut bearer = false;
    let mut found = None;
    for chunk in chunks {
        let mut item = chunk.trim();
        let first_end = item
            .find(|ch: char| ch.is_ascii_whitespace() || ch == '=')
            .unwrap_or(item.len());
        let first = &item[..first_end];
        let rest = item[first_end..].trim_start();
        if !rest.starts_with('=') {
            bearer = first.eq_ignore_ascii_case("Bearer");
            item = rest;
        }
        if !bearer {
            continue;
        }
        let Some((name, value)) = item.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case(key) {
            continue;
        }
        if found.is_some() {
            return None;
        }
        let value = value.trim();
        let decoded = if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        {
            let mut decoded = String::new();
            let mut chars = inner.chars();
            while let Some(ch) = chars.next() {
                decoded.push(if ch == '\\' { chars.next()? } else { ch });
            }
            decoded
        } else {
            if value.is_empty()
                || value
                    .chars()
                    .any(|ch| ch.is_ascii_whitespace() || ch == '"')
            {
                return None;
            }
            value.to_string()
        };
        found = Some(decoded);
    }
    found
}

/// Protected-resource metadata tries path-specific discovery, then the origin.
fn well_known(base: &url::Url, suffix: &str) -> Vec<String> {
    let origin = base.origin().ascii_serialization();
    let path = base.path().trim_end_matches('/');
    let mut urls = Vec::new();
    if !path.is_empty() {
        urls.push(format!("{origin}/.well-known/{suffix}{path}"));
    }
    urls.push(format!("{origin}/.well-known/{suffix}"));
    urls
}

fn auth_metadata_urls(issuer: &url::Url) -> Vec<String> {
    let origin = issuer.origin().ascii_serialization();
    let path = issuer.path().trim_end_matches('/');
    let mut urls = vec![
        format!("{origin}/.well-known/oauth-authorization-server{path}"),
        format!("{origin}/.well-known/openid-configuration{path}"),
    ];
    if !path.is_empty() {
        urls.push(format!("{origin}{path}/.well-known/openid-configuration"));
    }
    urls
}

async fn get_json<T: for<'de> Deserialize<'de>>(value: &str, local: bool) -> Result<Option<T>> {
    let endpoint = checked_url(value, local)?;
    let response = http_for(&endpoint)
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("OAuth metadata request failed"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND
        || response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED
    {
        return Ok(None);
    }
    if !response.status().is_success() {
        bail!(
            "OAuth metadata endpoint returned HTTP {}",
            response.status()
        );
    }
    serde_json::from_slice(&response_bytes(response).await?)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("OAuth metadata endpoint returned invalid JSON"))
}

fn validate_auth_metadata(meta: &AuthServerMetadata, issuer: &str, local: bool) -> Result<()> {
    if meta.issuer != issuer {
        bail!("OAuth metadata issuer does not match the discovered issuer");
    }
    checked_url(&meta.authorization_endpoint, local)?;
    checked_url(&meta.token_endpoint, local)?;
    if let Some(endpoint) = &meta.registration_endpoint {
        checked_url(endpoint, local)?;
    }
    // The 2025-06-18 baseline predates mandatory capability advertisement, so
    // absence is compatible. An explicit list excluding S256 is not.
    if meta
        .code_challenge_methods_supported
        .as_ref()
        .is_some_and(|methods| !methods.iter().any(|method| method == "S256"))
    {
        bail!("authorization server does not support PKCE S256");
    }
    Ok(())
}

struct Discovery {
    auth_server: AuthServerMetadata,
    scope: Option<String>,
    resource: String,
    local: bool,
}

async fn discover(mcp_url: &str) -> Result<Discovery> {
    let parsed = checked_url(mcp_url, true)?;
    let local = is_loopback(&parsed);
    let resource = canonical_resource(mcp_url);
    let probe = http_for(&parsed)
        .post(parsed.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .body(r#"{"jsonrpc":"2.0","id":0,"method":"ping"}"#)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("MCP OAuth discovery probe failed"))?;
    if probe.status().is_redirection() {
        bail!("MCP OAuth discovery refuses redirects");
    }
    let challenges = if probe.status() == reqwest::StatusCode::UNAUTHORIZED {
        probe
            .headers()
            .get_all(reqwest::header::WWW_AUTHENTICATE)
            .iter()
            .filter_map(|header| header.to_str().ok())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut scope = challenges
        .iter()
        .find_map(|header| www_authenticate_param(header, "scope"));
    let advertised = challenges
        .iter()
        .find_map(|header| www_authenticate_param(header, "resource_metadata"));
    let prm_urls = advertised
        .map(|url| vec![url])
        .unwrap_or_else(|| well_known(&parsed, "oauth-protected-resource"));
    let mut prm = None;
    for endpoint in prm_urls {
        if let Some(meta) = get_json::<ProtectedResourceMetadata>(&endpoint, local).await? {
            let resource_url = checked_url(&meta.resource, local)?;
            if resource_url.query().is_some() || canonical_resource(&meta.resource) != resource {
                bail!("OAuth protected-resource metadata does not match the MCP resource");
            }
            if meta.authorization_servers.is_empty() {
                bail!("OAuth resource metadata has no authorization servers");
            }
            prm = Some(meta);
            break;
        }
    }
    let prm = prm.context("MCP server does not publish OAuth protected-resource metadata")?;
    if scope.is_none() && !prm.scopes_supported.is_empty() {
        scope = Some(prm.scopes_supported.join(" "));
    }
    let issuer = &prm.authorization_servers[0];
    let issuer_url = checked_url(issuer, local)?;
    if issuer_url.query().is_some() {
        bail!("OAuth issuer must not contain a query");
    }
    let mut auth_server = None;
    for endpoint in auth_metadata_urls(&issuer_url) {
        if let Some(meta) = get_json::<AuthServerMetadata>(&endpoint, local).await? {
            validate_auth_metadata(&meta, issuer, local)?;
            auth_server = Some(meta);
            break;
        }
    }
    Ok(Discovery {
        auth_server: auth_server.context("authorization server does not publish OAuth metadata")?,
        scope,
        resource: prm.resource,
        local,
    })
}

// Login ---------------------------------------------------------------------

#[derive(Deserialize)]
struct RegistrationResponse {
    client_id: String,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

async fn register_client(
    endpoint: &str,
    redirect_uri: &str,
    local: bool,
) -> Result<RegistrationResponse> {
    let endpoint = checked_url(endpoint, local)?;
    let body = serde_json::json!({
        "client_name": "jcode",
        "client_uri": "https://github.com/1jehuang/jcode",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let response = http_for(&endpoint)
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("OAuth client registration request failed"))?;
    if !response.status().is_success() {
        bail!(
            "OAuth client registration returned HTTP {}",
            response.status()
        );
    }
    let client: RegistrationResponse = serde_json::from_slice(&response_bytes(response).await?)
        .map_err(|_| anyhow::anyhow!("OAuth client registration returned invalid JSON"))?;
    if client.client_id.is_empty()
        || client
            .token_endpoint_auth_method
            .as_deref()
            .is_some_and(|method| method != "none")
    {
        bail!("OAuth registration did not provide a public client");
    }
    Ok(client)
}

fn pkce_pair() -> (String, String) {
    let bytes: [u8; 32] = rand::random();
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

pub enum BrowserMode {
    Open,
    Print,
}

// Invalid local requests are ignored, including error callbacks with the wrong
// state. Bound method, callback path, duplicate fields and optional RFC 9207
// issuer checks to this flow before interpreting any provider response.
fn callback_result(target: &str, state: &str, meta: &AuthServerMetadata) -> Result<Option<String>> {
    if !target.starts_with("/callback?") {
        return Ok(None);
    }
    let callback = url::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| anyhow::anyhow!("invalid OAuth callback"))?;
    if callback.path() != "/callback" || callback.fragment().is_some() {
        return Ok(None);
    }
    let mut values = BTreeMap::new();
    for (key, value) in callback.query_pairs() {
        if matches!(key.as_ref(), "state" | "code" | "error" | "iss")
            && values
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return Ok(None);
        }
    }
    if values.get("state").map(String::as_str) != Some(state) {
        return Ok(None);
    }
    match values.get("iss") {
        Some(issuer) if issuer != &meta.issuer => bail!("OAuth callback issuer mismatch"),
        None if meta.authorization_response_iss_parameter_supported => {
            bail!("OAuth callback missing issuer")
        }
        _ => (),
    }
    if values.contains_key("error") {
        bail!("OAuth authorization was denied or cancelled");
    }
    Ok(values.remove("code").filter(|code| !code.is_empty()))
}

async fn callback_request(stream: &mut tokio::net::TcpStream) -> Result<Option<String>> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_CALLBACK_BYTES {
            return Ok(None);
        }
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            break;
        }
    }
    let Some(line) = bytes.split(|byte| *byte == b'\n').next() else {
        return Ok(None);
    };
    let Ok(line) = std::str::from_utf8(line) else {
        return Ok(None);
    };
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "GET" || !matches!(parts[2], "HTTP/1.0" | "HTTP/1.1") {
        return Ok(None);
    }
    Ok(Some(parts[1].to_owned()))
}

async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    state: &str,
    meta: &AuthServerMetadata,
) -> Result<String> {
    use tokio::io::AsyncWriteExt;
    loop {
        let (mut stream, peer) = listener.accept().await?;
        if !peer.ip().is_loopback() {
            continue;
        }
        let target =
            tokio::time::timeout(Duration::from_secs(2), callback_request(&mut stream)).await;
        let result = match target {
            Ok(Ok(Some(target))) => callback_result(&target, state, meta),
            _ => Ok(None),
        };
        let success = matches!(&result, Ok(Some(_)));
        let (status, body) = if success {
            ("200 OK", "Login complete. Return to jcode.")
        } else {
            ("400 Bad Request", "Invalid or unsuccessful OAuth callback.")
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            stream.write_all(response.as_bytes()),
        )
        .await;
        match result {
            Ok(Some(code)) => return Ok(code),
            Err(error) => return Err(error),
            Ok(None) => (),
        }
    }
}

/// Explicit interactive login. `on_url` receives the authorization URL once it
/// is ready, even in `Print` mode. Only this API can open a browser. The supplied
/// URL contains state, so callers must show it intentionally, not log it.
pub async fn login(mcp_url: &str, browser: BrowserMode, on_url: impl FnOnce(&str)) -> Result<()> {
    validate_mcp_url(mcp_url)?;
    let path = store_path()?;
    let key = canonical_resource(mcp_url);
    let initial_generation = generation(&load_store_at(&path)?, &key);
    let discovery = discover(mcp_url).await?;
    let listener = crate::auth::oauth::bind_callback_listener(0)
        .context("could not bind OAuth loopback callback")?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    if discovery
        .auth_server
        .token_endpoint_auth_methods_supported
        .as_ref()
        .is_some_and(|methods| !methods.iter().any(|method| method == "none"))
    {
        bail!("authorization server does not support public OAuth clients");
    }
    let registration_endpoint = discovery.auth_server.registration_endpoint.as_deref()
        .context("authorization server does not support dynamic registration; configure static authorization instead")?;
    let client = register_client(registration_endpoint, &redirect_uri, discovery.local).await?;
    let (verifier, challenge) = pkce_pair();
    let state = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let mut auth_url = checked_url(
        &discovery.auth_server.authorization_endpoint,
        discovery.local,
    )?;
    // Metadata query parameters must not supply duplicates of our security
    // fields. Preserve other endpoint parameters, e.g. provider tenant hints.
    if auth_url.query_pairs().any(|(key, _)| {
        matches!(
            key.as_ref(),
            "response_type"
                | "client_id"
                | "redirect_uri"
                | "code_challenge"
                | "code_challenge_method"
                | "state"
                | "resource"
                | "scope"
        )
    }) {
        bail!("authorization endpoint contains reserved OAuth parameters");
    }
    {
        let mut query = auth_url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &discovery.resource);
        if let Some(scope) = &discovery.scope {
            query.append_pair("scope", scope);
        }
    }
    let auth_url = auth_url.to_string();
    on_url(&auth_url);
    if matches!(browser, BrowserMode::Open) {
        let _ = open::that(&auth_url);
    }
    let code = tokio::time::timeout(
        LOGIN_TIMEOUT,
        wait_for_callback(listener, &state, &discovery.auth_server),
    )
    .await
    .context("timed out waiting for OAuth browser login")??;
    let mut entry = StoredOAuth {
        client_id: client.client_id,
        token_endpoint: discovery.auth_server.token_endpoint,
        scope: discovery.scope,
        resource: Some(discovery.resource),
        ..Default::default()
    };
    let token = post_token(
        &entry,
        mcp_url,
        vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ],
    )
    .await?;
    apply_token(&mut entry, token)?;
    let _guard = lock_store(&path).await?;
    let mut store = load_store_at(&path)?;
    if generation(&store, &key) != initial_generation {
        bail!("OAuth login was superseded by logout or a newer login; retry login");
    }
    store.servers.insert(key.clone(), entry);
    advance_generation(&mut store, &key)?;
    save_store_at(&path, &store)
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
