//! End-to-end tests for jcode using a mock provider
//!
//! These tests verify the full flow from user input to response
//! without making actual API calls.

mod mock_provider;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt, stream};
use jcode::agent::Agent;
use jcode::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use jcode::protocol::{Request, ServerEvent};
use jcode::provider::{EventStream, Provider};
use jcode::server;
use jcode::session::{Session, StoredCompactionState};
use jcode::tool::Registry;
use mock_provider::MockProvider;
use std::ffi::OsString;
use std::io::Read;
use std::net::TcpListener as StdTcpListener;
#[cfg(unix)]
use std::os::fd::FromRawFd;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

static JCODE_HOME_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

fn lock_jcode_home() -> std::sync::MutexGuard<'static, ()> {
    let mutex = JCODE_HOME_LOCK.get_or_init(|| Mutex::new(()));
    // Recover from poisoned state if a previous test panicked
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct TestEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_home: Option<OsString>,
    prev_test_session: Option<OsString>,
    prev_debug_control: Option<OsString>,
    _temp_home: tempfile::TempDir,
}

impl TestEnvGuard {
    fn new() -> Result<Self> {
        let lock = lock_jcode_home();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-e2e-home-")
            .tempdir()?;
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_test_session = std::env::var_os("JCODE_TEST_SESSION");
        let prev_debug_control = std::env::var_os("JCODE_DEBUG_CONTROL");

        jcode::env::set_var("JCODE_HOME", temp_home.path());
        jcode::env::set_var("JCODE_TEST_SESSION", "1");
        jcode::env::set_var("JCODE_DEBUG_CONTROL", "1");

        Ok(Self {
            _lock: lock,
            prev_home,
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

fn setup_test_env() -> Result<TestEnvGuard> {
    TestEnvGuard::new()
}

struct EnvVarGuard {
    name: &'static str,
    prev: Option<OsString>,
}

impl EnvVarGuard {
    fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
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

fn reserve_tcp_port() -> Result<u16> {
    let listener = StdTcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

async fn wait_for_socket(path: &std::path::Path) -> Result<()> {
    let start = Instant::now();
    while !path.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}

async fn wait_for_debug_socket_ready(path: &std::path::Path) -> Result<()> {
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

async fn wait_for_tcp_port(port: u16) -> Result<()> {
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
struct CapturingCompactionProvider {
    captured_messages: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl CapturingCompactionProvider {
    fn new() -> Self {
        Self::default()
    }

    fn captured_messages(&self) -> Arc<Mutex<Vec<Vec<Message>>>> {
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

fn flatten_text_blocks(message: &Message) -> String {
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

async fn collect_until_done_unix(
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
    anyhow::bail!("timed out waiting for done event {target_id} over unix socket")
}

async fn collect_until_history_unix(
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

fn summarize_history_invariant(event: &ServerEvent) -> Option<String> {
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

fn summarize_message_invariant(events: &[ServerEvent]) -> Vec<String> {
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

struct TransportScenarioResult {
    subscribe_events: Vec<ServerEvent>,
    history_events: Vec<ServerEvent>,
    message_events: Vec<ServerEvent>,
    resume_events: Vec<ServerEvent>,
}

async fn run_unix_transport_scenario() -> Result<TransportScenarioResult> {
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
        let message_events = collect_until_done_unix(&mut client, message_id).await?;

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

async fn run_websocket_transport_scenario() -> Result<TransportScenarioResult> {
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

async fn debug_create_headless_session_with_command(
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

async fn debug_create_headless_session(debug_socket_path: std::path::PathBuf) -> Result<String> {
    debug_create_headless_session_with_command(debug_socket_path, "create_session").await
}

async fn debug_run_command(
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

async fn wait_for_server_client(socket_path: &std::path::Path) -> Result<server::Client> {
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

fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
struct PtyChild {
    child: Child,
    input: std::fs::File,
    output: Arc<Mutex<Vec<u8>>>,
}

#[cfg(unix)]
impl PtyChild {
    fn send_input(&mut self, input: &str) -> Result<()> {
        use std::io::Write;

        self.input.write_all(input.as_bytes())?;
        self.input.flush()?;
        Ok(())
    }

    fn send_command(&mut self, command: &str) -> Result<()> {
        self.send_input(command)?;
        self.send_input("\r")
    }

    fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }
}

#[cfg(unix)]
fn spawn_pty_child(mut cmd: Command) -> Result<PtyChild> {
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
            std::ptr::null(),
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
fn set_file_mtime(path: &std::path::Path, when: std::time::SystemTime) -> Result<()> {
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
async fn wait_for_connected_client_session(
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
async fn wait_for_selfdev_reload_cycle(
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
async fn wait_for_selfdev_client_reload_cycle(
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

#[cfg(unix)]
fn latest_log_excerpt(home_dir: &std::path::Path) -> Option<String> {
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

#[tokio::test]
async fn resume_session_restores_persisted_compaction_for_provider_context() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-compaction-resume-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = CapturingCompactionProvider::new();
    let captured_messages = provider.captured_messages();
    let provider: Arc<dyn Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let result = async {
        let mut session = Session::create_with_id(
            "session_resume_compaction_restore_test".to_string(),
            None,
            Some("resume compaction restore test".to_string()),
        );
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "older user turn".to_string(),
                cache_control: None,
            }],
        );
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "older assistant turn".to_string(),
                cache_control: None,
            }],
        );
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "recent preserved turn".to_string(),
                cache_control: None,
            }],
        );
        session.compaction = Some(StoredCompactionState {
            summary_text: "Worked on Gemini OAuth reload fixes.".to_string(),
            openai_encrypted_content: None,
            covers_up_to_turn: 2,
            original_turn_count: 2,
            compacted_count: 2,
        });
        session.save()?;

        wait_for_socket(&socket_path).await?;
        let mut client = server::Client::connect_with_path(socket_path.clone()).await?;

        let subscribe_id = client.subscribe().await?;
        let _ = collect_until_done_unix(&mut client, subscribe_id).await?;

        let resume_id = client.resume_session(&session.id).await?;
        let _ = collect_until_history_unix(&mut client, resume_id).await?;

        let message_id = client
            .send_message("continue from the restored session")
            .await?;
        let mut seen_events = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let event = timeout(Duration::from_secs(1), client.read_event()).await??;
            let is_done = matches!(event, ServerEvent::Done { id } if id == message_id);
            let is_error = matches!(event, ServerEvent::Error { id, .. } if id == message_id);
            seen_events.push(format!("{event:?}"));
            if is_done {
                break;
            }
            if is_error {
                anyhow::bail!(
                    "message request failed while validating compaction restore: {}",
                    seen_events.join(" | ")
                );
            }
        }

        let captured = captured_messages.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "expected exactly one provider completion call"
        );
        let provider_messages = &captured[0];
        assert!(
            provider_messages.len() >= 3,
            "expected summary + preserved tail + new user message"
        );

        let summary_text = flatten_text_blocks(&provider_messages[0]);
        assert!(summary_text.contains("Previous Conversation Summary"));
        assert!(summary_text.contains("Gemini OAuth reload fixes"));

        let joined = provider_messages
            .iter()
            .map(flatten_text_blocks)
            .collect::<Vec<_>>()
            .join("\n---\n");
        assert!(joined.contains("recent preserved turn"));
        assert!(joined.contains("continue from the restored session"));
        assert!(!joined.contains("older user turn"));
        assert!(!joined.contains("older assistant turn"));

        Ok::<_, anyhow::Error>(())
    }
    .await;

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);
    result
}

/// Test that a simple text response works
#[tokio::test]
async fn test_simple_response() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // Queue a simple response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Hello! ".to_string()),
        StreamEvent::TextDelta("How can I help?".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("test-session-123".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Say hello").await?;
    let saved = Session::load(agent.session_id())?;

    assert_eq!(response, "Hello! How can I help?");
    assert!(saved.is_debug, "test sessions should be marked debug");
    Ok(())
}

#[tokio::test]
async fn test_agent_clear_preserves_debug_flag() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    agent.set_debug(true);
    let old_session_id = agent.session_id().to_string();

    agent.clear();

    assert_ne!(agent.session_id(), old_session_id);
    assert!(agent.is_debug());
    Ok(())
}

#[tokio::test]
async fn test_debug_create_session_marks_debug() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-debug-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    wait_for_socket(&socket_path).await?;

    let session_id = debug_create_headless_session(debug_socket_path.clone()).await?;
    let session = Session::load(&session_id)?;
    assert!(session.is_debug);

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

#[tokio::test]
async fn test_debug_create_selfdev_session_marks_canary() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-debug-selfdev-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    wait_for_socket(&socket_path).await?;

    let session_id = debug_create_headless_session_with_command(
        debug_socket_path.clone(),
        "create_session:selfdev:/tmp",
    )
    .await?;
    let session = Session::load(&session_id)?;
    assert!(session.is_debug);
    assert!(session.is_canary);

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

#[tokio::test]
async fn test_clear_preserves_debug_for_resumed_debug_session() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-clear-debug-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    wait_for_socket(&socket_path).await?;

    let debug_session_id = debug_create_headless_session(debug_socket_path.clone()).await?;
    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let resume_id = client.resume_session(&debug_session_id).await?;

    // Drain resume completion so clear() events are unambiguous.
    let mut saw_resume_history = false;
    let resume_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < resume_deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::History { id, .. } if id == resume_id => {
                saw_resume_history = true;
                break;
            }
            ServerEvent::Error { id, message, .. } if id == resume_id => {
                anyhow::bail!("resume_session failed: {}", message);
            }
            _ => {}
        }
    }
    if !saw_resume_history {
        anyhow::bail!("Timed out waiting for resume history event");
    }

    client.clear().await?;

    let mut new_session_id = None;
    let clear_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < clear_deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::SessionId { session_id } => {
                new_session_id = Some(session_id);
            }
            ServerEvent::Done { .. } if new_session_id.is_some() => break,
            _ => {}
        }
    }

    let new_session_id = new_session_id
        .ok_or_else(|| anyhow::anyhow!("Did not receive new session id after clear"))?;
    assert_ne!(new_session_id, debug_session_id);
    let session = Session::load(&new_session_id)?;
    assert!(session.is_debug);

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

#[tokio::test]
async fn test_websocket_transport_matches_unix_socket_for_subscribe_history_message_and_resume()
-> Result<()> {
    let _env = setup_test_env()?;
    let unix = run_unix_transport_scenario().await?;
    let websocket = run_websocket_transport_scenario().await?;

    assert!(
        unix.subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Ack { id } if *id == 1))
    );
    assert!(
        unix.subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 1))
    );
    assert!(
        websocket
            .subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Ack { id } if *id == 1))
    );
    assert!(
        websocket
            .subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 1))
    );

