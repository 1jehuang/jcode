//! Per-session accounts over a real socket: two sessions on one server, a
//! fake provider that records `set_account_pin`, and the stored auth.json.

#![cfg_attr(test, allow(clippy::await_holding_lock))]
use super::*;
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::provider::{AccountPin, AccountProviderKind, EventStream, Provider};
use async_trait::async_trait;
use futures::stream;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Provider fork that keeps its own pin (like the real runtimes) and records
/// every `set_account_pin` call in a log shared by all forks.
#[derive(Clone)]
struct PinRecordingProvider {
    pin: Arc<StdMutex<BTreeMap<AccountProviderKind, AccountPin>>>,
    calls: Arc<StdMutex<Vec<(AccountProviderKind, Option<String>)>>>,
}

use std::collections::BTreeMap;

impl PinRecordingProvider {
    fn new() -> Self {
        Self {
            pin: Arc::new(StdMutex::new(BTreeMap::new())),
            calls: Arc::new(StdMutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl Provider for PinRecordingProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta("ok".into())),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }
    fn name(&self) -> &str {
        "pin-recording"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            pin: Arc::new(StdMutex::new(self.pin.lock().unwrap().clone())),
            calls: Arc::clone(&self.calls),
        })
    }
    fn fork_for_new_session(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            pin: Arc::new(StdMutex::new(BTreeMap::new())),
            calls: Arc::clone(&self.calls),
        })
    }
    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        self.pin.lock().unwrap().get(&kind).cloned()
    }
    fn set_account_pin(&self, kind: AccountProviderKind, pin: Option<AccountPin>) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((kind, pin.as_ref().map(|pin| pin.label.clone())));
        let mut pins = self.pin.lock().unwrap();
        match pin {
            Some(pin) => {
                pins.insert(kind, pin);
            }
            None => {
                pins.remove(&kind);
            }
        }
        Ok(())
    }
}

fn claude_account(label: &str, email: &str) -> crate::auth::claude::AnthropicAccount {
    crate::auth::claude::AnthropicAccount {
        label: label.to_string(),
        access: format!("access-{label}"),
        refresh: format!("refresh-{label}"),
        expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
        email: Some(email.to_string()),
        subscription_type: Some("max".to_string()),
        scopes: Vec::new(),
    }
}

/// Three stored Claude accounts; otter (the first) is the default.
fn store_three_claude_accounts() {
    for (label, email) in [
        ("claude-otter", "otter@example.com"),
        ("claude-fox", "fox@example.com"),
        ("claude-panda", "panda@example.com"),
    ] {
        crate::auth::claude::upsert_account(claude_account(label, email)).expect("store account");
    }
    assert_eq!(stored_claude_default().as_deref(), Some("claude-otter"));
}

fn stored_claude_default() -> Option<String> {
    crate::auth::claude::load_auth_file()
        .expect("auth file")
        .active_anthropic_account
}

struct TestServer {
    sessions: SessionAgents,
    connect: Box<
        dyn Fn() -> (
            tokio::task::JoinHandle<Result<()>>,
            crate::transport::Stream,
        ),
    >,
}

fn test_server(provider_template: Arc<dyn Provider>) -> TestServer {
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::new()));
    let global_session_id = Arc::new(RwLock::new(String::new()));
    let client_count = Arc::new(RwLock::new(0usize));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let shared_context = Arc::new(RwLock::new(HashMap::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let file_touch = FileTouchService::new();
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::new()));
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
    let (debug_response_tx, _) = broadcast::channel(8);
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let (global_event_tx, _) = broadcast::channel(8);
    let global_is_processing = Arc::new(RwLock::new(false));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());
    let sessions_for_connect = Arc::clone(&sessions);
    let connect = move || {
        let (server_stream, client_stream) = crate::transport::Stream::pair().expect("socket pair");
        let task = tokio::spawn(handle_client(
            server_stream,
            Arc::clone(&sessions_for_connect),
            global_event_tx.clone(),
            provider_template.clone(),
            global_is_processing.clone(),
            global_session_id.clone(),
            client_count.clone(),
            Arc::clone(&client_connections),
            swarm_members.clone(),
            swarms_by_id.clone(),
            shared_context.clone(),
            swarm_plans.clone(),
            swarm_coordinators.clone(),
            file_touch.clone(),
            channel_subscriptions.clone(),
            channel_subscriptions_by_session.clone(),
            client_debug_state.clone(),
            debug_response_tx.clone(),
            event_history.clone(),
            event_counter.clone(),
            swarm_event_tx.clone(),
            "jcode-test".to_string(),
            "🧪".to_string(),
            mcp_pool.clone(),
            shutdown_signals.clone(),
            soft_interrupt_queues.clone(),
            AwaitMembersRuntime::default(),
            SwarmMutationRuntime::default(),
        ));
        (task, client_stream)
    };
    TestServer {
        sessions,
        connect: Box::new(connect),
    }
}

