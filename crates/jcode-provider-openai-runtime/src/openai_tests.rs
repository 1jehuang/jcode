#![allow(clippy::collapsible_match)]

use super::*;
use anyhow::Result;
use futures::{SinkExt, StreamExt};
use jcode_base::auth::codex::CodexCredentials;
use jcode_message_types::{ContentBlock, Role};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::MutexGuard;
use std::time::{Duration, Instant};
const BRIGHT_PEARL_WRAPPED_TOOL_CALL_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/openai/bright_pearl_wrapped_tool_call.txt"
));

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        jcode_base::env::set_var(key, value);
        Self { key, previous }
    }

    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        jcode_base::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        jcode_base::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            jcode_base::env::set_var(self.key, previous);
        } else {
            jcode_base::env::remove_var(self.key);
        }
    }
}

pub(super) async fn test_persistent_ws_state() -> (PersistentWsState, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test websocket listener");
    let addr = listener.local_addr().expect("listener local addr");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket client");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("accept websocket handshake");
        while let Some(message) = ws.next().await {
            match message {
                Ok(WsMessage::Ping(payload)) => {
                    let _ = ws.send(WsMessage::Pong(payload)).await;
                }
                Ok(WsMessage::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let (client_ws, _) = connect_async(format!("ws://{}", addr))
        .await
        .expect("connect websocket client");
    (
        PersistentWsState {
            ws_stream: client_ws,
            identity: openai_websocket_prewarm::prewarm_identity(&prewarm_test_credentials()),
            last_response_id: "resp_test".to_string(),
            connected_at: Instant::now(),
            last_activity_at: Instant::now(),
            last_response_completed_at: Instant::now(),
            message_count: 1,
            last_input_item_count: 1,
            last_input_item_hashes: persistent_ws_input_item_hashes(&[
                serde_json::json!({"role":"user","content":"previous"}),
            ]),
        },
        server,
    )
}

async fn test_persistent_ws_state_with_ping_notify() -> (
    PersistentWsState,
    tokio::task::JoinHandle<()>,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test websocket listener");
    let addr = listener.local_addr().expect("listener local addr");
    let ping_notify = Arc::new(tokio::sync::Notify::new());
    let server_ping_notify = Arc::clone(&ping_notify);
    let pong_notify = Arc::new(tokio::sync::Notify::new());
    let server_pong_notify = Arc::clone(&pong_notify);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket client");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("accept websocket handshake");
        while let Some(message) = ws.next().await {
            match message {
                Ok(WsMessage::Ping(payload)) => {
                    server_ping_notify.notify_one();
                    let _ = ws.send(WsMessage::Pong(b"stale-pong".to_vec())).await;
                    let _ = ws.send(WsMessage::Ping(b"server-keepalive".to_vec())).await;
                    let _ = ws.send(WsMessage::Pong(payload)).await;
                }
                Ok(WsMessage::Pong(payload)) if payload.as_slice() == b"server-keepalive" => {
                    server_pong_notify.notify_one();
                }
                Ok(WsMessage::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let (client_ws, _) = connect_async(format!("ws://{}", addr))
        .await
        .expect("connect websocket client");
    (
        PersistentWsState {
            ws_stream: client_ws,
            identity: openai_websocket_prewarm::prewarm_identity(&prewarm_test_credentials()),
            last_response_id: "resp_test".to_string(),
            connected_at: Instant::now(),
            last_activity_at: Instant::now(),
            last_response_completed_at: Instant::now(),
            message_count: 1,
            last_input_item_count: 1,
            last_input_item_hashes: persistent_ws_input_item_hashes(&[
                serde_json::json!({"role":"user","content":"previous"}),
            ]),
        },
        server,
        ping_notify,
        pong_notify,
    )
}

struct LiveOpenAITestEnv {
    _lock: MutexGuard<'static, ()>,
    _jcode_home: EnvVarGuard,
    _transport: EnvVarGuard,
    _temp: tempfile::TempDir,
}

impl LiveOpenAITestEnv {
    fn new() -> Result<Option<Self>> {
        let lock = jcode_base::storage::lock_test_env();
        let Some(source_auth) = real_codex_auth_path() else {
            return Ok(None);
        };

        let temp = tempfile::Builder::new()
            .prefix("jcode-openai-live-")
            .tempdir()?;
        let target_auth = temp
            .path()
            .join("external")
            .join(".codex")
            .join("auth.json");
        std::fs::create_dir_all(
            target_auth
                .parent()
                .expect("temp auth target should have a parent"),
        )?;
        std::fs::copy(source_auth, &target_auth)?;

        let jcode_home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let transport = EnvVarGuard::set("JCODE_OPENAI_TRANSPORT", "https");

        Ok(Some(Self {
            _lock: lock,
            _jcode_home: jcode_home,
            _transport: transport,
            _temp: temp,
        }))
    }
}

fn real_codex_auth_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home.join(".codex").join("auth.json");
    path.exists().then_some(path)
}

async fn live_openai_catalog() -> Result<Option<jcode_base::provider::OpenAIModelCatalog>> {
    let Some(_env) = LiveOpenAITestEnv::new()? else {
        return Ok(None);
    };
    let creds = jcode_base::auth::codex::load_credentials()?;
    if !OpenAIProvider::is_chatgpt_mode(&creds) {
        return Ok(None);
    }

    let token = openai_access_token(
        &Arc::new(RwLock::new(creds)),
        &jcode_provider_core::AccountPinSlot::default(),
    )
    .await?;
    Ok(Some(
        jcode_base::provider::fetch_openai_model_catalog(&token).await?,
    ))
}

async fn live_openai_smoke(model: &str, sentinel: &str) -> Result<Option<String>> {
    let Some(_env) = LiveOpenAITestEnv::new()? else {
        return Ok(None);
    };
    let creds = jcode_base::auth::codex::load_credentials()?;
    if !OpenAIProvider::is_chatgpt_mode(&creds) {
        return Ok(None);
    }

    let provider = OpenAIProvider::new(creds);
    provider.set_model(model)?;
    let response = provider
        .complete_simple(&format!("Reply with exactly {}.", sentinel), "")
        .await?;
    Ok(Some(response))
}

include!("openai_tests/models_state.rs");
include!("openai_tests/responses_input.rs");
include!("openai_tests/transport_runtime.rs");
include!("openai_tests/websocket_prewarm.rs");
include!("openai_tests/payloads.rs");
include!("openai_tests/parsing_tools.rs");

/// Mirror of the Anthropic round-trip guard: the runtime-provider identity that
/// `set_credential_mode` writes for OpenAI must decode back to the same mode so
/// the model picker / header widget report the auth method that requests will
/// actually use.
#[test]
fn openai_credential_mode_runtime_provider_identity_round_trips() {
    let _guard = jcode_base::storage::lock_test_env();
    let previous = std::env::var_os("JCODE_RUNTIME_PROVIDER");

    jcode_base::env::set_var("JCODE_RUNTIME_PROVIDER", "openai");
    assert_eq!(
        OpenAICredentialMode::from_runtime_env(jcode_provider_core::DualAuthProvider::OpenAI),
        OpenAICredentialMode::OAuth,
        "OAuth selection must surface as the OAuth runtime identity"
    );

    jcode_base::env::set_var("JCODE_RUNTIME_PROVIDER", "openai-api");
    assert_eq!(
        OpenAICredentialMode::from_runtime_env(jcode_provider_core::DualAuthProvider::OpenAI),
        OpenAICredentialMode::ApiKey,
        "API-key selection must surface as the API-key runtime identity"
    );

    match previous {
        Some(value) => jcode_base::env::set_var("JCODE_RUNTIME_PROVIDER", value),
        None => jcode_base::env::remove_var("JCODE_RUNTIME_PROVIDER"),
    }
}

#[tokio::test]
async fn openai_available_efforts_follow_active_model_catalog_metadata() {
    let provider = OpenAIProvider::new_browser_only();
    *provider.model.write().await = "gpt-5.6".to_string();
    provider
        .model_reasoning_efforts
        .write()
        .expect("reasoning effort catalog lock")
        .insert(
            "gpt-5.6".to_string(),
            vec![
                "minimal".to_string(),
                "medium".to_string(),
                "max".to_string(),
            ],
        );

    assert_eq!(
        provider.available_efforts(),
        vec!["minimal", "medium", "max", "swarm", "swarm-deep"]
    );
    *provider.model.write().await = "gpt-5.6[1m]".to_string();
    assert_eq!(
        provider.available_efforts(),
        vec!["minimal", "medium", "max", "swarm", "swarm-deep"],
        "long-context aliases must use the canonical model's catalog metadata"
    );
    *provider.model.write().await = "gpt-5.6".to_string();
    assert_eq!(
        provider
            .api_reasoning_effort_with_swarm_root(Some("swarm"), Some("max"))
            .as_deref(),
        Some("max")
    );

    provider
        .model_reasoning_efforts
        .write()
        .expect("reasoning effort catalog lock")
        .insert(
            "gpt-5.6".to_string(),
            vec!["low".to_string(), "high".to_string(), "xhigh".to_string()],
        );
    assert_eq!(
        provider
            .api_reasoning_effort_with_swarm_root(Some("swarm"), Some("max"))
            .as_deref(),
        Some("xhigh"),
        "swarm must clamp to the active model's strongest advertised effort"
    );
    assert!(
        provider.set_reasoning_effort("max").is_err(),
        "explicit effort choices must respect active-model catalog capabilities"
    );
    provider
        .set_reasoning_effort("xhigh")
        .expect("advertised effort should be accepted");
    provider
        .model_reasoning_efforts
        .write()
        .expect("reasoning effort catalog lock")
        .insert(
            "gpt-5.5".to_string(),
            vec!["low".to_string(), "high".to_string()],
        );
    *provider.model.write().await = "gpt-5.5".to_string();
    provider.revalidate_reasoning_effort();
    assert_eq!(
        provider.reasoning_effort(),
        None,
        "an effort unsupported by the newly selected model must not remain active"
    );
    assert!(provider.set_reasoning_effort("typo").is_err());
}

#[test]
fn catalog_credential_identity_survives_token_refresh_but_changes_accounts() {
    let credentials = |access: &str, refresh: &str, account: Option<&str>| CodexCredentials {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        id_token: None,
        account_id: account.map(str::to_string),
        expires_at: None,
    };
    assert_eq!(
        OpenAIProvider::catalog_credential_identity(&credentials("old", "refresh", Some("acct"))),
        OpenAIProvider::catalog_credential_identity(&credentials("new", "refresh", Some("acct")))
    );
    assert_ne!(
        OpenAIProvider::catalog_credential_identity(&credentials("old", "refresh-a", None)),
        OpenAIProvider::catalog_credential_identity(&credentials("new", "refresh-b", None))
    );
}

include!("openai_tests/persistent_terminal.rs");

include!("openai_tests/persistent_prefix.rs");

fn stored_openai_account(
    label: &str,
    access: &str,
    refresh: &str,
) -> jcode_base::auth::codex::OpenAiAccount {
    jcode_base::auth::codex::OpenAiAccount {
        label: label.to_string(),
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        id_token: None,
        account_id: Some(format!("{access}-acct")),
        expires_at: Some(chrono::Utc::now().timestamp_millis() + 8 * 60 * 60 * 1000),
        email: None,
    }
}

fn openai_session_from_store() -> OpenAIProvider {
    OpenAIProvider::new(jcode_base::auth::codex::load_credentials().expect("stored OpenAI login"))
}

/// A relogin that stores a different ChatGPT account under the same label
/// must reach every live session, not just one that got a reload.
#[tokio::test]
async fn openai_same_label_relogin_replaces_token_in_every_live_session() {
    let _lock = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "openai");
    let _api_key = EnvVarGuard::remove("OPENAI_API_KEY");
    jcode_base::auth::codex::set_active_account_override(None);

    let label = jcode_base::auth::codex::upsert_account(stored_openai_account(
        "openai-1",
        "old-openai-access",
        "old-openai-refresh",
    ))
    .unwrap();
    let session = openai_session_from_store();
    let other_session = openai_session_from_store();
    assert_eq!(
        session.resolve_access_token().await.unwrap(),
        "old-openai-access"
    );

    jcode_base::auth::codex::upsert_account(stored_openai_account(
        &label,
        "new-openai-access",
        "new-openai-refresh",
    ))
    .unwrap();
    jcode_base::auth::AuthStatus::invalidate_cache();

    assert_eq!(
        session.resolve_access_token().await.unwrap(),
        "new-openai-access"
    );
    assert_eq!(
        other_session.resolve_access_token().await.unwrap(),
        "new-openai-access"
    );
}

/// `/account switch` only invalidates the requesting session. Every other
/// live session must use the newly active account on its next request.
#[tokio::test]
async fn openai_account_switch_replaces_token_in_other_live_sessions() {
    let _lock = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "openai");
    let _api_key = EnvVarGuard::remove("OPENAI_API_KEY");
    jcode_base::auth::codex::set_active_account_override(None);

    let first = jcode_base::auth::codex::upsert_account(stored_openai_account(
        "openai-1",
        "first-openai-access",
        "first-openai-refresh",
    ))
    .unwrap();
    let second = jcode_base::auth::codex::upsert_account(stored_openai_account(
        "openai-2",
        "second-openai-access",
        "second-openai-refresh",
    ))
    .unwrap();
    jcode_base::auth::codex::set_active_account(&first).unwrap();
    let other_session = openai_session_from_store();
    assert_eq!(
        other_session.resolve_access_token().await.unwrap(),
        "first-openai-access"
    );

    jcode_base::auth::codex::set_active_account(&second).unwrap();

    assert_eq!(
        other_session.resolve_access_token().await.unwrap(),
        "second-openai-access"
    );
    jcode_base::auth::codex::set_active_account_override(None);
}

/// An external Codex CLI relogin rewrites ~/.codex/auth.json without telling
/// jcode. The next request must still use the new login.
#[tokio::test]
async fn openai_external_codex_relogin_replaces_token() {
    let _lock = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "openai");
    let _api_key = EnvVarGuard::remove("OPENAI_API_KEY");
    let _legacy = EnvVarGuard::set("JCODE_ALLOW_CODEX_LEGACY_AUTH", "1");
    jcode_base::auth::codex::set_active_account_override(None);

    let path = temp.path().join("external/.codex/auth.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let write_login = |access: &str| {
        std::fs::write(
            &path,
            serde_json::json!({
                "tokens": {
                    "access_token": access,
                    "refresh_token": format!("{access}-refresh"),
                    "account_id": format!("{access}-acct"),
                    "expires_at": chrono::Utc::now().timestamp_millis() + 8 * 60 * 60 * 1000
                }
            })
            .to_string(),
        )
        .unwrap();
    };
    write_login("codex-old-login");
    let session = openai_session_from_store();
    assert_eq!(
        session.resolve_access_token().await.unwrap(),
        "codex-old-login"
    );

    write_login("codex-new-login-other-account");

    assert_eq!(
        session.resolve_access_token().await.unwrap(),
        "codex-new-login-other-account"
    );
}

/// A token refresh that started for one account and finishes after the session
/// moved to another account must not overwrite the new account's bearer while
/// keeping the new account id.
#[tokio::test]
async fn openai_refresh_finishing_after_account_switch_keeps_new_credentials() {
    let credentials = Arc::new(RwLock::new(CodexCredentials {
        access_token: "new-account-access".to_string(),
        refresh_token: "new-account-refresh".to_string(),
        id_token: None,
        account_id: Some("new-account-id".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp_millis() + 8 * 60 * 60 * 1000),
    }));
    // The refresh began from the old account's refresh token, before the switch.
    let refreshed = jcode_base::auth::oauth::OAuthTokens {
        access_token: "old-account-refreshed-access".to_string(),
        refresh_token: "old-account-rotated-refresh".to_string(),
        expires_at: chrono::Utc::now().timestamp_millis() + 3_600_000,
        id_token: None,
        scopes: Vec::new(),
    };

    let bearer = super::openai_stream_runtime::commit_openai_refresh(
        &credentials,
        "old-account-refresh",
        refreshed,
    )
    .await;

    let cached = credentials.read().await;
    assert_eq!(bearer, "new-account-access");
    assert_eq!(cached.access_token, "new-account-access");
    assert_eq!(cached.refresh_token, "new-account-refresh");
    assert_eq!(cached.account_id.as_deref(), Some("new-account-id"));
}

/// Without an account change the refreshed bearer replaces the cached one.
#[tokio::test]
async fn openai_refresh_without_account_switch_updates_cache() {
    let credentials = Arc::new(RwLock::new(CodexCredentials {
        access_token: "same-account-access".to_string(),
        refresh_token: "same-account-refresh".to_string(),
        id_token: None,
        account_id: Some("same-account-id".to_string()),
        expires_at: Some(0),
    }));
    let refreshed = jcode_base::auth::oauth::OAuthTokens {
        access_token: "same-account-refreshed-access".to_string(),
        refresh_token: "same-account-rotated-refresh".to_string(),
        expires_at: chrono::Utc::now().timestamp_millis() + 3_600_000,
        id_token: None,
        scopes: Vec::new(),
    };

    let bearer = super::openai_stream_runtime::commit_openai_refresh(
        &credentials,
        "same-account-refresh",
        refreshed,
    )
    .await;

    let cached = credentials.read().await;
    assert_eq!(bearer, "same-account-refreshed-access");
    assert_eq!(cached.access_token, "same-account-refreshed-access");
    assert_eq!(cached.refresh_token, "same-account-rotated-refresh");
    assert_eq!(cached.account_id.as_deref(), Some("same-account-id"));
}

fn seed_two_pinnable_openai_accounts(expires_at: i64) -> (String, String) {
    let account = |label: &str, access: &str, refresh: &str, email: &str| {
        let mut acc = stored_openai_account(label, access, refresh);
        acc.email = Some(email.to_string());
        acc.expires_at = Some(expires_at);
        acc
    };
    let otter = jcode_base::auth::codex::upsert_account(account(
        "openai-1",
        "otter-access",
        "otter-refresh",
        "otter@example.com",
    ))
    .unwrap();
    let fox = jcode_base::auth::codex::upsert_account(account(
        "openai-2",
        "fox-access",
        "fox-refresh",
        "fox@example.com",
    ))
    .unwrap();
    jcode_base::auth::codex::set_active_account(&otter).unwrap();
    jcode_base::auth::codex::set_active_account_override(None);
    (otter, fox)
}

/// Forks are separate sessions. Pinning one fork to another account must not
/// change the bearer the original session sends.
#[tokio::test]
async fn openai_fork_does_not_share_credential_cache() {
    let _lock = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "openai");
    let _api_key = EnvVarGuard::remove("OPENAI_API_KEY");
    jcode_base::auth::codex::set_active_account_override(None);
    let far = chrono::Utc::now().timestamp_millis() + 8 * 60 * 60 * 1000;
    let (_otter, fox) = seed_two_pinnable_openai_accounts(far);

    let session = openai_session_from_store();
    assert_eq!(
        session.resolve_access_token().await.unwrap(),
        "otter-access"
    );
    let forked = session.fork_concrete();

    // A refresh or account change in the fork rewrites the fork's cache only.
    *forked.credentials.write().await =
        jcode_base::auth::codex::load_credentials_for_account(&fox).unwrap();
    assert_eq!(
        session.credentials.read().await.access_token,
        "otter-access",
        "a fork must own its credential cache"
    );

    // Pinning the fork sends fox from the fork and otter from the session.
    forked
        .set_account_pin(
            AccountProviderKind::OpenAi,
            Some(jcode_base::auth::codex::pin_for_label(&fox).unwrap()),
        )
        .unwrap();
    assert_eq!(forked.resolve_access_token().await.unwrap(), "fox-access");
    assert_eq!(
        forked.resolved_account_label(AccountProviderKind::OpenAi),
        Some(fox.clone())
    );
    assert_eq!(session.account_pin(AccountProviderKind::OpenAi), None);
    assert_eq!(
        session.resolve_access_token().await.unwrap(),
        "otter-access"
    );
    assert_eq!(
        session
            .resolved_account_label(AccountProviderKind::OpenAi)
            .as_deref(),
        Some("openai-otter")
    );
}

/// A pinned session whose token needs a refresh writes the refreshed tokens to
/// the pinned account, never to the default account.
#[tokio::test]
async fn openai_refresh_writes_back_to_pinned_label_not_default() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _lock = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "openai");
    let _api_key = EnvVarGuard::remove("OPENAI_API_KEY");
    jcode_base::auth::codex::set_active_account_override(None);
    // Both accounts are about to expire, so the next token read refreshes.
    let soon = chrono::Utc::now().timestamp_millis() + 60_000;
    let (otter, fox) = seed_two_pinnable_openai_accounts(soon);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/oauth/token", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16 * 1024];
        let mut read = 0;
        loop {
            let n = socket.read(&mut buf[read..]).await.unwrap();
            read += n;
            let text = String::from_utf8_lossy(&buf[..read]).to_string();
            if let Some(pos) = text.find("\r\n\r\n") {
                let len = text[..pos]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().to_string())
                    })
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                if read >= pos + 4 + len || n == 0 {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let body =
            r#"{"access_token":"fox-refreshed","refresh_token":"fox-refresh-2","expires_in":3600}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(resp.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&buf[..read]).to_string()
    });
    jcode_base::auth::oauth::set_token_url_override_for_tests(
        jcode_base::auth::oauth::TokenUrlProvider::OpenAi,
        Some(url),
    );

    let session = openai_session_from_store();
    session
        .set_account_pin(
            AccountProviderKind::OpenAi,
            Some(jcode_base::auth::codex::pin_for_label(&fox).unwrap()),
        )
        .unwrap();
    let token = session.resolve_access_token().await;
    jcode_base::auth::oauth::set_token_url_override_for_tests(
        jcode_base::auth::oauth::TokenUrlProvider::OpenAi,
        None,
    );
    assert_eq!(token.unwrap(), "fox-refreshed");
    let request = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("refresh must reach the token endpoint")
        .unwrap();
    assert!(request.contains("fox-refresh"), "{request}");

    let accounts = jcode_base::auth::codex::list_accounts().unwrap();
    let find = |label: &str| accounts.iter().find(|a| a.label == label).unwrap().clone();
    assert_eq!(find(&otter).access_token, "otter-access");
    assert_eq!(find(&fox).access_token, "fox-refreshed");
    assert_eq!(find(&fox).email.as_deref(), Some("fox@example.com"));
}