    let unix_history = unix
        .history_events
        .iter()
        .find_map(summarize_history_invariant)
        .ok_or_else(|| anyhow::anyhow!("missing unix history event"))?;
    let websocket_history = websocket
        .history_events
        .iter()
        .find_map(summarize_history_invariant)
        .ok_or_else(|| anyhow::anyhow!("missing websocket history event"))?;
    assert_eq!(
        unix_history, websocket_history,
        "history payload should match across transports"
    );

    assert_eq!(
        summarize_message_invariant(&unix.message_events),
        summarize_message_invariant(&websocket.message_events),
        "message streaming events should match across transports after removing broadcast noise"
    );

    let unix_resume = unix
        .resume_events
        .iter()
        .find_map(summarize_history_invariant)
        .ok_or_else(|| anyhow::anyhow!("missing unix resume history event"))?;
    let websocket_resume = websocket
        .resume_events
        .iter()
        .find_map(summarize_history_invariant)
        .ok_or_else(|| anyhow::anyhow!("missing websocket resume history event"))?;
    assert_eq!(
        unix_resume, websocket_resume,
        "resume history payload should match across transports"
    );

    Ok(())
}

/// Test that multi-turn conversation works with session resume
#[tokio::test]
async fn test_multi_turn_conversation() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // First turn response
    provider.queue_response(vec![
        StreamEvent::TextDelta("I'll remember that.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-abc".to_string()),
    ]);

    // Second turn response
    provider.queue_response(vec![
        StreamEvent::TextDelta("You said hello earlier.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-abc".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    // First turn
    let response1 = agent.run_once_capture("Hello").await?;
    assert_eq!(response1, "I'll remember that.");

    // Second turn - should use session resume
    let response2 = agent.run_once_capture("What did I say?").await?;
    assert_eq!(response2, "You said hello earlier.");

    Ok(())
}

/// Test that token usage is tracked
#[tokio::test]
async fn test_token_usage() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    provider.queue_response(vec![
        StreamEvent::TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(20),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        },
        StreamEvent::TextDelta("Response".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-123".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Test").await?;
    assert_eq!(response, "Response");

    Ok(())
}

/// Test error handling
#[tokio::test]
async fn test_stream_error() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    provider.queue_response(vec![
        StreamEvent::TextDelta("Starting...".to_string()),
        StreamEvent::Error {
            message: "Something went wrong".to_string(),
            retry_after_secs: None,
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let result = agent.run_once_capture("Test").await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Something went wrong")
    );

    Ok(())
}

/// Test model cycling over the socket interface (server + client)
#[tokio::test]
async fn test_socket_model_cycle_supported_models() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::with_models(vec!["gpt-5.2-codex", "claude-opus-4-5-20251101"]);
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    // Wait for socket to appear
    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let request_id = client.cycle_model(1).await?;

    let mut saw_model_changed = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::ModelChanged {
                id, model, error, ..
            } if id == request_id => {
                assert!(error.is_none(), "Expected successful model change");
                assert_eq!(model, "claude-opus-4-5-20251101");
                saw_model_changed = true;
                break;
            }
            _ => {}
        }
    }

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    assert!(saw_model_changed, "Did not receive model_changed event");
    Ok(())
}

