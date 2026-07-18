#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::{
    FileAccess, Server, SessionInterruptQueues, SwarmMember, dispatch_background_task_completion,
    file_activity_scope_label, persist_swarm_state_snapshot, remove_session_entry,
};
use crate::agent::Agent;
use crate::bus::{
    BackgroundTaskCompleted, BackgroundTaskProgress, BackgroundTaskProgressEvent,
    BackgroundTaskProgressKind, BackgroundTaskProgressSource, BackgroundTaskStatus, FileOp,
    FileTouch,
};
use crate::message::{Message, Role, StreamEvent, ToolDefinition};
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use crate::tool::selfdev::ReloadContext;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tokio::time::timeout;

struct EnvGuard {
    prev_home: Option<OsString>,
    prev_runtime_dir: Option<OsString>,
    prev_socket: Option<OsString>,
}

struct ScopedEnvVar {
    key: &'static str,
    prev: Option<OsString>,
}

fn file_access_with_summary(summary: Option<&str>) -> FileAccess {
    FileAccess {
        session_id: "session-peer".to_string(),
        op: FileOp::Edit,
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
        intent: None,
        summary: summary.map(str::to_string),
        detail: None,
    }
}

fn file_touch_with_summary(summary: Option<&str>) -> FileTouch {
    FileTouch {
        session_id: "session-current".to_string(),
        path: std::path::PathBuf::from("src/lib.rs"),
        op: FileOp::Edit,
        intent: None,
        summary: summary.map(str::to_string),
        detail: None,
    }
}

#[test]
fn file_activity_scope_label_classifies_overlap() {
    let previous = file_access_with_summary(Some("edited lines 10-20"));
    let current = file_touch_with_summary(Some("edited lines 18-25"));
    assert_eq!(
        file_activity_scope_label(&previous, &current),
        "overlapping lines"
    );

    let current = file_touch_with_summary(Some("edited lines 30-40"));
    assert_eq!(
        file_activity_scope_label(&previous, &current),
        "same file, non-overlapping lines"
    );

    let current = file_touch_with_summary(None);
    assert_eq!(file_activity_scope_label(&previous, &current), "same file");
}

#[tokio::test]
async fn removing_server_session_clears_active_pid_marker() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create temporary JCODE_HOME");
    let _home_guard = ScopedEnvVar::set("JCODE_HOME", home.path());
    let session_id = "session_marker_cleanup_test";
    crate::storage::register_active_pid(session_id, std::process::id());

    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.to_string(),
        "agent",
    )])));
    let removed = remove_session_entry(&sessions, session_id).await;

    assert_eq!(removed, Some("agent"));
    assert!(!sessions.read().await.contains_key(session_id));
    assert!(
        !crate::storage::active_session_ids()
            .iter()
            .any(|id| id == session_id),
        "removing a live server session must also remove its presence marker"
    );
}

#[test]
fn configured_server_name_normalizes_operator_labels() {
    assert_eq!(
        super::normalize_configured_server_name(" Mount Cloud/Fabian ").as_deref(),
        Some("mount-cloud-fabian")
    );
    assert_eq!(
        super::normalize_configured_server_name("john@example.com").as_deref(),
        Some("john-example.com")
    );
    assert_eq!(super::normalize_configured_server_name(" 🫥 "), None);
}

#[test]
fn server_identity_uses_configured_name() {
    let _guard = crate::storage::lock_test_env();
    let _server_name_guard = ScopedEnvVar::set("JCODE_SERVER_NAME", "env-name");
    let _server_display_name_guard = ScopedEnvVar::set("JCODE_SERVER_DISPLAY_NAME", "display-name");

    let server = Server::new_with_name(
        Arc::new(StreamingMockProvider::default()),
        Some("Mount Cloud/Fabian".to_string()),
    );

    assert_eq!(server.identity().name, "mount-cloud-fabian");
    assert!(
        server
            .identity()
            .id
            .starts_with("server_mount-cloud-fabian_")
    );
}

