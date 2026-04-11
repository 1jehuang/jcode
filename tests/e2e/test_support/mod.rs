//! End-to-end tests for jcode using a mock provider
//!
//! These tests verify the full flow from user input to response
//! without making actual API calls.

pub(crate) use crate::mock_provider::MockProvider;
pub(crate) use anyhow::{Context, Result};
pub(crate) use async_trait::async_trait;
pub(crate) use futures::{SinkExt, StreamExt, stream};
pub(crate) use jcode::agent::Agent;
pub(crate) use jcode::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
pub(crate) use jcode::protocol::{Request, ServerEvent};
pub(crate) use jcode::provider::{EventStream, Provider};
pub(crate) use jcode::server;
pub(crate) use jcode::session::{Session, StoredCompactionState};
pub(crate) use jcode::tool::Registry;
pub(crate) use std::ffi::OsString;
pub(crate) use std::io::Read;
pub(crate) use std::net::TcpListener as StdTcpListener;
#[cfg(unix)]
use std::os::fd::FromRawFd;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
pub(crate) use std::process::{Child, Command, Stdio};
pub(crate) use std::sync::Arc;
pub(crate) use std::sync::Mutex;
pub(crate) use std::time::{Duration, Instant};
pub(crate) use tokio::net::TcpStream;
pub(crate) use tokio::time::timeout;
pub(crate) use tokio_tungstenite::connect_async;
pub(crate) use tokio_tungstenite::tungstenite::Message as WsMessage;
pub(crate) use tokio_tungstenite::tungstenite::client::IntoClientRequest;

static JCODE_HOME_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

fn lock_jcode_home() -> std::sync::MutexGuard<'static, ()> {
    let mutex = JCODE_HOME_LOCK.get_or_init(|| Mutex::new(()));
    // Recover from poisoned state if a previous test panicked
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) struct TestEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_home: Option<OsString>,
    prev_runtime_dir: Option<OsString>,
    prev_test_session: Option<OsString>,
    prev_debug_control: Option<OsString>,
    _temp_home: tempfile::TempDir,
}

impl TestEnvGuard {
    pub(crate) fn new() -> Result<Self> {
        let lock = lock_jcode_home();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-e2e-home-")
            .tempdir()?;
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_runtime_dir = std::env::var_os("JCODE_RUNTIME_DIR");
        let prev_test_session = std::env::var_os("JCODE_TEST_SESSION");
        let prev_debug_control = std::env::var_os("JCODE_DEBUG_CONTROL");
        let runtime_dir = temp_home.path().join("runtime");
        std::fs::create_dir_all(&runtime_dir)?;

        jcode::env::set_var("JCODE_HOME", temp_home.path());
        jcode::env::set_var("JCODE_RUNTIME_DIR", &runtime_dir);
        jcode::env::set_var("JCODE_TEST_SESSION", "1");
        jcode::env::set_var("JCODE_DEBUG_CONTROL", "1");

        Ok(Self {
            _lock: lock,
            prev_home,
            prev_runtime_dir,
            prev_test_session,
            prev_debug_control,
            _temp_home: temp_home,
        })
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        if let Some(prev_home) = &self.prev_home {
            jcode::env::set_var("JCODE_HOME", prev_home);
        } else {
            jcode::env::remove_var("JCODE_HOME");
        }

        if let Some(prev_runtime_dir) = &self.prev_runtime_dir {
            jcode::env::set_var("JCODE_RUNTIME_DIR", prev_runtime_dir);
        } else {
            jcode::env::remove_var("JCODE_RUNTIME_DIR");
        }

        if let Some(prev_test_session) = &self.prev_test_session {
            jcode::env::set_var("JCODE_TEST_SESSION", prev_test_session);
        } else {
            jcode::env::remove_var("JCODE_TEST_SESSION");
        }

        if let Some(prev_debug_control) = &self.prev_debug_control {
            jcode::env::set_var("JCODE_DEBUG_CONTROL", prev_debug_control);
        } else {
            jcode::env::remove_var("JCODE_DEBUG_CONTROL");
        }
    }
}