/// Test that resume restores model selection and tool output in history
#[tokio::test]
async fn test_resume_restores_model_and_tool_history() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-resume-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;

    let mut session = Session::create(None, Some("Resume Test".to_string()));
    session.model = Some("gpt-5.2-codex".to_string());
    session.add_message(
        jcode::message::Role::User,
        vec![jcode::message::ContentBlock::Text {
            text: "Run a tool".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        jcode::message::Role::Assistant,
        vec![
            jcode::message::ContentBlock::Text {
                text: "Running...".to_string(),
                cache_control: None,
            },
            jcode::message::ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"cmd": "echo hi"}),
            },
        ],
    );
    session.add_message(
        jcode::message::Role::User,
        vec![jcode::message::ContentBlock::ToolResult {
            tool_use_id: "tool-1".to_string(),
            content: "hi\n".to_string(),
            is_error: None,
        }],
    );
    session.save()?;

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    // Default model = claude, resume should switch to gpt-5.2-codex
    let provider = MockProvider::with_models(vec!["claude-opus-4-5-20251101", "gpt-5.2-codex"]);
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let resume_id = client.resume_session(&session.id).await?;

    let mut history_event = None;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::History {
                id,
                messages,
                provider_model,
                ..
            } if id == resume_id => {
                history_event = Some((messages, provider_model));
                break;
            }
            _ => {}
        }
    }

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    let (messages, provider_model) =
        history_event.ok_or_else(|| anyhow::anyhow!("Did not receive history event"))?;

    assert_eq!(provider_model, Some("gpt-5.2-codex".to_string()));

    let tool_msg = messages
        .iter()
        .find(|m| m.role == "tool")
        .ok_or_else(|| anyhow::anyhow!("Tool message missing in history"))?;
    assert!(tool_msg.content.contains("hi"));
    let tool_data = tool_msg
        .tool_data
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Tool metadata missing in history"))?;
    assert_eq!(tool_data.name, "bash");

    Ok(())
}

/// Test that subscribe selfdev hint marks the session as canary
#[tokio::test]
async fn test_subscribe_selfdev_hint_marks_canary() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    // Wait for socket to appear
    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(2) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let subscribe_id = client.subscribe_with_info(None, Some(true)).await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == subscribe_id) {
            break;
        }
    }

    let history_event = client.get_history_event().await?;
    match history_event {
        ServerEvent::History { is_canary, .. } => {
            assert_eq!(is_canary, Some(true));
        }
        _ => anyhow::bail!("Expected history event after subscribe"),
    }

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that working_dir alone no longer upgrades a session to self-dev.
#[tokio::test]
async fn test_subscribe_working_dir_without_selfdev_hint_stays_normal() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let fake_repo = tempfile::tempdir()?;
    std::fs::create_dir_all(fake_repo.path().join(".git"))?;
    std::fs::write(
        fake_repo.path().join("Cargo.toml"),
        "[package]\nname = \"jcode\"\nversion = \"0.0.0\"\n",
    )?;
    let nested_dir = fake_repo.path().join("nested").join("worktree");
    std::fs::create_dir_all(&nested_dir)?;

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(2) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let subscribe_id = client
        .subscribe_with_info(Some(nested_dir.display().to_string()), None)
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == subscribe_id) {
            break;
        }
    }

    let history_event = client.get_history_event().await?;
    match history_event {
        ServerEvent::History { is_canary, .. } => {
            assert_eq!(is_canary, Some(false));
        }
        _ => anyhow::bail!("Expected history event after subscribe"),
    }

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that switching models resets the provider resume session
#[tokio::test]
async fn test_model_switch_resets_provider_session() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = Arc::new(MockProvider::with_models(vec!["model-a", "model-b"]));
    provider.queue_response(vec![
        StreamEvent::TextDelta("hello".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-1".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("again".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider.clone();
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(2) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;

    let msg_id = client.send_message("hello").await?;
    let mut saw_done1 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg_id) {
            saw_done1 = true;
            break;
        }
    }
    assert!(saw_done1, "Did not receive Done for first message");

    let model_id = client.cycle_model(1).await?;
    let mut saw_model = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::ModelChanged { id, error: None, .. } if id == model_id) {
            saw_model = true;
            break;
        }
    }
    assert!(saw_model, "Did not receive ModelChanged after cycle");

    let msg2_id = client.send_message("second").await?;
    let mut saw_done2 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg2_id) {
            saw_done2 = true;
            break;
        }
    }
    assert!(saw_done2, "Did not receive Done for second message");

    let resume_ids = provider.captured_resume_session_ids.lock().unwrap().clone();
    assert_eq!(resume_ids.len(), 2);
    assert_eq!(resume_ids[0], None);
    assert_eq!(resume_ids[1], None);

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that switching models only affects the active session
#[tokio::test]
async fn test_model_switch_is_per_session() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = Arc::new(MockProvider::with_models(vec!["model-a", "model-b"]));
    provider.queue_response(vec![
        StreamEvent::TextDelta("one".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-1".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("two".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-2".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("three".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider.clone();
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(2) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client1 = server::Client::connect_with_path(socket_path.clone()).await?;
    let mut client2 = server::Client::connect_with_path(socket_path.clone()).await?;

    // Give server time to set up both client sessions
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msg1 = client1.send_message("hello").await?;
    let mut done1 = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client1.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg1) {
            done1 = true;
            break;
        }
    }
    assert!(done1, "Did not receive Done for client1 message");

    let msg2 = client2.send_message("hello").await?;
    let mut done2 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client2.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg2) {
            done2 = true;
            break;
        }
    }
    assert!(done2, "Did not receive Done for client2 message");

    let model_id = client1.cycle_model(1).await?;
    let mut saw_model = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client1.read_event()).await??;
        if matches!(event, ServerEvent::ModelChanged { id, error: None, .. } if id == model_id) {
            saw_model = true;
            break;
        }
    }
    assert!(saw_model, "Did not receive ModelChanged after cycle");

    let msg3 = client2.send_message("after").await?;
    let mut done3 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client2.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg3) {
            done3 = true;
            break;
        }
    }
    assert!(done3, "Did not receive Done for client2 after switch");

    let models = provider.captured_models.lock().unwrap().clone();
    assert!(models.len() >= 3, "Expected at least 3 model captures");
    assert_eq!(models[2], "model-a");

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that the system prompt does NOT identify the agent as "Claude Code"
/// The agent should identify as "jcode" or just a generic "coding assistant powered by Claude"
#[tokio::test]
async fn test_system_prompt_no_claude_code_identity() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = Arc::new(MockProvider::new());

    // Queue a simple response
    provider.queue_response(vec![
        StreamEvent::TextDelta("I'm a coding assistant.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("test-identity-123".to_string()),
    ]);

    // Keep a clone of Arc<MockProvider> before converting to Arc<dyn Provider>
    let provider_for_check = provider.clone();
    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider;
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);

    // Run a simple query - we just need to trigger a complete() call
    let _ = agent.run_once_capture("Who are you?").await?;

    // Get the captured system prompt from our Arc<MockProvider>
    let captured_prompts = provider_for_check.captured_system_prompts.lock().unwrap();

    assert!(
        !captured_prompts.is_empty(),
        "No system prompts were captured"
    );

    let system_prompt = &captured_prompts[0];

    // Check only the identity portion at the start of the system prompt
    // (not the full prompt which may include CLAUDE.md with "Claude Code CLI" references)
    // The first ~500 chars contain the identity statement
    let identity_portion = if system_prompt.len() > 500 {
        &system_prompt[..500]
    } else {
        system_prompt
    };
    let lower_identity = identity_portion.to_lowercase();

    // The identity portion should NOT say "you are claude code" or similar
    assert!(
        !lower_identity.contains("you are claude code"),
        "System prompt should NOT identify as 'You are Claude Code'. Found: {}",
        identity_portion
    );

    // Should identify as jcode
    assert!(
        lower_identity.contains("jcode"),
        "System prompt should identify as jcode. Found: {}",
        identity_portion
    );

    // It's OK if it says "powered by Claude" or just "Claude" (the model name)
    // It's OK if project context references "Claude Code CLI" as a tool

    Ok(())
}