#[test]
fn server_identity_reads_name_from_env() {
    let _guard = crate::storage::lock_test_env();
    let _server_name_guard = ScopedEnvVar::set("JCODE_SERVER_NAME", "mount-cloud/john");
    let _server_display_name_guard = ScopedEnvVar::set("JCODE_SERVER_DISPLAY_NAME", "ignored");

    let server = Server::new_with_name(Arc::new(StreamingMockProvider::default()), None);

    assert_eq!(server.identity().name, "mount-cloud-john");
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.prev_home {
            crate::env::set_var("JCODE_HOME", value);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        if let Some(value) = &self.prev_runtime_dir {
            crate::env::set_var("JCODE_RUNTIME_DIR", value);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
        if let Some(value) = &self.prev_socket {
            crate::env::set_var("JCODE_SOCKET", value);
        } else {
            crate::env::remove_var("JCODE_SOCKET");
        }
    }
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(value) = &self.prev {
            crate::env::set_var(self.key, value);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

fn configure_test_env(root: &tempfile::TempDir) -> EnvGuard {
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_runtime_dir = std::env::var_os("JCODE_RUNTIME_DIR");
    let prev_socket = std::env::var_os("JCODE_SOCKET");
    let home_dir = root.path().join("home");
    let runtime_dir = root.path().join("runtime");
    std::fs::create_dir_all(&home_dir).expect("create home dir");
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");
    crate::env::set_var("JCODE_HOME", &home_dir);
    crate::env::set_var("JCODE_RUNTIME_DIR", &runtime_dir);
    crate::env::remove_var("JCODE_SOCKET");
    EnvGuard {
        prev_home,
        prev_runtime_dir,
        prev_socket,
    }
}

#[derive(Default, Clone)]
struct StreamingMockProvider {
    responses: Arc<StdMutex<Vec<Vec<StreamEvent>>>>,
}

impl StreamingMockProvider {
    fn queue_response(&self, response: Vec<StreamEvent>) {
        self.responses
            .lock()
            .expect("streaming mock response queue lock")
            .push(response);
    }
}

#[async_trait]
impl Provider for StreamingMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let response = self
            .responses
            .lock()
            .expect("streaming mock response queue lock")
            .remove(0);
        Ok(Box::pin(tokio_stream::iter(response.into_iter().map(Ok))))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn test_agent(provider: Arc<dyn Provider>) -> Arc<Mutex<Agent>> {
    let registry = Registry::new(provider.clone()).await;
    Arc::new(Mutex::new(Agent::new(provider, registry)))
}

#[allow(clippy::type_complexity)]
fn empty_swarm_status_state() -> (
    Arc<RwLock<HashMap<String, std::collections::HashSet<String>>>>,
    Arc<RwLock<std::collections::VecDeque<super::SwarmEvent>>>,
    Arc<std::sync::atomic::AtomicU64>,
    broadcast::Sender<super::SwarmEvent>,
) {
    let (swarm_event_tx, _) = broadcast::channel(16);
    (
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(std::collections::VecDeque::new())),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        swarm_event_tx,
    )
}

fn attached_swarm_member(
    session_id: &str,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
) -> SwarmMember {
    SwarmMember {
        session_id: session_id.to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: None,
        swarm_enabled: false,
        status: "ready".to_string(),
        detail: None,
        friendly_name: Some("otter".to_string()),
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    }
}

fn persisted_headless_member(
    session_id: &str,
    swarm_id: &str,
    status: &str,
    detail: &str,
) -> SwarmMember {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    SwarmMember {
        session_id: session_id.to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some(swarm_id.to_string()),
        swarm_enabled: true,
        status: status.to_string(),
        detail: Some(detail.to_string()),
        friendly_name: Some(session_id.to_string()),
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    }
}

#[tokio::test]
async fn background_task_wake_runs_live_session_immediately_when_idle() {
    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![
        StreamEvent::TextDelta("Build result processed.".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    let agent = test_agent(provider_dyn).await;
    let session_id = agent.lock().await.session_id().to_string();
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        agent.clone(),
    )])));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        attached_swarm_member(&session_id, member_event_tx),
    )])));
    let task = BackgroundTaskCompleted {
        task_id: "bgwake".to_string(),
        tool_name: "selfdev-build".to_string(),
        display_name: None,
        session_id: session_id.clone(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "done\n".to_string(),
        output_file: std::env::temp_dir().join("bgwake.output"),
        duration_secs: 1.4,
        notify: true,
        wake: true,
    };

    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    dispatch_background_task_completion(
        &task,
        &sessions,
        &soft_interrupt_queues,
        &swarm_members,
        &swarms_by_id,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    )
    .await;

    let notification = timeout(Duration::from_secs(2), async {
        loop {
            match member_event_rx.recv().await {
                Some(ServerEvent::Notification {
                    notification_type,
                    message,
                    ..
                }) => return (notification_type, message),
                Some(_) => continue,
                None => panic!("member stream closed before notification"),
            }
        }
    })
    .await
    .expect("background task notification should arrive promptly");

    match notification.0 {
        NotificationType::Message { scope, channel, .. } => {
            assert_eq!(scope.as_deref(), Some("background_task"));
            assert!(channel.is_none());
        }
        other => panic!("unexpected notification type: {other:?}"),
    }
    assert!(notification.1.contains("**Background task** `bgwake`"));

    let streamed = timeout(Duration::from_secs(2), async {
        loop {
            match member_event_rx.recv().await {
                Some(ServerEvent::TextDelta { text })
                    if text.contains("Build result processed.") =>
                {
                    return text;
                }
                Some(_) => continue,
                None => panic!("member stream closed before wake ran"),
            }
        }
    })
    .await
    .expect("wake delivery should start streaming promptly");
    assert!(streamed.contains("Build result processed."));

    let guard = agent.lock().await;
    assert!(guard.messages().iter().any(|message| {
        message.role == Role::User
            && message
                .content_preview()
                .contains("**Background task** `bgwake`")
    }));
}

