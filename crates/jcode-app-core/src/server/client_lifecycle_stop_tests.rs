#![allow(clippy::await_holding_lock)]
//! A client turn must be refused once the session it targets was stopped.
//!
//! Split out of `client_lifecycle_tests.rs`, which is at its size budget.

use super::*;
use crate::message::{Message, ToolDefinition};
use crate::provider::EventStream;
use async_trait::async_trait;

/// A provider that must never be reached: admission is decided before any work.
struct NoRequests;

#[async_trait]
impl Provider for NoRequests {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("a stopped session must not reach the provider")
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

struct Home {
    _temp: tempfile::TempDir,
    old: Option<std::ffi::OsString>,
}

impl Home {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let old = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        Self { _temp: temp, old }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            crate::env::set_var("JCODE_HOME", old);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

/// A stop removes the worker from routing but the connection keeps its own
/// handle to the Agent. A turn submitted afterwards must be rejected under that
/// handle instead of starting, running, and reporting a completion for a session
/// the coordinator already retired.
#[tokio::test]
async fn stopped_session_rejects_a_client_turn_before_it_starts() {
    assert_client_turn_rejected(true).await;
}

#[tokio::test]
async fn residual_pr1362_open_agent_with_stopping_gate_rejects_client_turn() {
    assert_client_turn_rejected(false).await;
}

async fn assert_client_turn_rejected(closed: bool) {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let id = if closed {
        "session_stopped_client_turn"
    } else {
        "session_stopping_client_turn"
    };
    let provider: Arc<dyn Provider> = Arc::new(NoRequests);
    let registry = Registry::new(provider.clone()).await;
    let mut session = crate::session::Session::create_with_id(id.into(), None, None);
    session.working_dir = Some("/workspace/stopped".into());
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )));
    if closed {
        agent.lock().await.mark_closed();
    } else {
        super::super::state::begin_session_interrupt_stop(id).await;
        assert!(!agent.lock().await.is_closed());
    }

    let members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (event_tx, _event_rx) = broadcast::channel(8);
    let swarm = SwarmStatusRefs {
        members: &members,
        swarms_by_id: &swarms_by_id,
        event_history: &event_history,
        event_counter: &event_counter,
        event_tx: &event_tx,
    };

    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
    let (processing_done_tx, _processing_done_rx) = mpsc::unbounded_channel();
    let mut client_is_processing = false;
    let mut message_id = None;
    let mut session_id = None;
    let mut task = None;
    let mut state = ProcessingState {
        client_is_processing: &mut client_is_processing,
        message_id: &mut message_id,
        session_id: &mut session_id,
        task: &mut task,
    };

    start_processing_message(
        ProcessingMessage {
            id: 7,
            content: "after the stop".to_string(),
            images: Vec::new(),
            system_reminder: None,
            active_skill: None,
        },
        id,
        &mut state,
        &agent,
        &client_event_tx,
        &processing_done_tx,
        Vec::new(),
        &swarm,
    )
    .await;

    match client_event_rx.try_recv() {
        Ok(ServerEvent::Error {
            id: rejected_id,
            message,
            ..
        }) => {
            assert_eq!(rejected_id, 7, "the rejection must answer the refused turn");
            assert!(
                message.contains("stopped"),
                "unexpected rejection reason: {message}"
            );
        }
        other => panic!("expected a stopped-session rejection, got {other:?}"),
    }
    assert!(
        !client_is_processing,
        "a refused turn must not claim the session as processing"
    );
    assert!(message_id.is_none(), "a refused turn must not be recorded");
    assert!(
        session_id.is_none(),
        "a refused turn must not own the processing session"
    );
    assert!(task.is_none(), "a refused turn must not spawn work");
}

#[tokio::test(flavor = "current_thread")]
async fn residual_pr1362_client_reserves_agent_while_publishing_running() {
    assert_client_reservation(true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn residual_pr1362_client_reserves_agent_before_spawned_task_first_poll() {
    assert_client_reservation(false).await;
}

async fn assert_client_reservation(check_while_publishing: bool) {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let id = if check_while_publishing {
        "session_client_reservation_publishing"
    } else {
        "session_client_reservation_spawned"
    };
    let provider: Arc<dyn Provider> = Arc::new(NoRequests);
    let registry = Registry::new(provider.clone()).await;
    let session = crate::session::Session::create_with_id(id.into(), None, None);
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )));
    let members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (event_tx, _event_rx) = broadcast::channel(8);
    let swarm = SwarmStatusRefs {
        members: &members,
        swarms_by_id: &swarms_by_id,
        event_history: &event_history,
        event_counter: &event_counter,
        event_tx: &event_tx,
    };
    let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel();
    let (processing_done_tx, _processing_done_rx) = mpsc::unbounded_channel();
    let mut client_is_processing = false;
    let mut message_id = None;
    let mut session_id = None;
    let mut task = None;
    let mut state = ProcessingState {
        client_is_processing: &mut client_is_processing,
        message_id: &mut message_id,
        session_id: &mut session_id,
        task: &mut task,
    };

    // Force startup to suspend at the running-status write, after admission.
    let members_guard = members.read().await;
    let mut start = Box::pin(start_processing_message(
        ProcessingMessage {
            id: 8,
            content: "reserved turn".into(),
            images: Vec::new(),
            system_reminder: None,
            active_skill: None,
        },
        id,
        &mut state,
        &agent,
        &client_event_tx,
        &processing_done_tx,
        Vec::new(),
        &swarm,
    ));
    assert!(futures::poll!(start.as_mut()).is_pending());
    if check_while_publishing {
        assert!(
            agent.try_lock().is_err(),
            "admitted client must reserve Agent while publishing running"
        );
        // Stop must not wait for a producer permit held by this reservation.
        let stop = super::super::state::begin_session_interrupt_stop(id);
        assert!(futures::poll!(std::pin::pin!(stop)).is_ready());
    }
    drop(members_guard);
    assert!(futures::poll!(start.as_mut()).is_ready());
    drop(start);
    // No executor yield occurred: the spawned task has never been polled.
    assert!(
        agent.try_lock().is_err(),
        "spawned client task must already own the Agent reservation"
    );
    let task = task.expect("admitted turn task");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(agent.try_lock().is_ok(), "abort must release reservation");
}