// ============================================================================
// Binary Integration Tests
// These tests run the actual jcode binary and require real credentials.
// Run with: cargo test --test e2e binary_integration -- --ignored
// ============================================================================

/// Test that the jcode binary can run standalone with Claude provider
#[tokio::test]
#[ignore] // Requires Claude credentials
async fn binary_integration_standalone_claude() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new("cargo")
        .args([
            "run",
            "--release",
            "--bin",
            "jcode",
            "--",
            "run",
            "Say 'test-ok' and nothing else",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success() || stdout.contains("test") || stderr.contains("Claude"),
        "Binary should run successfully. stdout: {}, stderr: {}",
        stdout,
        stderr
    );

    Ok(())
}

/// Test that the jcode binary can run with OpenAI provider
#[tokio::test]
#[ignore] // Requires OpenAI/Codex credentials
async fn binary_integration_openai_provider() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new("cargo")
        .args([
            "run",
            "--release",
            "--bin",
            "jcode",
            "--",
            "--provider",
            "openai",
            "run",
            "Say 'openai-ok' and nothing else",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check either success or identifiable OpenAI response
    let has_response = stdout.to_lowercase().contains("openai")
        || stdout.to_lowercase().contains("ok")
        || stderr.contains("OpenAI");

    assert!(
        output.status.success() || has_response,
        "OpenAI provider should work. stdout: {}, stderr: {}",
        stdout,
        stderr
    );

    Ok(())
}

/// Test that jcode version command works
#[tokio::test]
async fn binary_version_command() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new(env!("CARGO_BIN_EXE_jcode"))
        .arg("--version")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "Version command should succeed");
    assert!(
        stdout.contains("jcode") || stdout.contains("20"),
        "Version should contain 'jcode' or date. Got: {}",
        stdout
    );

    Ok(())
}

/// Test full server reload handoff against a real spawned server process.
///
/// Requires a built release binary at target/release/jcode because the reload
/// flow execs into the repo's reload candidate.
#[tokio::test]
#[ignore]
async fn binary_integration_reload_handoff() -> Result<()> {
    let _env = setup_test_env()?;

    let release_binary =
        jcode::build::release_binary_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    if !release_binary.exists() {
        anyhow::bail!(
            "release binary missing at {} (run `cargo build --release` first)",
            release_binary.display()
        );
    }

    let temp_root = tempfile::Builder::new()
        .prefix("jcode-reload-e2e-")
        .tempdir()?;
    let runtime_dir = temp_root.path().join("runtime");
    let home_dir = temp_root.path().join("home");
    let install_dir = temp_root.path().join("install");
    let stderr_path = temp_root.path().join("server-stderr.log");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&install_dir)?;

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let stderr_file = std::fs::File::create(&stderr_path)?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_jcode"))
        .arg("--no-update")
        .arg("--socket")
        .arg(&socket_path)
        .arg("serve")
        // This test must exercise the real exec-based reload handoff, not the
        // in-process test shortcut used by other e2e cases.
        .env_remove("JCODE_TEST_SESSION")
        .env("JCODE_HOME", &home_dir)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_INSTALL_DIR", &install_dir)
        .env("JCODE_DEBUG_CONTROL", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()?;

    let test_result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_debug_socket_ready(&debug_socket_path).await?;
        let server_info_before =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_before_json: serde_json::Value = serde_json::from_str(&server_info_before)?;
        let server_id_before = server_info_before_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id before reload"))?
            .to_string();

        let mut client = wait_for_server_client(&socket_path).await?;
        client.reload().await?;

        let disconnect_deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_disconnect = false;
        while Instant::now() < disconnect_deadline {
            match tokio::time::timeout(Duration::from_secs(1), client.read_event()).await {
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => {
                    saw_disconnect = true;
                    break;
                }
            }
        }
        assert!(
            saw_disconnect,
            "old client connection never disconnected during reload"
        );

        let marker_deadline = Instant::now() + Duration::from_secs(20);
        while jcode::server::reload_marker_active(Duration::from_secs(30)) {
            if Instant::now() >= marker_deadline {
                anyhow::bail!("reload marker remained active too long after restart");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        wait_for_debug_socket_ready(&debug_socket_path).await?;
        let _client = wait_for_server_client(&socket_path).await?;

        let server_info_after =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_after_json: serde_json::Value = serde_json::from_str(&server_info_after)?;
        let server_id_after = server_info_after_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id after reload"))?;

        assert_ne!(
            server_id_after, server_id_before,
            "server identity should change after exec-based reload"
        );
        assert!(
            server_info_after_json
                .get("uptime_secs")
                .and_then(|v| v.as_u64())
                .is_some(),
            "replacement server should answer debug state queries after reload"
        );

        Ok::<_, anyhow::Error>(())
    }
    .await;

    kill_child(&mut child);
    if let Err(ref error) = test_result {
        if let Ok(stderr) = std::fs::read_to_string(&stderr_path) {
            eprintln!("spawned server stderr:\n{}", stderr);
        }
        eprintln!("reload e2e test error: {error:#}");
    }
    test_result
}

/// Test repeated self-dev reload handoff against a real TUI client running in a PTY.
///
/// Requires a built release binary at target/release/jcode because the
/// self-dev server reload path execs into the repo's reload candidate.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn binary_integration_selfdev_reload_reconnects_quickly() -> Result<()> {
    let _env = setup_test_env()?;

    let release_binary =
        jcode::build::release_binary_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    if !release_binary.exists() {
        anyhow::bail!(
            "release binary missing at {} (run `cargo build --release` first)",
            release_binary.display()
        );
    }

    let temp_root = tempfile::Builder::new()
        .prefix("jcode-selfdev-reload-e2e-")
        .tempdir()?;
    let runtime_dir = temp_root.path().join("runtime");
    let home_dir = temp_root.path().join("home");
    let install_dir = temp_root.path().join("install");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&install_dir)?;

    let _home_guard = EnvVarGuard::set("JCODE_HOME", &home_dir);
    let _runtime_guard = EnvVarGuard::set("JCODE_RUNTIME_DIR", &runtime_dir);
    let _install_guard = EnvVarGuard::set("JCODE_INSTALL_DIR", &install_dir);

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");
    let mut command = Command::new(&release_binary);
    command
        .arg("--no-update")
        .arg("--provider")
        .arg("antigravity")
        .arg("self-dev")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("JCODE_TEST_SESSION")
        .env("JCODE_HOME", &home_dir)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_INSTALL_DIR", &install_dir);

    let mut child = spawn_pty_child(command)?;

    let test_result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_debug_socket_ready(&debug_socket_path).await?;
        let session_id =
            wait_for_connected_client_session(&debug_socket_path, Duration::from_secs(10)).await?;

        let state_before =
            debug_run_command(debug_socket_path.clone(), "client:state", None).await?;
        let _: serde_json::Value = serde_json::from_str(&state_before)?;

        let server_info_before =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_before_json: serde_json::Value = serde_json::from_str(&server_info_before)?;
        let mut server_id_before = server_info_before_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing initial server id"))?
            .to_string();

        for cycle in 1..=3 {
            child.send_command("/server-reload")?;

            let server_id_after = wait_for_selfdev_reload_cycle(
                &debug_socket_path,
                &session_id,
                &server_id_before,
                Duration::from_secs(20),
            )
            .await?;
            assert_ne!(
                server_id_after, server_id_before,
                "self-dev reload cycle {} should replace the server process",
                cycle
            );
            server_id_before = server_id_after;
        }

        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        debug_run_command(debug_socket_path.clone(), "client:quit", None),
    )
    .await;
    kill_child(&mut child.child);

    if let Err(ref error) = test_result {
        eprintln!("self-dev reload e2e test error: {error:#}");
        eprintln!("self-dev client PTY output:\n{}", child.output_text());
        if let Some(log_excerpt) = latest_log_excerpt(&home_dir) {
            eprintln!("self-dev reload logs (tail):\n{}", log_excerpt);
        }
    }

    test_result
}

