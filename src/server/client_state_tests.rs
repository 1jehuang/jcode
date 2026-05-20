use super::handle_get_history;
use super::session_activity_snapshot;
use crate::agent::Agent;
use crate::message::{Message, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::server::ClientConnectionInfo;
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::io::BufRead as _;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, RwLock, mpsc};

struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Err(anyhow::anyhow!(
            "mock provider complete should not be called in client_state tests"
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }

    fn model(&self) -> String {
        "mock-model".to_string()
    }
}

#[tokio::test]
async fn session_activity_snapshot_prefers_live_tool_name_for_target_session() {
    let now = Instant::now();
    let client_connections = Arc::new(RwLock::new(HashMap::from([
        (
            "conn-idle".to_string(),
            ClientConnectionInfo {
                client_id: "conn-idle".to_string(),
                session_id: "other-session".to_string(),
                client_instance_id: None,
                debug_client_id: None,
                connected_at: now,
                last_seen: now,
                is_processing: true,
                current_tool_name: Some("bash".to_string()),
                disconnect_tx: mpsc::unbounded_channel().0,
            },
        ),
        (
            "conn-target".to_string(),
            ClientConnectionInfo {
                client_id: "conn-target".to_string(),
                session_id: "target-session".to_string(),
                client_instance_id: None,
                debug_client_id: None,
                connected_at: now,
                last_seen: now,
                is_processing: true,
                current_tool_name: Some("batch".to_string()),
                disconnect_tx: mpsc::unbounded_channel().0,
            },
        ),
    ])));

    let snapshot = session_activity_snapshot(&client_connections, "target-session", false)
        .await
        .expect("activity snapshot");

    assert!(snapshot.is_processing);
    assert_eq!(snapshot.current_tool_name.as_deref(), Some("batch"));
}

#[tokio::test]
async fn session_activity_snapshot_uses_fallback_when_no_live_connection_is_marked_busy() {
    let client_connections = Arc::new(RwLock::new(HashMap::<String, ClientConnectionInfo>::new()));

    let snapshot = session_activity_snapshot(&client_connections, "target-session", true)
        .await
        .expect("fallback snapshot");

    assert!(snapshot.is_processing);
    assert_eq!(snapshot.current_tool_name, None);
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "test intentionally keeps the agent busy lock held to exercise persisted-history fallback"
)]
async fn handle_get_history_falls_back_to_persisted_snapshot_when_agent_is_busy() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_busy_history_fallback";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("busy fallback".to_string()),
    );
    session.model = Some("mock-model".to_string());
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg-busy-fallback".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "persisted fallback history".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save session");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::empty();
    let mut live_session = session.clone();
    live_session.title = Some("live agent".to_string());
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider.clone(),
        registry,
        live_session,
        None,
    )));
    let busy_guard = agent.lock().await;

    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.to_string(),
        Arc::clone(&agent),
    )])));
    let client_connections = Arc::new(RwLock::new(HashMap::<String, ClientConnectionInfo>::new()));
    let client_count = Arc::new(RwLock::new(1usize));

    let (stream_a, mut stream_b) = crate::transport::stream_pair().expect("stream pair");
    let (_reader_a, writer_a) = stream_a.into_split();
    let writer = Arc::new(Mutex::new(writer_a));

    handle_get_history(
        42,
        session_id,
        true,
        &agent,
        &provider,
        &sessions,
        &client_connections,
        &client_count,
        &writer,
        "server-name",
        "🔥",
        None,
    )
    .await
    .expect("history should be written from persisted fallback");

    drop(busy_guard);
    drop(writer);

    let mut bytes = Vec::new();
    stream_b
        .read_to_end(&mut bytes)
        .await
        .expect("read history event bytes");
    let mut cursor = std::io::Cursor::new(bytes);
    let mut line = String::new();
    cursor.read_line(&mut line).expect("read first line");
    let event: crate::protocol::ServerEvent =
        serde_json::from_str(line.trim()).expect("decode history event");

    match event {
        crate::protocol::ServerEvent::History {
            id,
            session_id: returned_session_id,
            messages,
            activity,
            ..
        } => {
            assert_eq!(id, 42);
            assert_eq!(returned_session_id, session_id);
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].content, "persisted fallback history");
            let activity = activity.expect("fallback activity snapshot");
            assert!(activity.is_processing);
        }
        other => panic!("expected history event, got {:?}", other),
    }

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

