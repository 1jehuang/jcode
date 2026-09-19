//! Command Code auth: one-time auth.json snapshot import with /alpha/whoami
//! gate, browser OAuth loopback add/replace, locked atomic persistence (D-01..D-04).

use anyhow::{Context, Result};
use chrono::Utc;
use jcode_base::auth::account_store::write_json_secret_locked;
use jcode_provider_command_code::{
    OAUTH_LOOPBACK_HOST, OAUTH_LOOPBACK_PORT, OAUTH_TIMEOUT_SECS, PROVIDER_KEY, WHOAMI_URL,
    WhoamiIdentity, whoami_identity_valid,
};
use serde::{Deserialize, Serialize};

/// One persisted Command Code account. Keys are long-lived; refresh semantics
/// intentionally return the same key (D-04, no invented refresh lifecycle).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandCodeAccount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub api_key: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_name: Option<String>,
}

impl CommandCodeAccount {
    /// The refresh credential is the same long-lived key (D-04).
    pub fn refresh_key(&self) -> String {
        self.api_key.clone()
    }
}

/// Whoami gate with live HTTP: GET /alpha/whoami with the bearer key and
/// require a non-empty user id and username before any persistence happens.
pub async fn verify_whoami(client: &reqwest::Client, api_key: &str) -> Result<WhoamiIdentity> {
    let response = client
        .get(WHOAMI_URL)
        .bearer_auth(api_key)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("Command Code /alpha/whoami request failed (network)")?;
    let status = response.status();
    let head = response.text().await.unwrap_or_default();
    let head = head.chars().take(512).collect::<String>();
    if !status.is_success() {
        anyhow::bail!(
            "Command Code /alpha/whoami failed: {} (head: {})",
            status,
            head
        );
    }
    let identity: WhoamiIdentity = serde_json::from_str(&head).with_context(|| {
        format!(
            "Command Code /alpha/whoami returned unparseable body: {}",
            head
        )
    })?;
    verify_identity(&identity)?;
    Ok(identity)
}

const CR: u8 = 13;
const LF: u8 = 10;

/// One OAuth loopback callback query payload.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthCallback {
    pub api_key: String,
    pub state: String,
    pub user_id: String,
    pub user_name: String,
    pub key_name: Option<String>,
}