struct TestClient {
    reader: BufReader<crate::transport::ReadHalf>,
    writer: crate::transport::WriteHalf,
    events: Vec<ServerEvent>,
}

impl TestClient {
    fn new(stream: crate::transport::Stream) -> Self {
        let (reader, writer) = stream.into_split();
        Self {
            reader: BufReader::new(reader),
            writer,
            events: Vec::new(),
        }
    }

    async fn send(&mut self, value: serde_json::Value) {
        self.writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }

    /// Read until `predicate` matches. Every event read is kept in `events`.
    async fn until(&mut self, predicate: impl Fn(&ServerEvent) -> bool) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                assert!(self.reader.read_line(&mut line).await.unwrap() > 0);
                let event: ServerEvent = serde_json::from_str(&line).unwrap();
                self.events.push(event.clone());
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("daemon event timeout; events so far: {:?}", self.events))
    }

    async fn subscribe(
        &mut self,
        id: u64,
        dir: &std::path::Path,
        supports_accounts: bool,
    ) -> String {
        self.send(serde_json::json!({
            "type": "subscribe",
            "id": id,
            "working_dir": dir,
            "supports_session_accounts": supports_accounts,
        }))
        .await;
        self.until(|e| matches!(e, ServerEvent::Done { id: done } if *done == id))
            .await;
        self.events
            .iter()
            .rev()
            .find_map(|event| match event {
                ServerEvent::SessionId { session_id } => Some(session_id.clone()),
                _ => None,
            })
            .expect("subscribe announces the session id")
    }
}

async fn session_pin(server: &TestServer, session_id: &str) -> Option<String> {
    let agent = server
        .sessions
        .read()
        .await
        .get(session_id)
        .cloned()
        .expect("live session");
    let guard = agent.lock().await;
    guard
        .provider_handle()
        .account_pin(AccountProviderKind::Claude)
        .map(|pin| pin.label)
}

#[tokio::test]
async fn set_session_account_does_not_change_other_sessions_or_default() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    let runtime = tempfile::TempDir::new().expect("runtime");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    store_three_claude_accounts();
    let provider = PinRecordingProvider::new();
    let server = test_server(Arc::new(provider.clone()));

    let (_t1, s1) = (server.connect)();
    let (_t2, s2) = (server.connect)();
    let mut c1 = TestClient::new(s1);
    let mut c2 = TestClient::new(s2);
    let id1 = c1.subscribe(10, sandbox.root(), true).await;
    let id2 = c2.subscribe(20, sandbox.root(), true).await;
    assert_ne!(id1, id2);

    c1.send(serde_json::json!({
        "type": "set_session_account", "id": 30, "provider": "claude", "label": "claude-fox"
    }))
    .await;
    let changed = c1
        .until(|e| matches!(e, ServerEvent::SessionAccountChanged { .. }))
        .await;
    assert!(matches!(
        &changed,
        ServerEvent::SessionAccountChanged { provider, label: Some(label), pinned: true, is_default: false, .. }
            if provider == "claude" && label == "claude-fox"
    ));
    c1.until(|e| matches!(e, ServerEvent::Done { id: 30 }))
        .await;

    assert_eq!(
        session_pin(&server, &id1).await.as_deref(),
        Some("claude-fox")
    );
    assert_eq!(
        session_pin(&server, &id2).await,
        None,
        "S2 must stay unpinned"
    );
    assert_eq!(
        stored_claude_default().as_deref(),
        Some("claude-otter"),
        "a per-window switch must not rewrite the stored default"
    );
    assert_eq!(crate::auth::claude::get_active_account_override(), None);
    assert_eq!(
        crate::auth::claude::active_account_label().as_deref(),
        Some("claude-otter")
    );
    assert_eq!(
        provider.calls.lock().unwrap().as_slice(),
        &[(AccountProviderKind::Claude, Some("claude-fox".to_string()))],
        "exactly one pin call, on S1's provider"
    );
    // The pin is persisted with an identity, so a later relabel cannot move
    // it. A session reaches disk with its first prompt.
    c1.send(serde_json::json!({"type": "message", "id": 32, "content": "hello"}))
        .await;
    c1.until(|e| matches!(e, ServerEvent::Done { id: 32 }))
        .await;
    let saved = crate::session::Session::load(&id1).expect("S1 persisted");
    assert_eq!(
        saved.account_pins.get("claude"),
        Some(&AccountPin::new(
            "claude-fox",
            Some("fox@example.com".to_string())
        ))
    );

    // Unpin S1: it follows the default again.
    c1.send(serde_json::json!({"type": "set_session_account", "id": 31, "provider": "claude"}))
        .await;
    c1.until(|e| matches!(e, ServerEvent::Done { id: 31 }))
        .await;
    assert_eq!(session_pin(&server, &id1).await, None);
    assert!(
        crate::session::Session::load(&id1)
            .expect("S1 persisted")
            .account_pins
            .is_empty()
    );
    crate::env::remove_var("JCODE_RUNTIME_DIR");
}