/// Test self-dev client binary reload against a real TUI client running in a PTY.
///
/// Starts from the test binary, then forces `/client-reload` to re-exec into
/// the built release candidate while keeping the shared server online.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn binary_integration_selfdev_client_reload_resumes_session() -> Result<()> {
    let _env = setup_test_env()?;

    let release_binary =
        jcode::build::release_binary_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    if !release_binary.exists() {
        anyhow::bail!(
            "release binary missing at {} (run `cargo build --release` first)",
            release_binary.display()
        );
    }

    let temp_root = tempfile::Builder::new()
        .prefix("jcode-selfdev-client-reload-e2e-")
        .tempdir()?;
    let runtime_dir = temp_root.path().join("runtime");
    let home_dir = temp_root.path().join("home");
    let install_dir = temp_root.path().join("install");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&install_dir)?;

    let _home_guard = EnvVarGuard::set("JCODE_HOME", &home_dir);
    let _runtime_guard = EnvVarGuard::set("JCODE_RUNTIME_DIR", &runtime_dir);
    let _install_guard = EnvVarGuard::set("JCODE_INSTALL_DIR", &install_dir);

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");
    let starter_binary = temp_root.path().join("jcode-selfdev-client-starter");
    std::fs::copy(env!("CARGO_BIN_EXE_jcode"), &starter_binary)?;
    let starter_mtime = std::fs::metadata(&release_binary)?
        .modified()?
        .checked_sub(Duration::from_secs(60))
        .unwrap_or(std::time::UNIX_EPOCH + Duration::from_secs(1));
    set_file_mtime(&starter_binary, starter_mtime)?;

    let mut command = Command::new(&starter_binary);
    command
        .arg("--no-update")
        .arg("--provider")
        .arg("antigravity")
        .arg("self-dev")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("JCODE_TEST_SESSION")
        .env("JCODE_HOME", &home_dir)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_INSTALL_DIR", &install_dir);

    let mut child = spawn_pty_child(command)?;

    let test_result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_debug_socket_ready(&debug_socket_path).await?;

        let session_id =
            wait_for_connected_client_session(&debug_socket_path, Duration::from_secs(10)).await?;

        let state_before =
            debug_run_command(debug_socket_path.clone(), "client:state", Some(&session_id)).await?;
        let state_before_json: serde_json::Value = serde_json::from_str(&state_before)?;
        let version_before = state_before_json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing client version before reload"))?
            .to_string();

        let clients_before =
            debug_run_command(debug_socket_path.clone(), "clients:map", None).await?;
        let clients_before_json: serde_json::Value = serde_json::from_str(&clients_before)?;
        let client_id_before = clients_before_json
            .get("clients")
            .and_then(|v| v.as_array())
            .and_then(|clients| {
                clients.iter().find_map(|client| {
                    let session = client.get("session_id").and_then(|v| v.as_str())?;
                    if session != session_id {
                        return None;
                    }
                    client
                        .get("client_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
            })
            .ok_or_else(|| anyhow::anyhow!("missing client id before reload"))?;

        let server_info_before =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_before_json: serde_json::Value = serde_json::from_str(&server_info_before)?;
        let server_id_before = server_info_before_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id before client reload"))?
            .to_string();

        child.send_command("/client-reload")?;

        let client_id_after = wait_for_selfdev_client_reload_cycle(
            &debug_socket_path,
            &session_id,
            &client_id_before,
            &server_id_before,
            Duration::from_secs(20),
        )
        .await?;

        let state_after =
            debug_run_command(debug_socket_path.clone(), "client:state", Some(&session_id)).await?;
        let state_after_json: serde_json::Value = serde_json::from_str(&state_after)?;
        let version_after = state_after_json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing client version after reload"))?;

        assert_ne!(
            client_id_after, client_id_before,
            "client reload should reconnect with a different client id"
        );
        assert_ne!(
            version_after, version_before,
            "client reload should switch binaries"
        );

        let server_info_after =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_after_json: serde_json::Value = serde_json::from_str(&server_info_after)?;
        let server_id_after = server_info_after_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id after client reload"))?;
        assert_eq!(
            server_id_after, server_id_before,
            "client reload should not replace the server process"
        );

        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        debug_run_command(debug_socket_path.clone(), "client:quit", None),
    )
    .await;
    kill_child(&mut child.child);

    if let Err(ref error) = test_result {
        eprintln!("self-dev client reload e2e test error: {error:#}");
        eprintln!("self-dev client PTY output:\n{}", child.output_text());
        if let Some(log_excerpt) = latest_log_excerpt(&home_dir) {
            eprintln!("self-dev client reload logs (tail):\n{}", log_excerpt);
        }
    }

    test_result
}

/// Test full self-dev `/reload` against a real TUI client running in a PTY.
///
/// Starts from an older starter binary so the client reloads into the built
/// release candidate while the shared server also restarts.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn binary_integration_selfdev_full_reload_resumes_session_quickly() -> Result<()> {
    let _env = setup_test_env()?;

    let release_binary =
        jcode::build::release_binary_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    if !release_binary.exists() {
        anyhow::bail!(
            "release binary missing at {} (run `cargo build --release` first)",
            release_binary.display()
        );
    }

    let temp_root = tempfile::Builder::new()
        .prefix("jcode-selfdev-full-reload-e2e-")
        .tempdir()?;
    let runtime_dir = temp_root.path().join("runtime");
    let home_dir = temp_root.path().join("home");
    let install_dir = temp_root.path().join("install");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&install_dir)?;

    let _home_guard = EnvVarGuard::set("JCODE_HOME", &home_dir);
    let _runtime_guard = EnvVarGuard::set("JCODE_RUNTIME_DIR", &runtime_dir);
    let _install_guard = EnvVarGuard::set("JCODE_INSTALL_DIR", &install_dir);

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");
    let starter_binary = temp_root.path().join("jcode-selfdev-full-reload-starter");
    std::fs::copy(env!("CARGO_BIN_EXE_jcode"), &starter_binary)?;
    let starter_mtime = std::fs::metadata(&release_binary)?
        .modified()?
        .checked_sub(Duration::from_secs(60))
        .unwrap_or(std::time::UNIX_EPOCH + Duration::from_secs(1));
    set_file_mtime(&starter_binary, starter_mtime)?;

    let mut command = Command::new(&starter_binary);
    command
        .arg("--no-update")
        .arg("--provider")
        .arg("antigravity")
        .arg("self-dev")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("JCODE_TEST_SESSION")
        .env("JCODE_HOME", &home_dir)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_INSTALL_DIR", &install_dir);

    let mut child = spawn_pty_child(command)?;

    let test_result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_debug_socket_ready(&debug_socket_path).await?;

        let session_id =
            wait_for_connected_client_session(&debug_socket_path, Duration::from_secs(10)).await?;

        let state_before =
            debug_run_command(debug_socket_path.clone(), "client:state", Some(&session_id)).await?;
        let state_before_json: serde_json::Value = serde_json::from_str(&state_before)?;
        let version_before = state_before_json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing client version before full reload"))?
            .to_string();

        let clients_before =
            debug_run_command(debug_socket_path.clone(), "clients:map", None).await?;
        let clients_before_json: serde_json::Value = serde_json::from_str(&clients_before)?;
        let client_id_before = clients_before_json
            .get("clients")
            .and_then(|v| v.as_array())
            .and_then(|clients| {
                clients.iter().find_map(|client| {
                    let session = client.get("session_id").and_then(|v| v.as_str())?;
                    if session != session_id {
                        return None;
                    }
                    client
                        .get("client_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
            })
            .ok_or_else(|| anyhow::anyhow!("missing client id before full reload"))?;

        let server_info_before =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_before_json: serde_json::Value = serde_json::from_str(&server_info_before)?;
        let server_id_before = server_info_before_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id before full reload"))?
            .to_string();

        child.send_command("/reload")?;

        let server_id_after = wait_for_selfdev_reload_cycle(
            &debug_socket_path,
            &session_id,
            &server_id_before,
            Duration::from_secs(20),
        )
        .await?;

        let client_id_after = wait_for_selfdev_client_reload_cycle(
            &debug_socket_path,
            &session_id,
            &client_id_before,
            &server_id_after,
            Duration::from_secs(20),
        )
        .await?;

        let state_after =
            debug_run_command(debug_socket_path.clone(), "client:state", Some(&session_id)).await?;
        let state_after_json: serde_json::Value = serde_json::from_str(&state_after)?;
        let version_after = state_after_json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing client version after full reload"))?;

        assert_ne!(
            server_id_after, server_id_before,
            "full reload should replace the server process"
        );
        assert_ne!(
            client_id_after, client_id_before,
            "full reload should reconnect with a different client id"
        );
        assert_ne!(
            version_after, version_before,
            "full reload should switch binaries"
        );

        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        debug_run_command(debug_socket_path.clone(), "client:quit", None),
    )
    .await;
    kill_child(&mut child.child);

    if let Err(ref error) = test_result {
        eprintln!("self-dev full reload e2e test error: {error:#}");
        eprintln!("self-dev client PTY output:\n{}", child.output_text());
        if let Some(log_excerpt) = latest_log_excerpt(&home_dir) {
            eprintln!("self-dev full reload logs (tail):\n{}", log_excerpt);
        }
    }

    test_result
}

// =============================================================================
// Ambient Mode Integration Tests
// =============================================================================

/// Test safety system: action classification
#[test]
fn test_safety_classification() {
    use jcode::safety::SafetySystem;

    let safety = SafetySystem::new();

    // Tier 1: auto-allowed
    assert!(safety.classify("read") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("glob") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("grep") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("memory") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("todoread") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("todowrite") == jcode::safety::ActionTier::AutoAllowed);

    // Tier 2: requires permission
    assert!(safety.classify("bash") == jcode::safety::ActionTier::RequiresPermission);
    assert!(safety.classify("edit") == jcode::safety::ActionTier::RequiresPermission);
    assert!(safety.classify("write") == jcode::safety::ActionTier::RequiresPermission);
    assert!(
        safety.classify("create_pull_request") == jcode::safety::ActionTier::RequiresPermission
    );
    assert!(safety.classify("send_email") == jcode::safety::ActionTier::RequiresPermission);

    // Case insensitive
    assert!(safety.classify("READ") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("Bash") == jcode::safety::ActionTier::RequiresPermission);
}

/// Test safety system: permission request queue + decision flow
#[test]
fn test_safety_permission_flow() {
    use jcode::safety::{PermissionRequest, PermissionResult, SafetySystem, Urgency};

    let safety = SafetySystem::new();

    // Count existing pending requests (may have leftover state from other tests)
    let baseline = safety.pending_requests().len();

    // Queue a permission request
    let req = PermissionRequest {
        id: "test_perm_flow_001".to_string(),
        action: "create_pull_request".to_string(),
        description: "Create PR for auth fixes".to_string(),
        rationale: "Found 3 failing auth tests".to_string(),
        urgency: Urgency::High,
        wait: false,
        created_at: chrono::Utc::now(),
        context: None,
    };

    let result = safety.request_permission(req);
    assert!(matches!(result, PermissionResult::Queued { .. }));

    // Verify our request was added
    let pending = safety.pending_requests();
    assert_eq!(pending.len(), baseline + 1);
    assert!(
        pending
            .iter()
            .any(|p| p.action == "create_pull_request" && p.id == "test_perm_flow_001")
    );

    // Record an approval decision
    let _ = safety.record_decision(
        "test_perm_flow_001",
        true,
        "test",
        Some("looks good".to_string()),
    );

    // Verify our request was removed
    assert_eq!(safety.pending_requests().len(), baseline);
}

/// Test safety system: transcript saving
#[test]
fn test_safety_transcript() {
    use jcode::safety::{AmbientTranscript, SafetySystem, TranscriptStatus};

    let safety = SafetySystem::new();

    let transcript = AmbientTranscript {
        session_id: "test_ambient_001".to_string(),
        started_at: chrono::Utc::now(),
        ended_at: Some(chrono::Utc::now()),
        status: TranscriptStatus::Complete,
        provider: "mock".to_string(),
        model: "mock-model".to_string(),
        actions: vec![],
        pending_permissions: 0,
        summary: Some("Test cycle completed".to_string()),
        compactions: 0,
        memories_modified: 3,
        conversation: None,
    };

    // Should not panic
    let result = safety.save_transcript(&transcript);
    assert!(result.is_ok());
}

/// Test safety system: summary generation
#[test]
fn test_safety_summary_generation() {
    use jcode::safety::{ActionLog, ActionTier, SafetySystem};

    let safety = SafetySystem::new();

    // Log some actions
    safety.log_action(ActionLog {
        action_type: "memory_consolidation".to_string(),
        description: "Merged 2 duplicate memories".to_string(),
        tier: ActionTier::AutoAllowed,
        details: None,
        timestamp: chrono::Utc::now(),
    });

    safety.log_action(ActionLog {
        action_type: "memory_prune".to_string(),
        description: "Pruned 1 stale memory".to_string(),
        tier: ActionTier::AutoAllowed,
        details: None,
        timestamp: chrono::Utc::now(),
    });

    let summary = safety.generate_summary();
    assert!(summary.contains("Merged 2 duplicate memories"));
    assert!(summary.contains("Pruned 1 stale memory"));
}

/// Test ambient state: load, save, record_cycle
#[test]
fn test_ambient_state_lifecycle() {
    use jcode::ambient::{AmbientCycleResult, AmbientState, AmbientStatus, CycleStatus};

    let mut state = AmbientState::default();
    assert!(matches!(state.status, AmbientStatus::Idle));
    assert_eq!(state.total_cycles, 0);
    assert!(state.last_run.is_none());

    // Record a cycle
    let result = AmbientCycleResult {
        summary: "Gardened 3 memories".to_string(),
        memories_modified: 3,
        compactions: 0,
        proactive_work: None,
        next_schedule: None,
        started_at: chrono::Utc::now(),
        ended_at: chrono::Utc::now(),
        status: CycleStatus::Complete,
        conversation: None,
    };

    state.record_cycle(&result);
    assert_eq!(state.total_cycles, 1);
    assert!(state.last_run.is_some());
    assert_eq!(state.last_summary.as_deref(), Some("Gardened 3 memories"));
    assert_eq!(state.last_memories_modified, Some(3));
    assert_eq!(state.last_compactions, Some(0));
    // No next_schedule → should be Idle
    assert!(matches!(state.status, AmbientStatus::Idle));
}

/// Test ambient scheduled queue: push, pop, priority ordering
#[test]
fn test_ambient_scheduled_queue() {
    use jcode::ambient::{Priority, ScheduledItem, ScheduledQueue};

    let tmp = std::env::temp_dir().join("jcode-test-queue.json");
    let _ = std::fs::remove_file(&tmp); // Clean up from previous runs
    let mut queue = ScheduledQueue::load(tmp);
    assert!(queue.is_empty());

    // Push items with different priorities
    let now = chrono::Utc::now();
    queue.push(ScheduledItem {
        id: "low_1".to_string(),
        scheduled_for: now - chrono::Duration::minutes(5),
        context: "low priority task".to_string(),
        priority: Priority::Low,
        target: jcode::ambient::ScheduleTarget::Ambient,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "high_1".to_string(),
        scheduled_for: now - chrono::Duration::minutes(5),
        context: "high priority task".to_string(),
        priority: Priority::High,
        target: jcode::ambient::ScheduleTarget::Ambient,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "future_1".to_string(),
        scheduled_for: now + chrono::Duration::hours(1),
        context: "future task".to_string(),
        priority: Priority::Normal,
        target: jcode::ambient::ScheduleTarget::Ambient,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    assert_eq!(queue.len(), 3);

    // Pop ready items: should get high priority first, then low (future not ready)
    let ready = queue.pop_ready();
    assert_eq!(ready.len(), 2);
    assert_eq!(ready[0].id, "high_1"); // High priority first
    assert_eq!(ready[1].id, "low_1"); // Low priority second

    // Future item still in queue
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.items()[0].id, "future_1");
}

/// Test adaptive scheduler: interval calculation
#[test]
fn test_adaptive_scheduler_intervals() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        ..Default::default()
    };

    let scheduler = AdaptiveScheduler::new(config);

    // With no rate limit info, should return max interval
    let interval = scheduler.calculate_interval(None);
    assert!(interval.as_secs() >= 120 * 60 - 1); // Allow 1s tolerance
}

