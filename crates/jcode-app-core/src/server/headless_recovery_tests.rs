//! Startup regressions: durable session/member input, real Server recovery,
//! recording provider. No credentials, exec, builds, or shared daemon required.
//! Wire from server.rs with `#[cfg(test)] mod headless_recovery_tests;`.
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::bus::{BackgroundTaskCompleted, BackgroundTaskStatus};
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::session::{Session, SessionStatus};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

struct IsolatedEnv(Vec<(&'static str, Option<OsString>)>);
impl IsolatedEnv {
    fn new(root: &std::path::Path) -> Self {
        let values = [
            ("JCODE_HOME", root.join("home").into_os_string()),
            ("JCODE_RUNTIME_DIR", root.join("runtime").into_os_string()),
            ("JCODE_MEMORY_ENABLED", OsString::from("0")),
            ("JCODE_MEMORY_SIDECAR_ENABLED", OsString::from("0")),
            ("JCODE_WAKE_MODE", OsString::from("internal")),
        ];
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        let previous = values.into_iter().map(|(key, value)| {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            (key, previous)
        }).collect();
        crate::config::invalidate_config_cache();
        Self(previous)
    }
}
impl Drop for IsolatedEnv {
    fn drop(&mut self) {
        for (key, previous) in self.0.drain(..).rev() {
            match previous {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
        crate::config::invalidate_config_cache();
    }
}

#[derive(Default, Clone)]
struct RecordingProvider {
    requests: Arc<StdMutex<Vec<Vec<Message>>>>,
    replies: Arc<StdMutex<VecDeque<bool>>>, // true means a subscription limit
    gate: Option<Arc<tokio::sync::Semaphore>>,
}
impl RecordingProvider {
    fn limited_once() -> Self {
        let provider = Self::default();
        provider.replies.lock().unwrap().push_back(true);
        provider
    }
    fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
#[async_trait]
impl Provider for RecordingProvider {
    async fn complete(&self, messages: &[Message], _tools: &[ToolDefinition], _system: &str,
        _resume_session_id: Option<&str>) -> Result<EventStream> {
        self.requests.lock().unwrap().push(messages.to_vec());
        if let Some(gate) = &self.gate {
            gate.acquire().await.expect("test gate stays open").forget();
        }
        if self.replies.lock().unwrap().pop_front().unwrap_or(false) {
            let error = jcode_provider_core::usage_limit_resume::with_usage_limit_reset(
                anyhow::anyhow!("Usage limit reached for this account"),
                Some(Duration::from_secs(1)),
            );
            return Ok(Box::pin(tokio_stream::iter(vec![Err(error)])));
        }
        Ok(Box::pin(tokio_stream::iter(vec![
            Ok(StreamEvent::TextDelta("recovery-completed-marker".to_string())),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }
    fn name(&self) -> &str { "test" }
    fn fork(&self) -> Arc<dyn Provider> { Arc::new(self.clone()) }
}

fn member(id: &str, status: &str, owner: Option<&str>) -> SwarmMember {
    let (event_tx, _) = mpsc::unbounded_channel();
    SwarmMember {
        session_id: id.to_string(), event_tx, event_txs: HashMap::new(),
        working_dir: None, swarm_id: Some("startup-regression-swarm".to_string()),
        swarm_enabled: true, status: status.to_string(), detail: None,
        friendly_name: Some(id.to_string()), report_back_to_session_id: owner.map(str::to_string),
        latest_completion_report: None, role: "agent".to_string(),
        joined_at: Instant::now(), last_status_change: Instant::now(), is_headless: true,
        output_tail: None, todo_progress: None, todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(), task_label: None,
    }
}

fn seed_worker(status: &str, pending_tool: bool, interrupted: bool) -> Result<String> {
    let mut session = Session::create(None, Some("recovery-worker".to_string()));
    session.add_message(Role::User, vec![ContentBlock::Text {
        text: "original-work-marker".to_string(), cache_control: None,
    }]);
    if pending_tool {
        session.add_message(Role::Assistant, vec![ContentBlock::ToolUse {
            id: "pending-read".to_string(), name: "read".to_string(),
            input: serde_json::json!({"file_path": "/nonexistent-recovery-fixture"}),
            thought_signature: None,
        }]);
    }
    if interrupted {
        session.add_message(Role::User, vec![ContentBlock::ToolResult {
            tool_use_id: "interrupted-tool".to_string(),
            content: "[Tool 'read' interrupted by server reload after 0.2s]".to_string(),
            is_error: Some(true),
        }]);
    }
    // Closed is intentional: the durable member is the only evidence that an
    // assistant tool_use was still in flight. The legacy detector misses it.
    session.status = SessionStatus::Closed;
    session.save()?;
    let worker = member(&session.id, status, Some("recovery-owner"));
    persist_swarm_state_snapshot("startup-regression-swarm", None, Some("recovery-owner"), &[worker]);
    Ok(session.id)
}

async fn wait_terminal(server: &Server, id: &str) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = server.swarm_state.members.read().await[id].status.clone();
            if matches!(status.as_str(), "ready" | "failed" | "stopped") { return status; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("startup worker must reach a terminal status")
}

#[tokio::test]
async fn startup_recovers_pending_assistant_tool_use_without_reload_marker() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed_worker("running", true, false)?;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    server.recover_headless_sessions_on_startup().await;
    assert_eq!(wait_terminal(&server, &id).await, "ready");
    assert_eq!(provider.calls(), 1, "a durable running member must resume even when the transcript ends in assistant tool_use and SessionStatus is Closed");
    let requests = provider.requests.lock().unwrap();
    assert!(requests[0].iter().any(|m| m.content.iter().any(|block| matches!(block, ContentBlock::Text { text, .. } if text.contains("original-work-marker")))));
    Ok(())
}

#[tokio::test]
async fn startup_completion_retains_report_and_notifies_owner() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed_worker("running", false, true)?;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut owner = member("recovery-owner", "ready", None);
    owner.is_headless = false;
    owner.role = "coordinator".to_string();
    owner.event_tx = tx;
    server.swarm_state.members.write().await.insert(owner.session_id.clone(), owner);
    server.recover_headless_sessions_on_startup().await;
    assert_eq!(wait_terminal(&server, &id).await, "ready");
    let report = server.swarm_state.members.read().await[&id].latest_completion_report.clone();
    assert!(report.is_some_and(|text| text.contains("recovery-completed-marker")), "startup completion must retain the recovered assistant output as the worker completion report");
    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(events.iter().any(|event| matches!(event, crate::protocol::ServerEvent::Notification { message, .. } if message.contains("recovery-completed-marker"))), "owner must receive the recovered completion report, not just an empty ready notification: {events:?}");
    assert_eq!(provider.calls(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn startup_usage_limit_waits_then_resumes_same_worker_once() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed_worker("rate_limited", false, true)?;
    let provider = Arc::new(RecordingProvider::limited_once());
    let server = Server::new(provider.clone());
    server.recover_headless_sessions_on_startup().await;
    for _ in 0..1000 {
        let status = server.swarm_state.members.read().await[&id].status.clone();
        assert_ne!(status, "failed", "quota during startup recovery must enter the ordinary server-owned retry path");
        if status == "rate_limited" { break; }
        tokio::task::yield_now().await;
    }
    assert_eq!(server.swarm_state.members.read().await[&id].status, "rate_limited");
    assert_eq!(provider.calls(), 1);
    tokio::time::advance(Duration::from_secs(40)).await;
    assert_eq!(wait_terminal(&server, &id).await, "ready");
    assert_eq!(provider.calls(), 2, "one limited call and exactly one same-session continuation");
    let sessions = server.sessions.read().await;
    let agent = sessions[&id].lock().await;
    assert_eq!(agent.session_id(), id);
    assert_eq!(agent.messages().iter().filter(|message| message.role == Role::User && message.content_preview().contains("original-work-marker")).count(), 1, "continuation must not replay the original user message");
    Ok(())
}

#[tokio::test]
async fn restored_worker_accepts_background_completion_wake() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed_worker("running", false, true)?;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    server.recover_headless_sessions_on_startup().await;
    assert_eq!(wait_terminal(&server, &id).await, "ready");
    let task = BackgroundTaskCompleted {
        task_id: "restored-wake-marker".to_string(), tool_name: "bash".to_string(),
        display_name: None, session_id: id.clone(), status: BackgroundTaskStatus::Completed,
        exit_code: Some(0), output_preview: "completed after restart".to_string(),
        output_file: root.path().join("unused-output"), duration_secs: 1.0,
        notify: true, wake: true,
    };
    dispatch_background_task_completion(&task, &server.sessions, &server.soft_interrupt_queues,
        &server.swarm_state.members, &server.swarm_state.swarms_by_id,
        &server.event_history, &server.event_counter, &server.swarm_event_tx).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while provider.calls() < 2 { tokio::task::yield_now().await; }
    }).await.expect("a restored headless worker needs a usable live event sink so wake dispatch does not treat it as an external/detached session");
    assert_eq!(wait_terminal(&server, &id).await, "ready");
    assert_eq!(provider.calls(), 2);
    let requests = provider.requests.lock().unwrap();
    assert!(requests[1].iter().any(|m| m.content.iter().any(|block| matches!(block, ContentBlock::Text { text, .. } if text.contains("restored-wake-marker")))));
    Ok(())
}

#[tokio::test]
async fn repeated_startup_sweeps_resume_once_and_leave_ready_and_stopped_workers_alone() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed_worker("running", false, true)?;
    let mut ready = Session::create(None, Some("already-ready".to_string()));
    ready.save()?;
    let mut stopped = Session::create(None, Some("cancelled-worker".to_string()));
    stopped.save()?;
    persist_swarm_state_snapshot("startup-regression-swarm", None, Some("recovery-owner"), &[
        member(&id, "running", Some("recovery-owner")),
        member(&ready.id, "ready", Some("recovery-owner")),
        member(&stopped.id, "stopped", Some("recovery-owner")),
    ]);
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    tokio::join!(server.recover_headless_sessions_on_startup(), server.recover_headless_sessions_on_startup());
    assert_eq!(wait_terminal(&server, &id).await, "ready");
    server.recover_headless_sessions_on_startup().await;
    assert_eq!(provider.calls(), 1);
    let sessions = server.sessions.read().await;
    assert!(!sessions.contains_key(&ready.id));
    assert!(!sessions.contains_key(&stopped.id));
    let members = server.swarm_state.members.read().await;
    assert_eq!(members[&ready.id].status, "stopped");
    assert_eq!(members[&stopped.id].status, "stopped");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn owner_socket_resume_during_startup_does_not_duplicate_worker_turn() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed_worker("running", false, true)?;
    let mut owner = Session::create(None, Some("socket-owner".to_string()));
    owner.add_message(Role::User, vec![ContentBlock::Text {
        text: "owner-work-already-finished".to_string(), cache_control: None,
    }]);
    owner.add_message(Role::Assistant, vec![ContentBlock::Text {
        text: "owner-finished-marker".to_string(), cache_control: None,
    }]);
    owner.status = SessionStatus::Closed;
    owner.save()?;
    let mut idle = Session::create(None, Some("idle-before-restart".to_string()));
    idle.save()?;
    persist_swarm_state_snapshot("startup-regression-swarm", None, Some(&owner.id), &[
        member(&id, "running", Some(&owner.id)),
        member(&idle.id, "ready", Some(&owner.id)),
    ]);

    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = Arc::new(RecordingProvider { gate: Some(gate.clone()), ..Default::default() });
    // Keep Unix socket names short enough for macOS sun_path.
    let socket_path = root.path().join("s");
    let debug_path = root.path().join("d");
    let server = Server::new_with_paths(provider.clone(), socket_path.clone(), debug_path);
    let listener = crate::transport::Listener::bind(&server.socket_path)?;
    let debug_listener = crate::transport::Listener::bind(&server.debug_socket_path)?;
    let (runtime, main_handle, debug_handle) = server
        .finish_startup_after_bind(listener, debug_listener, Instant::now()).await;

    let result: Result<()> = async {
        tokio::time::timeout(Duration::from_secs(3), async {
            while provider.calls() == 0 { tokio::task::yield_now().await; }
        }).await.expect("startup must enter the gated worker provider request");
        let mut client = Client::connect_with_path(socket_path).await?;
        // Stateful ResumeSession is rejected before a Subscribe carrying cwd.
        let subscribe_id = client.subscribe_with_info(
            Some(root.path().to_string_lossy().into_owned()), Some(false), None, false, false,
        ).await?;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match client.read_event().await? {
                    crate::protocol::ServerEvent::Done { id } if id == subscribe_id => break,
                    crate::protocol::ServerEvent::Error { id, message, .. } if id == subscribe_id => anyhow::bail!("fixture subscribe: {message}"),
                    _ => {},
                }
            }
            Ok::<_, anyhow::Error>(())
        }).await??;
        let mut idle_notices = 0usize;
        // Repeated real resume requests while startup continuation is in flight.
        // No synthetic owner-handler invocation or direct registry insertion.
        for _ in 0..3 {
            let request_id = client.resume_session_with_options(&owner.id, true, true).await?;
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    match client.read_event().await? {
                        crate::protocol::ServerEvent::Done { id } if id == request_id => break,
                        crate::protocol::ServerEvent::Error { id, message, .. } if id == request_id => anyhow::bail!("fixture owner resume: {message}"),
                        crate::protocol::ServerEvent::Notification { message, .. }
                            if message.contains(&idle.id) && message.contains("not restored") => idle_notices += 1,
                        _ => {},
                    }
                }
                Ok::<_, anyhow::Error>(())
            }).await??;
        }
        assert_eq!(provider.calls(), 1, "owner resume must not enqueue a second continuation or restart the owner turn");
        gate.add_permits(4);
        assert_eq!(wait_terminal(&server, &id).await, "ready");
        // All resume Done replies were drained while the worker was gated.
        assert_eq!(provider.calls(), 1);
        assert_eq!(server.swarm_state.members.read().await[&idle.id].status, "stopped");
        // A later resume's stream drains the preceding post-Done notice.
        assert_eq!(idle_notices, 1, "owner attach must deliver exactly one idle-worker notice");
        Ok(())
    }.await;
    gate.add_permits(4);
    runtime.shutdown().await;
    main_handle.await?;
    debug_handle.await?;
    result
}
