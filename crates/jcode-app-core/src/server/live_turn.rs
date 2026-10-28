//! Server-initiated ("wake") turns for live sessions.
//!
//! Several server paths start a full conversation turn in a session without
//! that session's client sending a message: swarm DM/broadcast wake delivery,
//! background-task completion wakes, scheduled-task delivery, and post-reload
//! resume. Those turns must keep the same bookkeeping as client-initiated
//! turns, otherwise the swarm member status stays "ready/idle" while the agent
//! is actually streaming and attached TUIs never learn the turn finished.
//!
//! This module is the single shared implementation: it marks the member
//! `running` while the turn streams, flips it back to `ready` (with a
//! completion report) or `failed` at the end, and fans out a terminal
//! `Done`/`Error` event (id 0) so attached clients can settle the externally
//! started turn in their UI.

use super::turn_failover::process_reserved_locked_message_with_server_failover;
use super::{
    SwarmEvent, SwarmMember, truncate_detail, update_member_status,
    update_member_status_with_report,
};
use crate::agent::Agent;
use crate::protocol::ServerEvent;
use futures::FutureExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
#[cfg(test)]
use tokio::sync::Mutex;
use tokio::sync::{OwnedMutexGuard, RwLock, broadcast};

use super::SessionAgents;

/// Swarm bookkeeping handles needed to keep member status accurate around a
/// server-initiated turn.
#[derive(Clone)]
pub(super) struct LiveTurnSwarmContext {
    pub members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub event_history: Arc<RwLock<VecDeque<SwarmEvent>>>,
    pub event_counter: Arc<AtomicU64>,
    pub event_tx: broadcast::Sender<SwarmEvent>,
    pub client_connections: ClientConnections,
}

impl LiveTurnSwarmContext {
    pub(super) fn new(
        members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
        swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
        event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
        event_counter: &Arc<AtomicU64>,
        event_tx: &broadcast::Sender<SwarmEvent>,
        client_connections: &ClientConnections,
    ) -> Self {
        Self {
            members: Arc::clone(members),
            swarms_by_id: Arc::clone(swarms_by_id),
            event_history: Arc::clone(event_history),
            event_counter: Arc::clone(event_counter),
            event_tx: event_tx.clone(),
            client_connections: Arc::clone(client_connections),
        }
    }
}

/// Resolve an environment only when it is unambiguous for the target session.
/// Disconnected interactive seats are not driven immediately; headless workers
/// intentionally have no client env and run with an explicitly empty scope.
async fn target_client_terminal_env(
    session_id: &str,
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_connections: &ClientConnections,
) -> Option<Vec<(String, String)>> {
    let is_headless = members
        .read()
        .await
        .get(session_id)
        .is_some_and(|member| member.is_headless);
    let connections = client_connections.read().await;
    let mut snapshots = connections
        .values()
        .filter(|connection| connection.session_id == session_id)
        .map(|connection| connection.terminal_env.clone());
    let Some(first) = snapshots.next() else {
        return is_headless.then(Vec::new);
    };
    snapshots.all(|snapshot| snapshot == first).then_some(first)
}

/// Reserve the live agent for `session_id` when the session has at least one
/// live client attachment and its agent is currently idle.
///
/// The returned guard *is* the reservation: it stays held until the tracked
/// turn finishes, so two concurrent wakes cannot both observe the agent as
/// idle and then serialize behind each other (#1152).
pub(super) async fn idle_live_agent(
    session_id: &str,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<OwnedMutexGuard<Agent>> {
    let agent =
        super::worker_revive::live_or_revived_agent(session_id, sessions, swarm_members).await?;

    // Headless workers never have a client attachment by design, and members
    // restored from disk after a server restart carry a closed placeholder
    // channel. Both are still server-drivable, so they must remain wakeable:
    // otherwise a wake falls back to a soft-interrupt queue that an idle agent
    // never drains, and the worker silently stays idle.
    let can_drive = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .map(|member| {
                member.is_headless || !member.event_txs.is_empty() || !member.event_tx.is_closed()
            })
            .unwrap_or(false)
    };
    if !can_drive {
        return None;
    }

    agent.try_lock_owned().ok()
}