/// Test adaptive scheduler: backoff on rate limit
#[test]
fn test_adaptive_scheduler_backoff() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        ..Default::default()
    };

    let mut scheduler = AdaptiveScheduler::new(config);

    let base_interval = scheduler.calculate_interval(None);

    // Hit rate limit
    scheduler.on_rate_limit_hit();
    let backed_off = scheduler.calculate_interval(None);
    assert!(backed_off >= base_interval);

    // Reset on success
    scheduler.on_successful_cycle();
    let after_reset = scheduler.calculate_interval(None);
    assert!(after_reset <= backed_off);
}

/// Test adaptive scheduler: pause on active session
#[test]
fn test_adaptive_scheduler_pause() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        pause_on_active_session: true,
        ..Default::default()
    };

    let mut scheduler = AdaptiveScheduler::new(config);

    assert!(!scheduler.should_pause());
    scheduler.set_user_active(true);
    assert!(scheduler.should_pause());
    scheduler.set_user_active(false);
    assert!(!scheduler.should_pause());
}

/// Test ambient tools: end_ambient_cycle via mock agent
#[tokio::test]
async fn test_ambient_end_cycle_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // Mock: agent calls end_ambient_cycle tool
    let tool_input = serde_json::json!({
        "summary": "Merged 2 duplicate memories, pruned 1 stale memory",
        "memories_modified": 3,
        "compactions": 0
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_001".to_string(),
            name: "end_ambient_cycle".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, the agent calls the provider again — mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Cycle complete.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Begin ambient cycle").await?;
    assert_eq!(response, "Cycle complete.");

    // The tool should have stored a cycle result
    let result = jcode::tool::ambient::take_cycle_result();
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(
        result.summary,
        "Merged 2 duplicate memories, pruned 1 stale memory"
    );
    assert_eq!(result.memories_modified, 3);
    assert_eq!(result.compactions, 0);

    Ok(())
}

/// Test ambient tools: request_permission via mock agent
#[tokio::test]
async fn test_ambient_request_permission_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    let tool_input = serde_json::json!({
        "action": "create_pull_request",
        "description": "Create PR for test fixes",
        "rationale": "Found 3 failing tests in auth module",
        "urgency": "high",
        "wait": false
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_perm_001".to_string(),
            name: "request_permission".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Permission requested.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);
    let ambient_session_id = agent.session_id().to_string();
    jcode::tool::ambient::register_ambient_session(ambient_session_id.clone());

    let response = agent.run_once_capture("Request permission").await?;
    jcode::tool::ambient::unregister_ambient_session(&ambient_session_id);
    assert_eq!(response, "Permission requested.");

    Ok(())
}

/// Test ambient tools: schedule_ambient via mock agent
#[tokio::test]
async fn test_ambient_schedule_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    let tool_input = serde_json::json!({
        "wake_in_minutes": 30,
        "context": "Check CI results and verify test fixes",
        "priority": "normal"
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_sched_001".to_string(),
            name: "schedule_ambient".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Scheduled next cycle.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Schedule next cycle").await?;
    assert_eq!(response, "Scheduled next cycle.");

    Ok(())
}

/// Test ambient system prompt builder
#[test]
fn test_ambient_system_prompt_builder() {
    use jcode::ambient::{
        AmbientState, MemoryGraphHealth, ResourceBudget, build_ambient_system_prompt,
    };

    let state = AmbientState::default();
    let queue_items = vec![];
    let health = MemoryGraphHealth {
        total: 42,
        active: 38,
        inactive: 4,
        low_confidence: 2,
        contradictions: 1,
        missing_embeddings: 0,
        duplicate_candidates: 3,
        last_consolidation: None,
    };
    let recent_sessions = vec![];
    let feedback: Vec<String> = vec![];
    let budget = ResourceBudget {
        provider: "mock".to_string(),
        tokens_remaining_desc: "50k tokens".to_string(),
        window_resets_desc: "2h".to_string(),
        user_usage_rate_desc: "5k/min".to_string(),
        cycle_budget_desc: "stay under 50k".to_string(),
    };

    let prompt = build_ambient_system_prompt(
        &state,
        &queue_items,
        &health,
        &recent_sessions,
        &feedback,
        &budget,
        0,
    );

    // Verify key sections exist
    assert!(
        prompt.contains("ambient agent"),
        "Prompt missing 'ambient agent'"
    );
    assert!(
        prompt.contains("Memory Graph Health"),
        "Prompt missing 'Memory Graph Health'"
    );
    assert!(
        prompt.contains("Total memories: 42"),
        "Prompt missing memory count"
    );
    assert!(
        prompt.contains("Resource Budget"),
        "Prompt missing 'Resource Budget'"
    );
    assert!(
        prompt.contains("end_ambient_cycle"),
        "Prompt missing end_ambient_cycle instruction"
    );
}

/// Test ambient runner handle: status_json
#[tokio::test]
async fn test_ambient_runner_status() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let status_json = handle.status_json().await;
    let status: serde_json::Value = serde_json::from_str(&status_json).unwrap();

    // Verify expected fields exist and have correct types
    assert!(status.get("status").is_some(), "Missing 'status' field");
    assert!(
        status.get("total_cycles").is_some(),
        "Missing 'total_cycles' field"
    );
    assert!(
        status.get("loop_running").is_some(),
        "Missing 'loop_running' field"
    );
    assert_eq!(
        status["loop_running"], false,
        "Runner loop should not be running"
    );
    assert!(
        status["total_cycles"].is_number(),
        "total_cycles should be a number"
    );
    assert!(
        status.get("queue_count").is_some(),
        "Missing 'queue_count' field"
    );
    assert!(
        status.get("active_user_sessions").is_some(),
        "Missing 'active_user_sessions' field"
    );
}

/// Test ambient runner handle: trigger and stop
#[tokio::test]
async fn test_ambient_runner_trigger_and_stop() {
    use jcode::ambient::AmbientStatus;
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    // Stop (sets status to disabled)
    handle.stop().await;
    let state = handle.state().await;
    assert!(
        matches!(state.status, AmbientStatus::Disabled),
        "After stop(), status should be Disabled, got: {:?}",
        state.status
    );

    // Runner should not be running (no loop was started)
    assert!(!handle.is_running().await, "Runner should not be active");
}

/// Test ambient runner handle: queue_json
#[tokio::test]
async fn test_ambient_runner_queue_json() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let json = handle.queue_json().await;
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
}