pub(crate) fn setup_test_env() -> Result<TestEnvGuard> {
    TestEnvGuard::new()
}

pub(crate) struct EnvVarGuard {
    name: &'static str,
    prev: Option<OsString>,
}

impl EnvVarGuard {
    pub(crate) fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(name);
        jcode::env::set_var(name, value);
        Self { name, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            jcode::env::set_var(self.name, prev);
        } else {
            jcode::env::remove_var(self.name);
        }
    }
}

pub(crate) fn reserve_tcp_port() -> Result<u16> {
    let listener = StdTcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

pub(crate) async fn wait_for_socket(path: &std::path::Path) -> Result<()> {
    let start = Instant::now();
    while !path.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}

pub(crate) async fn wait_for_debug_socket_ready(path: &std::path::Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last_error: Option<anyhow::Error> = None;
    loop {
        if Instant::now() >= deadline {
            if let Some(err) = last_error {
                return Err(err).context("debug socket never became responsive");
            }
            anyhow::bail!("debug socket never became responsive");
        }

        if !path.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
            continue;
        }

        match debug_run_command(path.to_path_buf(), "server:info", None).await {
            Ok(_) => return Ok(()),
            Err(err) => {
                last_error = Some(err);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

pub(crate) async fn wait_for_tcp_port(port: u16) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    anyhow::bail!("Gateway TCP port {} did not open", port)
}

fn pair_test_device(token: &str) -> Result<()> {
    let mut registry = jcode::gateway::DeviceRegistry::load();
    let now = chrono::Utc::now().to_rfc3339();
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(token.as_bytes());
    let token_hash = format!("sha256:{}", hex::encode(hasher.finalize()));
    registry.devices.retain(|d| d.id != "test-device-ws");
    registry.devices.push(jcode::gateway::PairedDevice {
        id: "test-device-ws".to_string(),
        name: "WS Test Device".to_string(),
        token_hash,
        apns_token: None,
        paired_at: now.clone(),
        last_seen: now,
    });
    registry.save()
}

struct WsTestClient {
    stream: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

#[derive(Clone, Default)]
pub(crate) struct CapturingCompactionProvider {
    captured_messages: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl CapturingCompactionProvider {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn captured_messages(&self) -> Arc<Mutex<Vec<Vec<Message>>>> {
        Arc::clone(&self.captured_messages)
    }
}

#[async_trait]
impl Provider for CapturingCompactionProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.captured_messages
            .lock()
            .unwrap()
            .push(messages.to_vec());

        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta("compaction-ok".to_string())),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".to_string()),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "capturing-compaction"
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        1_000
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

pub(crate) fn flatten_text_blocks(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl WsTestClient {
    async fn connect(port: u16, token: &str) -> Result<Self> {
        let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse()?);
        let (stream, _) = connect_async(request).await?;
        Ok(Self { stream, next_id: 1 })
    }

    async fn send_request(&mut self, request: Request) -> Result<u64> {
        let id = request.id();
        let json = serde_json::to_string(&request)?;
        self.stream.send(WsMessage::Text(json.into())).await?;
        Ok(id)
    }

    async fn subscribe(&mut self) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::Subscribe {
            id,
            working_dir: None,
            selfdev: None,
            target_session_id: None,
            client_instance_id: None,
            client_has_local_history: false,
            allow_session_takeover: false,
        })
        .await
    }

    async fn get_history(&mut self) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::GetHistory { id }).await
    }

    async fn send_message(&mut self, content: &str) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::Message {
            id,
            content: content.to_string(),
            images: vec![],
            system_reminder: None,
        })
        .await
    }

    async fn resume_session(&mut self, session_id: &str) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::ResumeSession {
            id,
            session_id: session_id.to_string(),
            client_instance_id: None,
            client_has_local_history: false,
            allow_session_takeover: false,
        })
        .await
    }

    async fn read_event(&mut self) -> Result<ServerEvent> {
        loop {
            let msg = timeout(Duration::from_secs(5), self.stream.next())
                .await?
                .ok_or_else(|| anyhow::anyhow!("websocket disconnected"))??;
            match msg {
                WsMessage::Text(text) => return Ok(serde_json::from_str(&text)?),
                WsMessage::Ping(data) => {
                    self.stream.send(WsMessage::Pong(data)).await?;
                }
                WsMessage::Pong(_) => continue,
                WsMessage::Close(_) => anyhow::bail!("websocket closed"),
                other => anyhow::bail!("unexpected websocket message: {other:?}"),
            }
        }
    }
}