/// Spawn `message` as a full tracked turn in a live session.
///
/// Mirrors the client-initiated turn lifecycle: the swarm member is marked
/// `running` before the turn starts and `ready` (with a completion report) or
/// `failed` when it finishes. A synthetic terminal `Done { id: 0 }` (or
/// `Error { id: 0, .. }`) is fanned out to attached clients so their UI can
/// finish rendering the externally started turn.
pub(super) async fn spawn_tracked_live_turn(
    session_id: &str,
    mut agent: OwnedMutexGuard<Agent>,
    message: String,
    system_reminder: Option<String>,
    display_role: Option<crate::session::StoredDisplayRole>,
    status_detail: Option<String>,
    swarm: LiveTurnSwarmContext,
) -> bool {
    let Ok(_admission) = crate::runtime_activity::global().try_enter_session(session_id) else {
        return false;
    };
    let Ok(turn_lease) = agent.claim_server_turn() else {
        return false;
    };
    if swarm
        .members
        .read()
        .await
        .get(session_id)
        .is_some_and(|member| member.report_back_to_session_id.is_some())
    {
        agent.set_inline_output_tap(true);
    }
    let Some(client_terminal_env) =
        target_client_terminal_env(session_id, &swarm.members, &swarm.client_connections).await
    else {
        crate::logging::warn(&format!(
            "Skipping server-initiated turn for session {} because its client identity is disconnected or ambiguous",
            session_id
        ));
        return false;
    };
    update_member_status(
        session_id,
        "running",
        status_detail,
        &swarm.members,
        &swarm.swarms_by_id,
        Some(&swarm.event_history),
        Some(&swarm.event_counter),
        Some(&swarm.event_tx),
    )
    .await;

    let (fallback, _) = tokio::sync::mpsc::unbounded_channel();
    let events = super::state::TurnEventChannel::new(
        session_id.to_string(),
        Arc::clone(&swarm.members),
        fallback,
        &turn_lease,
    );
    let event_tx = events.sender();
    let session_id = session_id.to_string();
    crate::runtime_activity::spawn_session(&session_id.clone(), async move {
        let start_message_index = agent.message_count();
        let pending_queue = agent.soft_interrupt_queue();
        let (result, stop_reason) = catch_live_turn_panic(async {
            if let Some(display_role) = display_role {
                agent
                    .run_reserved_streaming_mpsc(
                        &turn_lease,
                        &message,
                        vec![],
                        system_reminder,
                        event_tx.clone(),
                    )
                    .await
            } else {
                process_reserved_locked_message_with_server_failover(
                    &mut agent,
                    &turn_lease,
                    &message,
                    vec![],
                    system_reminder,
                    event_tx.clone(),
                )
                .await
            }
        })
        .await;
        let completion_report = agent.recent_assistant_output_tail_after(start_message_index);
        // Keep the reservation until after the terminal status is published.
        // Releasing it earlier lets a follow-up wake reserve the agent and
        // publish `running`, which this turn's later `ready`/`failed` would
        // then overwrite, hiding the newer turn and suppressing its
        // coordinator completion notification.
        let mut reservation = agent;
        let pending = pending_queue.lock().map(|queue| queue.len()).unwrap_or(0);
        if pending > 0 {
            let _ = event_tx.send(ServerEvent::SoftInterruptQueue { pending });
        }
        match result {
            Ok(()) => {
                update_member_status_with_report(
                    &session_id,
                    "ready",
                    None,
                    completion_report,
                    &swarm.members,
                    &swarm.swarms_by_id,
                    Some(&swarm.event_history),
                    Some(&swarm.event_counter),
                    Some(&swarm.event_tx),
                )
                .await;
                let _ = event_tx.send(ServerEvent::Done { id: 0 });
            }
            Err(error) => {
                let message = crate::util::format_error_chain(&error);
                if let Err(persist_error) = reservation.persist_turn_error(&message) {
                    crate::logging::error(&format!(
                        "Could not persist failed live turn for {}: {:#}",
                        session_id, persist_error
                    ));
                }
                crate::logging::error(&format!(
                    "Server-initiated turn failed for live session {}: {}",
                    session_id, error
                ));
                update_member_status_with_report(
                    &session_id,
                    "failed",
                    Some(truncate_detail(&error.to_string(), 120)),
                    completion_report,
                    &swarm.members,
                    &swarm.swarms_by_id,
                    Some(&swarm.event_history),
                    Some(&swarm.event_counter),
                    Some(&swarm.event_tx),
                )
                .await;
                let _ = event_tx.send(ServerEvent::TurnStopped {
                    reason: stop_reason,
                    message: message.clone(),
                    provider_stop_reason: None,
                });
                let _ = event_tx.send(ServerEvent::Error {
                    id: 0,
                    message,
                    retry_after_secs: None,
                });
            }
        }
        drop(reservation);
        events.finish().await;
        drop(turn_lease);
    })
    .is_some()
}