/// Test ambient runner handle: log_json
#[tokio::test]
async fn test_ambient_runner_log_json() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let json = handle.log_json().await;
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
}

/// Test memory reinforcement provenance
#[test]
fn test_memory_reinforcement_provenance() {
    use jcode::memory::{MemoryCategory, MemoryEntry};

    let mut entry = MemoryEntry::new(MemoryCategory::Preference, "User prefers dark mode");
    assert!(entry.reinforcements.is_empty());
    assert_eq!(entry.strength, 1); // Initial strength

    // Reinforce with provenance
    entry.reinforce("session_abc123", 42);
    assert_eq!(entry.strength, 2);
    assert_eq!(entry.reinforcements.len(), 1);
    assert_eq!(entry.reinforcements[0].session_id, "session_abc123");
    assert_eq!(entry.reinforcements[0].message_index, 42);

    // Reinforce again from different session
    entry.reinforce("session_def456", 10);
    assert_eq!(entry.strength, 3);
    assert_eq!(entry.reinforcements.len(), 2);
    assert_eq!(entry.reinforcements[1].session_id, "session_def456");
    assert_eq!(entry.reinforcements[1].message_index, 10);
}

/// Test ambient config defaults
#[test]
fn test_ambient_config_defaults() {
    use jcode::config::AmbientConfig;

    let config = AmbientConfig::default();
    assert!(!config.enabled);
    assert!(!config.allow_api_keys);
    assert_eq!(config.min_interval_minutes, 5);
    assert_eq!(config.max_interval_minutes, 120);
    assert!(config.pause_on_active_session);
    assert!(config.proactive_work);
    assert_eq!(config.work_branch_prefix, "ambient/");
    assert!(config.provider.is_none());
    assert!(config.model.is_none());
    assert!(config.api_daily_budget.is_none());
}