pub(crate) async fn collect_until_done_unix(
    client: &mut server::Client,
    target_id: u64,
) -> Result<Vec<ServerEvent>> {
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let event = timeout(Duration::from_secs(1), client.read_event()).await??;
        let is_done = matches!(event, ServerEvent::Done { id } if id == target_id);
        events.push(event);
        if is_done {
            return Ok(events);
        }
    }
    let seen = events
        .iter()
        .map(|event| format!("{event:?}"))
        .collect::<Vec<_>>()
        .join(" | ");
    anyhow::bail!(
        "timed out waiting for done event {target_id} over unix socket; seen events: {seen}"
    )
}

pub(crate) async fn collect_until_history_unix(
    client: &mut server::Client,
    target_id: u64,
) -> Result<Vec<ServerEvent>> {
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let event = timeout(Duration::from_secs(1), client.read_event()).await??;
        let is_target_history = matches!(event, ServerEvent::History { id, .. } if id == target_id);
        events.push(event);
        if is_target_history {
            return Ok(events);
        }
    }
    anyhow::bail!("timed out waiting for history event {target_id} over unix socket")
}

async fn collect_until_done_ws(
    client: &mut WsTestClient,
    target_id: u64,
) -> Result<Vec<ServerEvent>> {
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let event = client.read_event().await?;
        let is_done = matches!(event, ServerEvent::Done { id } if id == target_id);
        events.push(event);
        if is_done {
            return Ok(events);
        }
    }
    anyhow::bail!("timed out waiting for done event {target_id} over websocket")
}

async fn collect_until_history_ws(
    client: &mut WsTestClient,
    target_id: u64,
) -> Result<Vec<ServerEvent>> {
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let event = client.read_event().await?;
        let is_target_history = matches!(event, ServerEvent::History { id, .. } if id == target_id);
        events.push(event);
        if is_target_history {
            return Ok(events);
        }
    }
    anyhow::bail!("timed out waiting for history event {target_id} over websocket")
}

pub(crate) fn summarize_history_invariant(event: &ServerEvent) -> Option<String> {
    match event {
        ServerEvent::History {
            id,
            messages,
            provider_name,
            provider_model,
            available_models,
            available_model_routes,
            mcp_servers,
            skills,
            client_count,
            is_canary,
            upstream_provider,
            reasoning_effort,
            ..
        } => Some(format!(
            "history:{id}:messages={}:provider={}:model={}:available_models={:?}:routes={:?}:mcp={:?}:skills={:?}:client_count={:?}:is_canary={:?}:upstream={:?}:reasoning={:?}",
            messages.len(),
            provider_name.as_deref().unwrap_or(""),
            provider_model.as_deref().unwrap_or(""),
            available_models,
            available_model_routes,
            mcp_servers,
            skills,
            client_count,
            is_canary,
            upstream_provider,
            reasoning_effort,
        )),
        _ => None,
    }
}

pub(crate) fn summarize_message_invariant(events: &[ServerEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ServerEvent::Ack { id } => Some(format!("ack:{id}")),
            ServerEvent::ConnectionType { connection } => {
                Some(format!("connection_type:{connection}"))
            }
            ServerEvent::TextDelta { text } => Some(format!("text:{text}")),
            ServerEvent::SessionId { session_id } => Some(format!("session_id:{session_id}")),
            ServerEvent::Done { id } => Some(format!("done:{id}")),
            ServerEvent::Error { id, message, .. } => Some(format!("error:{id}:{message}")),
            _ => None,
        })
        .collect()
}