struct ReloadHistoryEnvGuard {
    prev_home: Option<std::ffi::OsString>,
    prev_runtime: Option<std::ffi::OsString>,
}

impl ReloadHistoryEnvGuard {
    fn new(home: &std::path::Path, runtime: &std::path::Path) -> Self {
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_HOME", home);
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime);
        Self {
            prev_home,
            prev_runtime,
        }
    }
}

impl Drop for ReloadHistoryEnvGuard {
    fn drop(&mut self) {
        crate::server::clear_reload_marker();
        if let Some(prev_home) = self.prev_home.take() {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        if let Some(prev_runtime) = self.prev_runtime.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

fn write_pending_user_session(
    session_id: &str,
    status: crate::session::SessionStatus,
) -> Result<()> {
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.status = status;
    session.add_message(
        crate::message::Role::User,
        vec![crate::message::ContentBlock::Text {
            text: "continue this after reload".to_string(),
            cache_control: None,
        }],
    );
    session.save()
}

#[test]
fn history_reload_recovery_infers_pending_active_user_turn_during_reload() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new()?;
    let runtime = tempfile::TempDir::new()?;
    let _guard = ReloadHistoryEnvGuard::new(home.path(), runtime.path());
    let session_id = "session_history_reload_fallback";
    write_pending_user_session(session_id, crate::session::SessionStatus::Active)?;
    crate::server::write_reload_state(
        "reload-history-fallback",
        "test-hash",
        crate::server::ReloadPhase::SocketReady,
        Some(session_id.to_string()),
    );

    let snapshot = super::history_reload_recovery_snapshot(session_id, None);
    assert!(
        snapshot.is_some(),
        "pending user turn during reload should get recovery directive"
    );
    let Some(snapshot) = snapshot else {
        return Ok(());
    };

    assert!(
        snapshot
            .continuation_message
            .contains("interrupted by a server reload")
    );
    Ok(())
}

#[test]
fn history_reload_recovery_does_not_infer_pending_user_turn_without_reload_marker() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new()?;
    let runtime = tempfile::TempDir::new()?;
    let _guard = ReloadHistoryEnvGuard::new(home.path(), runtime.path());
    let session_id = "session_history_no_reload_fallback";
    write_pending_user_session(session_id, crate::session::SessionStatus::Active)?;

    assert!(super::history_reload_recovery_snapshot(session_id, None).is_none());
    Ok(())
}

#[test]
fn history_reload_recovery_prefers_server_owned_intent_and_marks_delivered() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::TempDir::new()?;
    let runtime = tempfile::TempDir::new()?;
    let _guard = ReloadHistoryEnvGuard::new(home.path(), runtime.path());
    let session_id = "session_history_store_owned";
    super::super::reload_recovery::persist_intent(
        "reload-store-owned",
        session_id,
        super::super::reload_recovery::ReloadRecoveryRole::InterruptedPeer,
        crate::tool::selfdev::ReloadRecoveryDirective {
            reconnect_notice: Some("stored notice".to_string()),
            continuation_message: "stored continuation".to_string(),
        },
        "test store intent",
    )?;

    let Some(snapshot) = super::history_reload_recovery_snapshot(session_id, None) else {
        anyhow::bail!("server-owned recovery intent should be used");
    };
    assert_eq!(snapshot.continuation_message, "stored continuation");

    assert!(
        super::history_reload_recovery_snapshot(session_id, None).is_none(),
        "delivered server-owned recovery intent should not be emitted twice"
    );
    Ok(())
}