/// Run `message` immediately as a tracked turn if the session is live and
/// idle. Returns `true` when the turn was started.
pub(super) async fn run_live_turn_if_idle(
    session_id: &str,
    message: &str,
    system_reminder: Option<String>,
    sessions: &SessionAgents,
    swarm: LiveTurnSwarmContext,
) -> bool {
    let Some(agent) = idle_live_agent(session_id, sessions, &swarm.members).await else {
        return false;
    };
    let detail = Some(truncate_detail(message, 120)).filter(|detail| !detail.is_empty());
    spawn_tracked_live_turn(
        session_id,
        agent,
        message.to_string(),
        system_reminder,
        None,
        detail,
        swarm,
    )
    .await
}

pub(super) async fn run_live_system_turn_if_idle(
    session_id: &str,
    message: &str,
    sessions: &SessionAgents,
    swarm: LiveTurnSwarmContext,
) -> bool {
    let Some(agent) = idle_live_agent(session_id, sessions, &swarm.members).await else {
        return false;
    };
    let detail = Some(truncate_detail(message, 120)).filter(|detail| !detail.is_empty());
    spawn_tracked_live_turn(
        session_id,
        agent,
        message.to_string(),
        None,
        Some(crate::session::StoredDisplayRole::System),
        detail,
        swarm,
    )
    .await
}

/// Catch only unwind panics. A killed process cannot emit a trustworthy event.
async fn catch_live_turn_panic<F>(turn: F) -> (anyhow::Result<()>, crate::protocol::TurnStopReason)
where
    F: std::future::Future<Output = anyhow::Result<()>>,
{
    use crate::protocol::TurnStopReason;
    match std::panic::AssertUnwindSafe(turn).catch_unwind().await {
        Ok(result) => (result, TurnStopReason::Failure),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            (
                Err(anyhow::anyhow!("Processing task panicked: {}", message)),
                TurnStopReason::Crash,
            )
        }
    }
}

#[cfg(test)]
mod stop_reason_tests {
    use super::catch_live_turn_panic;
    use crate::protocol::TurnStopReason;

    #[tokio::test]
    async fn abnormal_stop_classifies_panics_without_parsing_error_strings() {
        let (result, reason) = catch_live_turn_panic(async { panic!("provider exploded") }).await;
        assert_eq!(reason, TurnStopReason::Crash);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("provider exploded")
        );
        let (result, reason) = catch_live_turn_panic(async {
            Err(anyhow::anyhow!(
                "Processing task panicked: just provider text"
            ))
        })
        .await;
        assert_eq!(reason, TurnStopReason::Failure);
        assert!(result.is_err());
        assert!(catch_live_turn_panic(async { Ok(()) }).await.0.is_ok());
    }
}