/// Constant-time-ish state comparison for the OAuth loopback gate. Empty
/// values never match anything.
pub fn oauth_state_matches(expected: &str, actual: &str) -> bool {
    if expected.is_empty() || actual.is_empty() || expected.len() != actual.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(actual.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Issue a fresh 32-character alphanumeric OAuth state value.
pub fn issue_oauth_state() -> String {
    use rand::Rng;
    use rand::distr::Alphanumeric;
    let mut rng = rand::rng();
    (0..32).map(|_| rng.sample(Alphanumeric) as char).collect()
}

/// Minimal percent-decoding for query values (plus as space and two-hex
/// escapes); malformed escapes keep the raw bytes.
#[allow(dead_code)]
fn urlencode_decode(value: &str) -> String {
    let raw = value.replace('+', " ");
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Parse an HTTP request head (request line plus optional headers) into an
/// OAuthCallback, extracting apiKey/state/userId/userName/keyName from the
/// query string. Returns None for anything malformed or missing fields.
pub fn parse_callback_from_request(request_head: &str) -> Option<OAuthCallback> {
    let request_line = request_head.lines().next()?;
    if request_line.split_whitespace().next()? != "POST" {
        return None;
    }
    let path = request_line.split_whitespace().nth(1)?;
    if path != "/callback" {
        return None;
    }
    let content_type = request_head
        .lines()
        .find_map(|line| line.strip_prefix("Content-Type:").map(str::trim))
        .unwrap_or("");
    if !content_type.eq_ignore_ascii_case("application/json") {
        return None;
    }
    let body = request_head.split("\r\n\r\n").nth(1).unwrap_or("");
    let object = serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .as_object()?
        .clone();
    let api_key = object.get("apiKey")?.as_str()?.to_string();
    let state = object.get("state")?.as_str()?.to_string();
    let user_id = object
        .get("userId")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let user_name = object.get("userName")?.as_str()?.to_string();
    let key_name = object
        .get("keyName")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if api_key.is_empty() || state.is_empty() || user_name.is_empty() {
        return None;
    }
    Some(OAuthCallback {
        api_key,
        state,
        user_id,
        user_name,
        key_name,
    })
}

/// Build an HTTP/1.1 JSON reply body without relying on escaped CR/LF
/// literals in source.
fn http_reply(status_line: &str, body: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(status_line.as_bytes());
    out.push(CR);
    out.push(LF);
    out.extend_from_slice(b"Content-Type: application/json");
    out.push(CR);
    out.push(LF);
    out.extend_from_slice(format!("Content-Length: {}", body.len()).as_bytes());
    out.push(CR);
    out.push(LF);
    out.push(CR);
    out.push(LF);
    out.extend_from_slice(body.as_bytes());
    out
}

/// Hand-built JSON body so no escaped quotes are needed in source.
fn json_success_body(success: bool) -> String {
    const DQ: char = 34 as char;
    format!(
        "{{{}success{}:{}}}",
        DQ,
        DQ,
        if success { "true" } else { "false" }
    )
}

/// Explicit OAuth add/replace flow: verify the state value first, then the
/// live whoami identity, then persist under the requested label semantics
/// (D-02, D-03).
pub async fn oauth_add_or_replace(
    store_path: &std::path::Path,
    callback: OAuthCallback,
    expected_state: &str,
    client: &reqwest::Client,
    requested_label: Option<String>,
) -> Result<String> {
    anyhow::ensure!(
        oauth_state_matches(expected_state, &callback.state),
        "Command Code OAuth state mismatch; rejecting callback"
    );
    let identity = verify_whoami(client, &callback.api_key).await?;
    let account = CommandCodeAccount {
        label: None,
        api_key: callback.api_key,
        user_id: if callback.user_id.is_empty() {
            identity.user.id.clone()
        } else {
            callback.user_id
        },
        user_name: callback.user_name,
        org_id: identity.user.org_id.clone(),
        key_name: callback.key_name,
    };
    persist_verified_account(store_path, account, &identity, requested_label)
}

/// Bind the loopback listener and poll for one browser callback until the
/// deadline, replying JSON on every parseable request and looping on state
/// mismatch. The std listener runs on a blocking task so async callers never
/// stall the runtime.
pub async fn await_oauth_callback(expected_state: &str) -> Result<OAuthCallback> {
    let expected_state = expected_state.to_string();
    tokio::task::spawn_blocking(move || blocking_oauth_accept(&expected_state))
        .await
        .context("OAuth callback task panicked")?
}

fn blocking_oauth_accept(expected_state: &str) -> Result<OAuthCallback> {
    let listener = std::net::TcpListener::bind((OAUTH_LOOPBACK_HOST, OAUTH_LOOPBACK_PORT))
        .context("binding Command Code OAuth loopback listener 127.0.0.1:5959")?;
    listener
        .set_nonblocking(true)
        .context("loopback listener nonblocking")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(OAUTH_TIMEOUT_SECS);
    let ok_reply = http_reply("HTTP/1.1 200 OK", &json_success_body(true));
    let bad_reply = http_reply("HTTP/1.1 400 Bad Request", &json_success_body(false));
    loop {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "Command Code OAuth callback timed out after {}s",
                OAUTH_TIMEOUT_SECS
            );
        }
        match listener.accept() {
            Ok((mut socket, _addr)) => {
                let _ = socket.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buffer = [0u8; 4096];
                let mut head = String::new();
                if let Ok(read) = std::io::Read::read(&mut socket, &mut buffer) {
                    head.push_str(&String::from_utf8_lossy(&buffer[..read]));
                }
                if let Some(callback) = parse_callback_from_request(&head) {
                    if oauth_state_matches(expected_state, &callback.state) {
                        let _ = std::io::Write::write_all(&mut socket, &ok_reply);
                        return Ok(callback);
                    }
                    let _ = std::io::Write::write_all(&mut socket, &bad_reply);
                } else {
                    let _ = std::io::Write::write_all(&mut socket, &bad_reply);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(err) => return Err(anyhow::Error::new(err).context("loopback accept failed")),
        }
    }
}

/// Storage file for the daemon-authoritative credential store.
pub fn auth_store_path() -> Result<std::path::PathBuf> {
    Ok(jcode_base::storage::app_config_dir()?.join("command_code_accounts.json"))
}

/// Resolve the daemon-authoritative active account for provider construction.
pub fn active_account() -> Option<CommandCodeAccount> {
    let path = auth_store_path().ok()?;
    let store = CommandCodeStore::load(&path);
    let label = store.active.as_deref();
    store
        .accounts
        .into_iter()
        .find(|account| label.is_none() || account.label.as_deref() == label)
}

/// The credential check happens on every /alpha/whoami flow; tests inject a
/// mock. A persisted key always returns the long-lived credential directly.

/// Locked, daemon-authoritative store for Command Code accounts.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CommandCodeStore {
    #[serde(default)]
    pub accounts: Vec<CommandCodeAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// One-time marker for the external auth.json snapshot import (D-01).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_at: Option<String>,
}

impl CommandCodeStore {
    pub fn load(path: &std::path::Path) -> Self {
        jcode_base::storage::read_json(path).unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        write_json_secret_locked(path, self)
    }
}

/// Insert or replace one account under an explicit or derived label, keeping
/// add/replace semantics explicit (never silently overwrite unless the caller
/// named an existing label, D-03).
pub fn upsert_account_locked(
    store: &mut CommandCodeStore,
    mut account: CommandCodeAccount,
    requested_label: Option<String>,
) -> String {
    let label = requested_label
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| {
            jcode_base::auth::account_store::next_account_label(PROVIDER_KEY, store.accounts.len())
        });
    account.label = Some(label.clone());
    if let Some(existing) = store
        .accounts
        .iter_mut()
        .find(|existing| existing.label.as_deref() == Some(label.as_str()))
    {
        *existing = account;
    } else {
        store.accounts.push(account);
    }
    if store.accounts.len() == 1 {
        store.active = Some(label.clone());
    }
    label
}

/// Gate: keys persist only after a successful whoami identity (D-02).
pub fn verify_identity(identity: &WhoamiIdentity) -> Result<()> {
    if whoami_identity_valid(identity) {
        Ok(())
    } else {
        anyhow::bail!(
            "Command Code /alpha/whoami identity incomplete; refusing to persist credentials"
        )
    }
}

/// Persist one whoami-verified account, returning the assigned label. The
/// verify-before-persist gate runs before any write happens.
pub fn persist_verified_account(
    store_path: &std::path::Path,
    candidate: CommandCodeAccount,
    identity: &WhoamiIdentity,
    requested_label: Option<String>,
) -> Result<String> {
    verify_identity(identity)?;
    let mut store = CommandCodeStore::load(store_path);
    let label = upsert_account_locked(&mut store, candidate, requested_label);
    store.save(store_path)?;
    Ok(label)
}

/// One-time import of the external snapshot file (~/.commandcode/auth.json).
/// D-01: attempted exactly once, marked via imported_at, never re-watched or
/// rescanned after the flag exists. Fails Closed when whoami rejects.
pub fn import_command_code_auth_snapshot(
    store_path: &std::path::Path,
    snapshot_path: &std::path::Path,
    whoami: impl Fn(&str) -> Result<WhoamiIdentity>,
) -> Result<Option<CommandCodeAccount>> {
    let store = CommandCodeStore::load(store_path);
    if store.imported_at.is_some() {
        return Ok(None);
    }
    let snapshot = jcode_base::storage::read_json::<serde_json::Value>(snapshot_path).ok();
    let candidate = snapshot.and_then(|value| {
        let api_key = value
            .get("apiKey")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if api_key.is_empty() {
            None
        } else {
            Some((api_key, value))
        }
    });
    let imported = match candidate {
        Some((api_key, value)) => Some((api_key.clone(), value, whoami(&api_key))),
        None => None,
    };
    let mut store = CommandCodeStore::load(store_path);
    store
        .imported_at
        .get_or_insert_with(|| Utc::now().to_rfc3339());
    store.save(store_path)?;
    match imported {
        Some((api_key, value, identity_result)) => {
            let identity = identity_result?;
            let account = CommandCodeAccount {
                label: None,
                api_key,
                user_id: identity.user.id.clone(),
                user_name: identity.user.user_name.clone(),
                org_id: identity.user.org_id.clone(),
                key_name: value
                    .get("keyName")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            };
            let label = persist_verified_account(store_path, account, &identity, None)?;
            let store = CommandCodeStore::load(store_path);
            Ok(Some(
                store
                    .accounts
                    .into_iter()
                    .find(|account| account.label.as_deref() == Some(label.as_str()))
                    .context("persisted account missing after save")?,
            ))
        }
        None => Ok(None),
    }
}

/// Startup adapter: perform the one-time import with the real whoami gate.
/// Runs in a short-lived thread so synchronous provider registration can call it
/// even when the daemon already owns a Tokio runtime.
pub fn import_snapshot_at_startup() {
    let Ok(store) = auth_store_path() else { return };
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let snapshot = std::path::PathBuf::from(home).join(".commandcode/auth.json");
    if !snapshot.exists() {
        return;
    }
    let _ = std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let client = reqwest::Client::new();
        let _ = import_command_code_auth_snapshot(&store, &snapshot, |key| {
            runtime.block_on(verify_whoami(&client, key))
        });
    })
    .join();
}