#[tokio::test]
async fn legacy_switch_request_pins_sender_only_and_keeps_default() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    let runtime = tempfile::TempDir::new().expect("runtime");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    store_three_claude_accounts();
    let provider = PinRecordingProvider::new();
    let server = test_server(Arc::new(provider.clone()));

    // An old client: no session-accounts capability.
    let (_t1, s1) = (server.connect)();
    let (_t2, s2) = (server.connect)();
    let mut c1 = TestClient::new(s1);
    let mut c2 = TestClient::new(s2);
    let id1 = c1.subscribe(10, sandbox.root(), false).await;
    let id2 = c2.subscribe(20, sandbox.root(), false).await;

    c1.send(serde_json::json!({
        "type": "switch_anthropic_account", "id": 40, "label": "claude-panda"
    }))
    .await;
    c1.until(|e| matches!(e, ServerEvent::Done { id: 40 }))
        .await;
    assert!(
        !c1.events
            .iter()
            .any(|e| matches!(e, ServerEvent::Error { .. })),
        "legacy switch failed: {:?}",
        c1.events
    );
    assert!(
        !c1.events
            .iter()
            .any(|e| matches!(e, ServerEvent::SessionAccountChanged { .. })),
        "an old client must not receive session_account_changed"
    );

    assert_eq!(
        session_pin(&server, &id1).await.as_deref(),
        Some("claude-panda")
    );
    assert_eq!(
        session_pin(&server, &id2).await,
        None,
        "other session stays on the default"
    );
    let auth = crate::auth::claude::load_auth_file().expect("auth");
    assert_eq!(
        auth.active_anthropic_account.as_deref(),
        Some("claude-otter"),
        "auth.json active_anthropic_account must be unchanged"
    );
    assert_eq!(crate::auth::claude::get_active_account_override(), None);

    // `set_default_account` is the only request that moves the default, and
    // it does not repin anyone.
    c2.send(serde_json::json!({
        "type": "set_default_account", "id": 50, "provider": "claude", "label": "claude-fox"
    }))
    .await;
    c2.until(|e| matches!(e, ServerEvent::Done { id: 50 }))
        .await;
    assert_eq!(stored_claude_default().as_deref(), Some("claude-fox"));
    assert_eq!(crate::auth::claude::get_active_account_override(), None);
    assert_eq!(
        session_pin(&server, &id1).await.as_deref(),
        Some("claude-panda")
    );
    assert_eq!(session_pin(&server, &id2).await, None);
    crate::env::remove_var("JCODE_RUNTIME_DIR");
}

#[tokio::test]
async fn subscribe_account_pins_pin_the_new_session_and_history_reports_it() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    let runtime = tempfile::TempDir::new().expect("runtime");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    store_three_claude_accounts();
    let server = test_server(Arc::new(PinRecordingProvider::new()));

    let (_t1, s1) = (server.connect)();
    let mut c1 = TestClient::new(s1);
    c1.send(serde_json::json!({
        "type": "subscribe", "id": 10, "working_dir": sandbox.root(),
        "supports_session_accounts": true,
        "account_pins": [["claude", "claude-fox"]],
    }))
    .await;
    c1.until(|e| matches!(e, ServerEvent::Done { id: 10 }))
        .await;
    c1.send(serde_json::json!({"type": "get_history", "id": 11}))
        .await;
    let ServerEvent::History {
        session_id,
        account_labels,
        ..
    } = c1
        .until(|e| matches!(e, ServerEvent::History { id: 11, .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        session_pin(&server, &session_id).await.as_deref(),
        Some("claude-fox")
    );
    assert_eq!(
        account_labels,
        vec![crate::protocol::SessionAccountInfo {
            provider: "claude".to_string(),
            label: Some("claude-fox".to_string()),
            pinned: true,
            is_default: false,
        }]
    );
    crate::env::remove_var("JCODE_RUNTIME_DIR");
}