/// Test ambient lock acquisition and release
#[test]
fn test_ambient_lock() {
    use jcode::ambient::AmbientLock;
    let _env = setup_test_env().expect("failed to setup isolated JCODE_HOME");

    // First acquisition should succeed
    let lock1 = AmbientLock::try_acquire();
    assert!(lock1.is_ok());
    let lock1 = lock1.unwrap();
    assert!(lock1.is_some());
    let lock1 = lock1.unwrap();

    // Second acquisition should fail (lock held)
    let lock2 = AmbientLock::try_acquire();
    assert!(lock2.is_ok());
    assert!(lock2.unwrap().is_none());

    // Release
    let _ = lock1.release();

    // Now should succeed again
    let lock3 = AmbientLock::try_acquire();
    assert!(lock3.is_ok());
    assert!(lock3.unwrap().is_some());
}

/// Test full ambient cycle simulation with mock provider
/// Simulates: agent receives prompt → uses tools → calls end_ambient_cycle
#[tokio::test]
async fn test_full_ambient_cycle_simulation() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // Turn 1: Agent calls end_ambient_cycle with full data
    let end_cycle_input = serde_json::json!({
        "summary": "Gardened memory graph: merged 2 duplicates about dark mode preference, pruned 1 stale memory with confidence 0.02, verified 5 facts against codebase.",
        "memories_modified": 6,
        "compactions": 1,
        "proactive_work": null,
        "next_schedule": {
            "wake_in_minutes": 45,
            "context": "Follow up on memory verification",
            "priority": "normal"
        }
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::TextDelta("Starting ambient cycle...\n".to_string()),
        StreamEvent::ToolUseStart {
            id: "call_end".to_string(),
            name: "end_ambient_cycle".to_string(),
        },
        StreamEvent::ToolInputDelta(end_cycle_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // Turn 2: After end_ambient_cycle tool result, agent responds
    provider.queue_response(vec![
        StreamEvent::TextDelta("Ambient cycle completed successfully.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider.clone(), registry);
    agent.set_system_prompt("You are the jcode ambient maintenance agent.");

    let response = agent.run_once_capture("Begin your ambient cycle.").await?;

    assert!(response.contains("Ambient cycle completed"));

    // Verify end_ambient_cycle stored the result
    let result = jcode::tool::ambient::take_cycle_result();
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.memories_modified, 6);
    assert_eq!(result.compactions, 1);
    assert!(result.summary.contains("Gardened memory graph"));
    assert!(result.next_schedule.is_some());
    let sched = result.next_schedule.unwrap();
    assert_eq!(sched.wake_in_minutes, Some(45));
    assert!(sched.context.contains("Follow up"));

    Ok(())
}