pub(crate) struct TransportScenarioResult {
    pub(crate) subscribe_events: Vec<ServerEvent>,
    pub(crate) history_events: Vec<ServerEvent>,
    pub(crate) message_events: Vec<ServerEvent>,
    pub(crate) resume_events: Vec<ServerEvent>,
}

pub(crate) async fn run_unix_transport_scenario() -> Result<TransportScenarioResult> {
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-ws-e2e-unix-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    provider.queue_response(vec![
        StreamEvent::ConnectionType {
            connection: "mock-stream".to_string(),
        },
        StreamEvent::TextDelta("Hello from mock".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("provider-session-1".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let result = async {
        wait_for_socket(&socket_path).await?;
        let mut client = server::Client::connect_with_path(socket_path.clone()).await?;

        let subscribe_id = client.subscribe().await?;
        let subscribe_events = collect_until_done_unix(&mut client, subscribe_id).await?;

        let history_event = client.get_history_event().await?;
        let server_session_id = match &history_event {
            ServerEvent::History { session_id, .. } => session_id.clone(),
            other => anyhow::bail!("expected unix history event, got {other:?}"),
        };
        let history_events = vec![history_event];

        let message_id = client.send_message("hello over transport").await?;
        let mut message_events = Vec::new();
        let mut saw_message_done = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let event = timeout(Duration::from_secs(1), client.read_event()).await??;
            let is_done = matches!(event, ServerEvent::Done { id } if id == message_id);
            message_events.push(event);
            if is_done {
                saw_message_done = true;
                break;
            }
        }
        if !saw_message_done {
            let history = debug_run_command(debug_socket_path.clone(), "history", None)
                .await
                .unwrap_or_else(|e| format!("<history error: {e}>"));
            let last_response = debug_run_command(debug_socket_path.clone(), "last_response", None)
                .await
                .unwrap_or_else(|e| format!("<last_response error: {e}>"));
            let response_persisted = history.contains("Hello from mock")
                || (last_response != "last_response: none" && !last_response.trim().is_empty());
            if !response_persisted {
                let state = debug_run_command(debug_socket_path.clone(), "state", None)
                    .await
                    .unwrap_or_else(|e| format!("<state error: {e}>"));
                let logs = std::env::var_os("JCODE_HOME")
                    .and_then(|home| latest_log_excerpt(std::path::Path::new(&home)));
                let seen = message_events
                    .iter()
                    .map(|event| format!("{event:?}"))
                    .collect::<Vec<_>>()
                    .join(" | ");
                anyhow::bail!(
                    "unix message phase failed: timed out waiting for done event {message_id} over unix socket; seen events: {seen}\nstate={state}\nhistory={history}\nlast_response={last_response}\nlogs={}"
                    , logs.unwrap_or_else(|| "<no logs>".to_string())
                );
            }
        }

        let resume_id = client.resume_session(&server_session_id).await?;
        let resume_events = collect_until_history_unix(&mut client, resume_id).await?;

        Ok::<_, anyhow::Error>(TransportScenarioResult {
            subscribe_events,
            history_events,
            message_events,
            resume_events,
        })
    }
    .await;

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);
    result
}

pub(crate) async fn run_websocket_transport_scenario() -> Result<TransportScenarioResult> {
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-ws-e2e-websocket-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");
    let gateway_port = reserve_tcp_port()?;
    let ws_token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    pair_test_device(ws_token)?;

    let provider = MockProvider::new();
    provider.queue_response(vec![
        StreamEvent::ConnectionType {
            connection: "mock-stream".to_string(),
        },
        StreamEvent::TextDelta("Hello from mock".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("provider-session-1".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone())
            .with_gateway_config(jcode::gateway::GatewayConfig {
                port: gateway_port,
                bind_addr: "127.0.0.1".to_string(),
                enabled: true,
            });
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_tcp_port(gateway_port).await?;
        let mut client = WsTestClient::connect(gateway_port, ws_token).await?;

        let subscribe_id = client.subscribe().await?;
        let subscribe_events = collect_until_done_ws(&mut client, subscribe_id).await?;

        let history_request_id = client.get_history().await?;
        let history_events = collect_until_history_ws(&mut client, history_request_id).await?;
        let server_session_id = history_events
            .iter()
            .find_map(|event| match event {
                ServerEvent::History { session_id, .. } => Some(session_id.clone()),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("missing websocket history session id"))?;

        let message_id = client.send_message("hello over transport").await?;
        let message_events = collect_until_done_ws(&mut client, message_id).await?;

        let resume_id = client.resume_session(&server_session_id).await?;
        let resume_events = collect_until_history_ws(&mut client, resume_id).await?;

        Ok::<_, anyhow::Error>(TransportScenarioResult {
            subscribe_events,
            history_events,
            message_events,
            resume_events,
        })
    }
    .await;

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);
    result
}

pub(crate) async fn debug_create_headless_session_with_command(
    debug_socket_path: std::path::PathBuf,
    command: &str,
) -> Result<String> {
    let mut debug_client = server::Client::connect_debug_with_path(debug_socket_path).await?;
    let request_id = debug_client.debug_command(command, None).await?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let event =
            tokio::time::timeout(Duration::from_secs(1), debug_client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::DebugResponse { id, ok, output } if id == request_id => {
                if !ok {
                    anyhow::bail!("create_session debug command failed: {}", output);
                }
                let value: serde_json::Value = serde_json::from_str(&output)?;
                let session_id = value
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("missing session_id in debug response"))?;
                return Ok(session_id.to_string());
            }
            _ => {}
        }
    }

    anyhow::bail!("Timed out waiting for create_session debug response")
}

pub(crate) async fn debug_create_headless_session(
    debug_socket_path: std::path::PathBuf,
) -> Result<String> {
    debug_create_headless_session_with_command(debug_socket_path, "create_session").await
}

pub(crate) async fn debug_run_command(
    debug_socket_path: std::path::PathBuf,
    command: &str,
    session_id: Option<&str>,
) -> Result<String> {
    let mut debug_client = server::Client::connect_debug_with_path(debug_socket_path).await?;
    let request_id = debug_client.debug_command(command, session_id).await?;

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen_events = Vec::new();
    while Instant::now() < deadline {
        let event =
            match tokio::time::timeout(Duration::from_secs(1), debug_client.read_event()).await {
                Ok(Ok(event)) => event,
                Ok(Err(err)) => return Err(err),
                Err(_) => continue,
            };
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::DebugResponse { id, ok, output } if id == request_id => {
                if !ok {
                    anyhow::bail!("debug command failed: {}", output);
                }
                return Ok(output);
            }
            ServerEvent::Error { id, message, .. } if id == request_id => {
                anyhow::bail!("debug command error: {}", message);
            }
            other => {
                seen_events.push(format!("{other:?}"));
            }
        }
    }

    anyhow::bail!(
        "Timed out waiting for debug command response: {command}. Seen events: {}",
        if seen_events.is_empty() {
            "<none>".to_string()
        } else {
            seen_events.join(" | ")
        }
    )
}

pub(crate) async fn wait_for_server_client(
    socket_path: &std::path::Path,
) -> Result<server::Client> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match server::Client::connect_with_path(socket_path.to_path_buf()).await {
            Ok(mut client) => {
                let ping_deadline = Instant::now() + Duration::from_secs(5);
                while Instant::now() < ping_deadline {
                    match client.ping().await {
                        Ok(true) => return Ok(client),
                        Ok(false) => continue,
                        Err(_) => break,
                    }
                }
                if Instant::now() >= deadline {
                    anyhow::bail!(
                        "server socket connected at {} but never became responsive",
                        socket_path.display()
                    );
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(err) if Instant::now() < deadline => {
                let _ = err;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

pub(crate) fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
pub(crate) struct PtyChild {
    pub(crate) child: Child,
    input: std::fs::File,
    output: Arc<Mutex<Vec<u8>>>,
}

#[cfg(unix)]
impl PtyChild {
    pub(crate) fn send_input(&mut self, input: &str) -> Result<()> {
        use std::io::Write;

        self.input.write_all(input.as_bytes())?;
        self.input.flush()?;
        Ok(())
    }

    pub(crate) fn send_command(&mut self, command: &str) -> Result<()> {
        self.send_input(command)?;
        self.send_input("\r")
    }

    pub(crate) fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }
}

#[cfg(unix)]
pub(crate) fn spawn_pty_child(mut cmd: Command) -> Result<PtyChild> {
    let mut master_fd = -1;
    let mut slave_fd = -1;
    let mut winsize = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut winsize,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let master = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };
    let writer = master.try_clone()?;

    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.stdin(Stdio::from(slave.try_clone()?));
    cmd.stdout(Stdio::from(slave.try_clone()?));
    cmd.stderr(Stdio::from(slave));

    let child = cmd.spawn()?;
    let output = Arc::new(Mutex::new(Vec::new()));
    let output_clone = Arc::clone(&output);
    std::thread::spawn(move || {
        let mut master = master;
        let mut buf = [0u8; 4096];
        loop {
            match master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => output_clone.lock().unwrap().extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
    });

    Ok(PtyChild {
        child,
        input: writer,
        output,
    })
}

#[cfg(unix)]
pub(crate) fn set_file_mtime(path: &std::path::Path, when: std::time::SystemTime) -> Result<()> {
    let duration = when
        .duration_since(std::time::UNIX_EPOCH)
        .context("mtime must be after unix epoch")?;
    let times = [
        libc::timespec {
            tv_sec: duration.as_secs() as libc::time_t,
            tv_nsec: duration.subsec_nanos() as libc::c_long,
        },
        libc::timespec {
            tv_sec: duration.as_secs() as libc::time_t,
            tv_nsec: duration.subsec_nanos() as libc::c_long,
        },
    ];
    let path_cstr = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, path_cstr.as_ptr(), times.as_ptr(), 0) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) async fn wait_for_connected_client_session(
    debug_socket_path: &std::path::Path,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last_observation = "clients:map never returned a connected client".to_string();

    while Instant::now() < deadline {
        match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "clients:map", None),
        )
        .await
        {
            Ok(Ok(output)) => {
                let value: serde_json::Value = serde_json::from_str(&output)?;
                if let Some(session_id) = value
                    .get("clients")
                    .and_then(|v| v.as_array())
                    .and_then(|clients| clients.first())
                    .and_then(|client| client.get("session_id"))
                    .and_then(|v| v.as_str())
                {
                    return Ok(session_id.to_string());
                }
                last_observation = output;
            }
            Ok(Err(err)) => {
                last_observation = err.to_string();
            }
            Err(_) => {
                last_observation = "clients:map timed out".to_string();
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    anyhow::bail!(
        "Timed out waiting for self-dev client to connect: {}",
        last_observation
    )
}

#[cfg(unix)]
pub(crate) async fn wait_for_selfdev_reload_cycle(
    debug_socket_path: &std::path::Path,
    expected_session_id: &str,
    previous_server_id: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last_observation = "no server/client observation yet".to_string();
    let mut stable_since: Option<Instant> = None;

    while Instant::now() < deadline {
        let marker_active = jcode::server::reload_marker_active(Duration::from_secs(30));
        let server_info = match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "server:info", None),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                last_observation =
                    format!("server:info failed while marker_active={marker_active}: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                last_observation =
                    format!("server:info timed out while marker_active={marker_active}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let server_info_json: serde_json::Value = serde_json::from_str(&server_info)?;
        let Some(server_id) = server_info_json.get("id").and_then(|v| v.as_str()) else {
            last_observation = format!("server:info missing id: {}", server_info);
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };

        if server_id == previous_server_id {
            last_observation = format!(
                "server id still {} while marker_active={marker_active}",
                previous_server_id
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        let clients_map = match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "clients:map", None),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                last_observation = format!(
                    "clients:map failed on replacement server {}: {}",
                    server_id, err
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                last_observation =
                    format!("clients:map timed out on replacement server {}", server_id);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let clients_json: serde_json::Value = serde_json::from_str(&clients_map)?;
        let clients = clients_json
            .get("clients")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let session_connected = clients.iter().any(|client| {
            client.get("session_id").and_then(|v| v.as_str()) == Some(expected_session_id)
        });

        if !session_connected || clients.len() != 1 {
            last_observation = format!(
                "replacement server {} not yet stable for session {} (client_count={}): {}",
                server_id,
                expected_session_id,
                clients.len(),
                clients_map
            );
            stable_since = None;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        match stable_since {
            Some(since) if since.elapsed() >= Duration::from_millis(150) => {
                return Ok(server_id.to_string());
            }
            Some(_) => {}
            None => {
                stable_since = Some(Instant::now());
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    anyhow::bail!(
        "Self-dev reload did not reconnect within {}s: {}",
        timeout.as_secs_f32(),
        last_observation
    )
}

#[cfg(unix)]
pub(crate) async fn wait_for_selfdev_client_reload_cycle(
    debug_socket_path: &std::path::Path,
    expected_session_id: &str,
    previous_client_id: &str,
    expected_server_id: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last_observation = "no client reload observation yet".to_string();
    let mut stable_since: Option<Instant> = None;

    while Instant::now() < deadline {
        let server_info = match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "server:info", None),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                last_observation = format!("server:info failed during client reload: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                last_observation = "server:info timed out during client reload".to_string();
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let server_info_json: serde_json::Value = serde_json::from_str(&server_info)?;
        let Some(server_id) = server_info_json.get("id").and_then(|v| v.as_str()) else {
            last_observation = format!("server:info missing id: {}", server_info);
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };

        if server_id != expected_server_id {
            last_observation = format!(
                "client reload unexpectedly changed server {} -> {}",
                expected_server_id, server_id
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        let clients_map = match tokio::time::timeout(
            Duration::from_millis(750),
            debug_run_command(debug_socket_path.to_path_buf(), "clients:map", None),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                last_observation = format!("clients:map failed during client reload: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                last_observation = "clients:map timed out during client reload".to_string();
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let clients_json: serde_json::Value = serde_json::from_str(&clients_map)?;
        let clients = clients_json
            .get("clients")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let new_client_id = clients.iter().find_map(|client| {
            let session_id = client.get("session_id").and_then(|v| v.as_str())?;
            if session_id != expected_session_id {
                return None;
            }
            client
                .get("client_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });

        let Some(new_client_id) = new_client_id else {
            last_observation = format!(
                "clients:map missing session {}: {}",
                expected_session_id, clients_map
            );
            stable_since = None;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };

        if new_client_id == previous_client_id {
            last_observation = format!(
                "client id still {} for session {}",
                previous_client_id, expected_session_id
            );
            stable_since = None;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        if clients.len() != 1 {
            last_observation = format!(
                "client reload not yet stable for session {} (client_count={}): {}",
                expected_session_id,
                clients.len(),
                clients_map
            );
            stable_since = None;
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }

        match stable_since {
            Some(since) if since.elapsed() >= Duration::from_millis(150) => {
                return Ok(new_client_id);
            }
            Some(_) => {}
            None => {
                stable_since = Some(Instant::now());
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    anyhow::bail!(
        "Self-dev client reload did not reconnect within {}s: {}",
        timeout.as_secs_f32(),
        last_observation
    )
}

pub(crate) fn latest_log_excerpt(home_dir: &std::path::Path) -> Option<String> {
    let logs_dir = home_dir.join("logs");
    let mut entries = std::fs::read_dir(logs_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    let latest = entries.pop()?;
    let content = std::fs::read_to_string(latest).ok()?;
    let tail = content
        .lines()
        .rev()
        .take(120)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Some(tail)
}
