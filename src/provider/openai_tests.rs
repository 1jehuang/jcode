use super::*;
use crate::auth::codex::CodexCredentials;
use crate::message::{ContentBlock, Role};
use anyhow::Result;
use futures::{SinkExt, StreamExt};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
const BRIGHT_PEARL_WRAPPED_TOOL_CALL_FIXTURE: &str =
    include_str!("../../tests/fixtures/openai/bright_pearl_wrapped_tool_call.txt");
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }

    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
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
        let lock = ENV_LOCK.lock().unwrap();
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

async fn live_openai_catalog() -> Result<Option<crate::provider::OpenAIModelCatalog>> {
    let Some(_env) = LiveOpenAITestEnv::new()? else {
        return Ok(None);
    };
    let creds = crate::auth::codex::load_credentials()?;
    if !OpenAIProvider::is_chatgpt_mode(&creds) {
        return Ok(None);
    }

    let token = openai_access_token(&Arc::new(RwLock::new(creds))).await?;
    Ok(Some(
        crate::provider::fetch_openai_model_catalog(&token).await?,
    ))
}

async fn live_openai_smoke(model: &str, sentinel: &str) -> Result<Option<String>> {
    let Some(_env) = LiveOpenAITestEnv::new()? else {
        return Ok(None);
    };
    let creds = crate::auth::codex::load_credentials()?;
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