#[tokio::test]
async fn wake_turn_tracks_member_status_and_emits_terminal_done() {
    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![
        StreamEvent::TextDelta("Wake turn finished.".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    let agent = test_agent(provider_dyn).await;
    let session_id = agent.lock().await.session_id().to_string();
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        agent.clone(),
    )])));
    let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
    let mut member = attached_swarm_member(&session_id, member_event_tx);
    member.swarm_id = Some("test-swarm".to_string());
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(session_id.clone(), member)])));
    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    {
        let mut swarms = swarms_by_id.write().await;
        swarms.insert(
            "test-swarm".to_string(),
            std::collections::HashSet::from([session_id.clone()]),
        );
    }

    let started = super::live_turn::run_live_turn_if_idle(
        &session_id,
        "DM from coordinator: please respond",
        Some("You received a direct swarm message.".to_string()),
        &sessions,
        super::live_turn::LiveTurnSwarmContext::new(
            &swarm_members,
            &swarms_by_id,
            &event_history,
            &event_counter,
            &swarm_event_tx,
        ),
    )
    .await;
    assert!(started, "idle live session should accept the wake turn");

    // Member status must flip to running while the wake turn streams.
    let observed_running = timeout(Duration::from_secs(2), async {
        loop {
            {
                let members = swarm_members.read().await;
                if members
                    .get(&session_id)
                    .is_some_and(|member| member.status == "running")
                {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or(false);

    // Attached clients must see a terminal Done event so the UI can settle.
    let saw_done = timeout(Duration::from_secs(2), async {
        loop {
            match member_event_rx.recv().await {
                Some(ServerEvent::Done { .. }) => return true,
                Some(_) => continue,
                None => return false,
            }
        }
    })
    .await
    .expect("wake turn should emit a terminal event promptly");
    assert!(saw_done, "wake turn must emit Done to attached clients");
    assert!(
        observed_running,
        "member status should be running while the wake turn streams"
    );

    // After completion the member returns to ready with a completion report.
    let (final_status, report) = timeout(Duration::from_secs(2), async {
        loop {
            {
                let members = swarm_members.read().await;
                if let Some(member) = members.get(&session_id)
                    && member.status == "ready"
                {
                    return (
                        member.status.clone(),
                        member.latest_completion_report.clone(),
                    );
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("member should return to ready after the wake turn");
    assert_eq!(final_status, "ready");
    assert!(
        report.is_some_and(|report| report.contains("Wake turn finished.")),
        "completion report should capture the wake turn's assistant text"
    );
}

#[tokio::test]
async fn background_task_notify_without_wake_does_not_queue_soft_interrupt() {
    let provider: Arc<dyn Provider> = Arc::new(StreamingMockProvider::default());
    let agent = test_agent(provider).await;
    let session_id = agent.lock().await.session_id().to_string();
    let queue = agent.lock().await.soft_interrupt_queue();
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        agent.clone(),
    )])));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        queue.clone(),
    )])));
    let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        attached_swarm_member(&session_id, member_event_tx),
    )])));
    let task = BackgroundTaskCompleted {
        task_id: "bgnotify".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: session_id.clone(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "ok\n".to_string(),
        output_file: std::env::temp_dir().join("bgnotify.output"),
        duration_secs: 0.7,
        notify: true,
        wake: false,
    };

    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    dispatch_background_task_completion(
        &task,
        &sessions,
        &soft_interrupt_queues,
        &swarm_members,
        &swarms_by_id,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    )
    .await;

    let notification = timeout(Duration::from_secs(2), member_event_rx.recv())
        .await
        .expect("background task notification should arrive promptly")
        .expect("member stream should stay open");
    match notification {
        ServerEvent::Notification { message, .. } => {
            assert!(message.contains("**Background task** `bgnotify`"));
        }
        other => panic!("expected notification, got {other:?}"),
    }

    let pending = queue.lock().expect("queue lock");
    assert!(
        pending.is_empty(),
        "notify-only delivery should not wake the session"
    );
}

