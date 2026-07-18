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

async fn test_persistent_ws_state() -> (PersistentWsState, tokio::task::JoinHandle<()>) {
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
            last_response_id: "resp_test".to_string(),
            connected_at: Instant::now(),
            last_activity_at: Instant::now(),
            message_count: 1,
            last_input_item_count: 1,
        },
        server,
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

    let token = openai_access_token(&Arc::new(RwLock::new(creds))).await?;
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
        provider.api_reasoning_effort(Some("swarm")).as_deref(),
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
        provider.api_reasoning_effort(Some("swarm")).as_deref(),
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