#[cfg(test)]
mod durable_error_tests {
    use super::*;
    use crate::message::{Message, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use crate::session::Session;
    use crate::tool::Registry;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;

    const FAILURE: &str = "invalid_request_error: No tool call found for function call output with call_id call_missing";

    #[derive(Default)]
    struct FailingProvider(AtomicU64);

    #[async_trait::async_trait]
    impl Provider for FailingProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!(FAILURE).context("mock HTTP 400"))
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self::default())
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn failed_wake_error_is_durable_before_live_terminal_event() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        struct RestoreHome(Option<std::ffi::OsString>);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => crate::env::set_var("JCODE_HOME", value),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
            }
        }
        let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", home.path());
        let provider = Arc::new(FailingProvider::default());
        let mut agent = Agent::new(provider.clone(), Registry::empty());
        agent.set_memory_enabled(false);
        let session_id = agent.session_id().to_string();
        let agent = Arc::new(Mutex::new(agent));
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let member = SwarmMember {
            session_id: session_id.clone(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: true,
            status: "ready".into(),
            detail: None,
            friendly_name: Some("failing-worker".into()),
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "agent".into(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: true,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
            task_label: None,
        };
        let members = Arc::new(RwLock::new(HashMap::from([(session_id.clone(), member)])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
        let event_history = Arc::new(RwLock::new(VecDeque::new()));
        let event_counter = Arc::new(AtomicU64::new(0));
        let (swarm_event_tx, _) = broadcast::channel(16);
        let client_connections = Arc::new(RwLock::new(HashMap::new()));
        let swarm = LiveTurnSwarmContext::new(
            &members,
            &swarms_by_id,
            &event_history,
            &event_counter,
            &swarm_event_tx,
            &client_connections,
        );
        let accepted = agent.lock().await.claim_server_turn().unwrap();
        assert!(
            !spawn_tracked_live_turn(
                &session_id,
                agent.clone().lock_owned().await,
                "must not compete".into(),
                None,
                Some(crate::session::StoredDisplayRole::System),
                None,
                swarm.clone(),
            )
            .await,
            "system display-role path must respect the accepted turn"
        );
        assert_eq!(provider.0.load(Ordering::SeqCst), 0);
        drop(accepted);
        assert!(
            spawn_tracked_live_turn(
                &session_id,
                agent.clone().lock_owned().await,
                "wake worker".into(),
                None,
                None,
                None,
                swarm,
            )
            .await
        );

        let mut stopped = false;
        let error_message = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = event_rx.recv().await.expect("live turn event stream");
                let payload = match event {
                    ServerEvent::TurnEvent { event, .. } => *event,
                    event => event,
                };
                match payload {
                    ServerEvent::TurnStopped {
                        reason, message, ..
                    } => {
                        assert_eq!(reason, crate::protocol::TurnStopReason::Failure);
                        assert!(message.contains(FAILURE));
                        stopped = true;
                    }
                    ServerEvent::Error { id, message, .. } => {
                        assert_eq!(id, 0);
                        break message;
                    }
                    ServerEvent::Done { .. } => panic!("failed live turn emitted Done"),
                    _ => {}
                }
            }
        })
        .await
        .expect("failed live turn must terminate");
        assert!(stopped);
        assert_eq!(error_message, format!("mock HTTP 400: {FAILURE}"));
        assert_eq!(provider.0.load(Ordering::SeqCst), 1);
        assert_eq!(members.read().await[&session_id].status, "failed");

        // Loading as soon as Error arrives must already show the error. This
        // exercises the actual spawn path, not only the persistence helper.
        let loaded = Session::load(&session_id).unwrap();
        assert_eq!(loaded.replay_events.len(), 1);
        assert!(
            !serde_json::to_string(&loaded.messages)
                .unwrap()
                .contains(FAILURE)
        );
        let history = crate::session::render_messages(&loaded);
        let failure = history
            .iter()
            .position(|message| message.role == "error")
            .unwrap();
        assert_eq!(history[failure].content, error_message);
        assert_eq!(history[failure - 1].content, "wake worker");
        assert_eq!(
            history
                .iter()
                .filter(|message| message.role == "error")
                .count(),
            1
        );
        // Join the agent reservation before dropping the isolated test home.
        drop(agent.lock().await);
    }
}

#[cfg(test)]
mod identity_scope_tests {
    use super::*;
    use crate::message::{Message, StreamEvent, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use std::sync::Mutex as StdMutex;
    use std::time::Instant;
    use tokio::sync::mpsc;

    #[derive(Clone, Default)]
    struct IdentityProvider(Arc<StdMutex<Vec<Option<String>>>>);

    #[async_trait::async_trait]
    impl Provider for IdentityProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            let mut inherited = HashMap::from([(
                "OPENRIG_SESSION_NAME".to_string(),
                "queue-worker@kernel".to_string(),
            )]);
            crate::hooks::apply_client_env_pairs(
                |key, value| match value {
                    Some(value) => {
                        inherited.insert(key.to_string(), value.to_string());
                    }
                    None => {
                        inherited.remove(key);
                    }
                },
                &crate::hooks::client_env_for_tools(),
            );
            self.0
                .lock()
                .expect("identity provider lock")
                .push(inherited.get("OPENRIG_SESSION_NAME").cloned());
            Ok(Box::pin(tokio_stream::iter(vec![
                Ok(StreamEvent::TextDelta("turn complete".to_string())),
                Ok(StreamEvent::MessageEnd { stop_reason: None }),
            ])))
        }