#[tokio::test]
async fn background_task_progress_notifies_attached_clients() {
    let provider: Arc<dyn Provider> = Arc::new(StreamingMockProvider::default());
    let agent = test_agent(provider).await;
    let session_id = agent.lock().await.session_id().to_string();
    let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        attached_swarm_member(&session_id, member_event_tx),
    )])));
    let task = BackgroundTaskProgressEvent {
        task_id: "bgprogress".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: session_id.clone(),
        progress: BackgroundTaskProgress {
            kind: BackgroundTaskProgressKind::Determinate,
            percent: Some(42.0),
            message: Some("Running tests".to_string()),
            current: Some(21),
            total: Some(50),
            unit: Some("tests".to_string()),
            eta_seconds: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
            source: BackgroundTaskProgressSource::Reported,
        },
    };

    super::dispatch_background_task_progress(&task, &swarm_members).await;

    let notification = timeout(Duration::from_secs(2), member_event_rx.recv())
        .await
        .expect("background task progress notification should arrive promptly")
        .expect("member stream should stay open");
    match notification {
        ServerEvent::Notification {
            notification_type,
            message,
            ..
        } => {
            match notification_type {
                NotificationType::Message { scope, channel, .. } => {
                    assert_eq!(scope.as_deref(), Some("background_task"));
                    assert!(channel.is_none());
                }
                other => panic!("unexpected notification type: {other:?}"),
            }
            assert!(message.contains("**Background task progress** `bgprogress`"));
            assert!(message.contains("42%"), "message was: {message}");
        }
        other => panic!("expected notification, got {other:?}"),
    }
}

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "test intentionally serializes process-wide JCODE_HOME/env state across async recovery assertions"
)]
async fn startup_recovery_resumes_interrupted_headless_sessions_after_reload() -> Result<()> {
    let _storage_guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new()?;
    let _env = configure_test_env(&temp);

    let provider = Arc::new(StreamingMockProvider::default());
    for _ in 0..2 {
        provider.queue_response(vec![
            StreamEvent::TextDelta("continued after reload".to_string()),
            StreamEvent::MessageEnd { stop_reason: None },
        ]);
    }

    let mut initiator = crate::session::Session::create(None, Some("initiator".to_string()));
    initiator.set_canary("self-dev");
    initiator.add_message(
        Role::User,
        vec![crate::message::ContentBlock::ToolResult {
            tool_use_id: "tool_reload".to_string(),
            content: "Reload initiated. Process restarting...".to_string(),
            is_error: Some(false),
        }],
    );
    initiator.save()?;

    ReloadContext {
        task_context: Some("Verify multi-session reload recovery".to_string()),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: initiator.id.clone(),
        timestamp: "2026-04-19T00:00:00Z".to_string(),
    }
    .save()?;

    let mut peer = crate::session::Session::create(None, Some("peer".to_string()));
    peer.add_message(
        Role::User,
        vec![crate::message::ContentBlock::ToolResult {
            tool_use_id: "tool_bash".to_string(),
            content: "[Tool 'bash' interrupted by server reload after 0.2s]".to_string(),
            is_error: Some(true),
        }],
    );
    peer.save()?;

    let swarm_id = "swarm-reload-recovery";
    persist_swarm_state_snapshot(
        swarm_id,
        None,
        None,
        &[
            persisted_headless_member(&initiator.id, swarm_id, "running", "selfdev reload"),
            persisted_headless_member(&peer.id, swarm_id, "running", "bash tool"),
        ],
    );

    let server = Server::new(provider.clone());
    {
        let members = server.swarm_state.members.read().await;
        assert_eq!(
            members
                .get(&initiator.id)
                .map(|member| member.status.as_str()),
            Some("crashed"),
            "persisted running headless sessions should load as crashed before recovery"
        );
        assert_eq!(
            members.get(&peer.id).map(|member| member.status.as_str()),
            Some("crashed")
        );
    }

    server.recover_headless_sessions_on_startup().await;

    timeout(Duration::from_secs(5), async {
        loop {
            let sessions = server.sessions.read().await;
            let Some(initiator_agent) = sessions.get(&initiator.id).cloned() else {
                drop(sessions);
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            };
            let Some(peer_agent) = sessions.get(&peer.id).cloned() else {
                drop(sessions);
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            };
            drop(sessions);

            let initiator_done = {
                let guard = initiator_agent.lock().await;
                guard.messages().iter().any(|message| {
                    message.role == Role::Assistant
                        && message.content_preview().contains("continued after reload")
                })
            };
            let peer_done = {
                let guard = peer_agent.lock().await;
                guard.messages().iter().any(|message| {
                    message.role == Role::Assistant
                        && message.content_preview().contains("continued after reload")
                })
            };
            let statuses_ready = {
                let members = server.swarm_state.members.read().await;
                members
                    .get(&initiator.id)
                    .map(|member| member.status.as_str())
                    == Some("ready")
                    && members.get(&peer.id).map(|member| member.status.as_str()) == Some("ready")
            };

            if initiator_done && peer_done && statuses_ready {
                break;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("headless reload recovery should resume both sessions");

    assert!(
        ReloadContext::peek_for_session(&initiator.id)?.is_none(),
        "initiator reload context should be consumed by headless recovery"
    );

    Ok(())
}

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "test intentionally serializes process-wide JCODE_HOME/env state across async recovery assertions"
)]
async fn startup_recovery_preserves_headed_session_reload_context_for_later_reconnect() -> Result<()>
{
    let _storage_guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new()?;
    let _env = configure_test_env(&temp);

    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![
        StreamEvent::TextDelta("continued after reload".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);

    let mut headless = crate::session::Session::create(None, Some("headless".to_string()));
    headless.add_message(
        Role::User,
        vec![crate::message::ContentBlock::ToolResult {
            tool_use_id: "tool_bash".to_string(),
            content: "[Tool 'bash' interrupted by server reload after 0.2s]".to_string(),
            is_error: Some(true),
        }],
    );
    headless.save()?;

    ReloadContext {
        task_context: Some("resume headless worker".to_string()),
        version_before: "old-headless".to_string(),
        version_after: "new-headless".to_string(),
        session_id: headless.id.clone(),
        timestamp: "2026-04-19T00:00:00Z".to_string(),
    }
    .save()?;

    let headed_session_id = crate::id::new_id("headed-reconnect");
    ReloadContext {
        task_context: Some("resume headed reconnecting session".to_string()),
        version_before: "old-headed".to_string(),
        version_after: "new-headed".to_string(),
        session_id: headed_session_id.clone(),
        timestamp: "2026-04-19T00:00:01Z".to_string(),
    }
    .save()?;

    let swarm_id = "swarm-reload-headed-mixed";
    persist_swarm_state_snapshot(
        swarm_id,
        None,
        None,
        &[persisted_headless_member(
            &headless.id,
            swarm_id,
            "running",
            "bash tool",
        )],
    );

    let server = Server::new(provider.clone());
    server.recover_headless_sessions_on_startup().await;

    timeout(Duration::from_secs(5), async {
        loop {
            let sessions = server.sessions.read().await;
            let Some(headless_agent) = sessions.get(&headless.id).cloned() else {
                drop(sessions);
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            };
            drop(sessions);

            let headless_done = {
                let guard = headless_agent.lock().await;
                guard.messages().iter().any(|message| {
                    message.role == Role::Assistant
                        && message.content_preview().contains("continued after reload")
                })
            };
            if headless_done {
                break;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("headless reload recovery should complete");

    assert!(
        ReloadContext::peek_for_session(&headless.id)?.is_none(),
        "headless session reload context should be consumed by startup recovery"
    );
    assert!(
        ReloadContext::peek_for_session(&headed_session_id)?.is_some(),
        "headed reconnecting session reload context should remain available for later reconnect"
    );

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "test intentionally serializes process-wide JCODE_HOME/env state across async startup assertions"
)]
async fn startup_ready_signal_is_not_blocked_by_headless_recovery_delay() -> Result<()> {
    use std::os::unix::io::FromRawFd;
    use tokio::io::AsyncReadExt;

    let _storage_guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new()?;
    let _env = configure_test_env(&temp);
    let _delay_guard = ScopedEnvVar::set("JCODE_TEST_HEADLESS_STARTUP_RECOVERY_DELAY_MS", "500");

    let mut headless =
        crate::session::Session::create(None, Some("headless-ready-delay".to_string()));
    headless.save()?;

    let swarm_id = "swarm-ready-before-recovery";
    persist_swarm_state_snapshot(
        swarm_id,
        None,
        None,
        &[persisted_headless_member(
            &headless.id,
            swarm_id,
            "running",
            "delay startup recovery",
        )],
    );

    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![StreamEvent::MessageEnd { stop_reason: None }]);
    let server = Server::new(provider);

    let mut ready_fds = [0; 2];
    let pipe_rc = unsafe { libc::pipe(ready_fds.as_mut_ptr()) };
    assert_eq!(pipe_rc, 0, "pipe() should succeed");
    let read_fd = ready_fds[0];
    let write_fd = ready_fds[1];
    let _ready_fd_guard = ScopedEnvVar::set("JCODE_READY_FD", write_fd.to_string());

    let main_listener = crate::transport::Listener::bind(&server.socket_path)?;
    let debug_listener = crate::transport::Listener::bind(&server.debug_socket_path)?;

    let startup = tokio::spawn(async move {
        server
            .finish_startup_after_bind(main_listener, debug_listener, Instant::now())
            .await
    });

    let read_file = unsafe { std::fs::File::from_raw_fd(read_fd) };
    let mut async_read = tokio::fs::File::from_std(read_file);
    let mut ready = [0u8; 1];
    timeout(Duration::from_millis(200), async {
        async_read.read_exact(&mut ready).await
    })
    .await
    .expect("ready signal should arrive before delayed startup recovery completes")?;
    assert_eq!(ready, [b'R']);
    assert!(
        !startup.is_finished(),
        "startup task should still be blocked on delayed recovery even though ready was already signaled"
    );

    let (runtime, main_handle, debug_handle) = timeout(Duration::from_secs(2), startup)
        .await
        .expect("startup should finish after delayed recovery")
        .expect("startup task should succeed");
    timeout(Duration::from_secs(1), runtime.shutdown())
        .await
        .expect("runtime should shut down");
    timeout(Duration::from_secs(1), async {
        main_handle.await.expect("main accept loop");
        debug_handle.await.expect("debug accept loop");
    })
    .await
    .expect("accept loops should observe runtime cancellation");

    Ok(())
}