        fn name(&self) -> &str {
            "identity-test"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(self.clone())
        }
    }

    fn member(
        session_id: &str,
        event_tx: mpsc::UnboundedSender<ServerEvent>,
        attach_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> SwarmMember {
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx,
            event_txs: HashMap::from([("client".to_string(), attach_tx)]),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            friendly_name: None,
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

    fn connection(session_id: &str, identity: &str) -> crate::server::ClientConnectionInfo {
        let (disconnect_tx, _) = mpsc::unbounded_channel();
        let now = Instant::now();
        crate::server::ClientConnectionInfo {
            client_id: format!("connection-{identity}"),
            session_id: session_id.to_string(),
            client_instance_id: None,
            debug_client_id: None,
            connected_at: now,
            last_seen: now,
            is_processing: false,
            current_tool_name: None,
            terminal_env: vec![("OPENRIG_SESSION_NAME".to_string(), identity.to_string())],
            disconnect_tx,
        }
    }

    #[tokio::test]
    async fn concurrent_live_turns_keep_target_identity_and_reject_missing_or_ambiguous_env() {
        let _env_lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("isolated Jcode home");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        struct RestoreHome(Option<std::ffi::OsString>);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => crate::env::set_var("JCODE_HOME", value),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
            }
        }
        let _restore_home = RestoreHome(previous_home);

        let provider = Arc::new(IdentityProvider::default());
        let mut agents = Vec::new();
        let mut members = HashMap::new();
        let mut connections = HashMap::new();
        let mut receivers = Vec::new();
        let mut ids = Vec::new();
        for identity in ["design-designer@hull", "product-partner@hull"] {
            let mut agent = Agent::new(provider.clone(), Registry::empty());
            agent.set_memory_enabled(false);
            let session_id = agent.session_id().to_string();
            let agent = Arc::new(Mutex::new(agent));
            let (event_tx, event_rx) = mpsc::unbounded_channel();
            let (attach_tx, attach_rx) = mpsc::unbounded_channel();
            members.insert(session_id.clone(), member(&session_id, event_tx, attach_tx));
            connections.insert(
                format!("connection-{identity}"),
                connection(&session_id, identity),
            );
            agents.push(agent);
            receivers.push((event_rx, attach_rx));
            ids.push(session_id);
        }
        let members = Arc::new(RwLock::new(members));
        let connections = Arc::new(RwLock::new(connections));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
        let event_history = Arc::new(RwLock::new(VecDeque::new()));
        let event_counter = Arc::new(AtomicU64::new(0));
        let (event_tx, _) = broadcast::channel(16);
        let make_context = |connections: &ClientConnections| {
            LiveTurnSwarmContext::new(
                &members,
                &swarms_by_id,
                &event_history,
                &event_counter,
                &event_tx,
                connections,
            )
        };

        let (first_started, second_started) = tokio::join!(
            spawn_tracked_live_turn(
                &ids[0],
                Arc::clone(&agents[0]).lock_owned().await,
                "wake designer".to_string(),
                None,
                None,
                None,
                make_context(&connections),
            ),
            spawn_tracked_live_turn(
                &ids[1],
                Arc::clone(&agents[1]).lock_owned().await,
                "wake product partner".to_string(),
                None,
                None,
                None,
                make_context(&connections),
            ),
        );
        assert!(first_started && second_started);

        for (_, attach_rx) in &mut receivers {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if matches!(attach_rx.recv().await, Some(ServerEvent::Done { id: 0 })) {
                        break;
                    }
                }
            })
            .await
            .expect("each target live turn should complete");
        }
        let mut seen = provider.0.lock().expect("identity provider lock").clone();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                Some("design-designer@hull".to_string()),
                Some("product-partner@hull".to_string()),
            ]
        );

        let empty_connections = Arc::new(RwLock::new(HashMap::new()));
        assert!(
            !spawn_tracked_live_turn(
                &ids[1],
                Arc::clone(&agents[1]).lock_owned().await,
                "must not run without a connected client".to_string(),
                None,
                None,
                None,
                make_context(&empty_connections),
            )
            .await
        );

        let ambiguous_connections = Arc::new(RwLock::new(HashMap::from([
            (
                "first-client".to_string(),
                connection(&ids[0], "design-designer@hull"),
            ),
            (
                "second-client".to_string(),
                connection(&ids[0], "queue-worker@kernel"),
            ),
        ])));
        assert!(
            !spawn_tracked_live_turn(
                &ids[0],
                Arc::clone(&agents[0]).lock_owned().await,
                "must not run with ambiguous client identity".to_string(),
                None,
                None,
                None,
                make_context(&ambiguous_connections),
            )
            .await
        );
        assert_eq!(provider.0.lock().expect("identity provider lock").len(), 2);
    }
}
