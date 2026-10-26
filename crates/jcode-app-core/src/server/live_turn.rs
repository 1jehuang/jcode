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

use super::turn_failover::process_locked_message_with_server_failover;
use super::{
    SwarmEvent, SwarmMember, session_event_fanout_sender, truncate_detail, update_member_status,
    update_member_status_with_report,
};
use crate::agent::Agent;
use crate::protocol::ServerEvent;
use futures::FutureExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock, broadcast};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;

/// Swarm bookkeeping handles needed to keep member status accurate around a
/// server-initiated turn.
#[derive(Clone)]
pub(super) struct LiveTurnSwarmContext {
    pub members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub event_history: Arc<RwLock<VecDeque<SwarmEvent>>>,
    pub event_counter: Arc<AtomicU64>,
    pub event_tx: broadcast::Sender<SwarmEvent>,
}

impl LiveTurnSwarmContext {
    pub(super) fn new(
        members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
        swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
        event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
        event_counter: &Arc<AtomicU64>,
        event_tx: &broadcast::Sender<SwarmEvent>,
    ) -> Self {
        Self {
            members: Arc::clone(members),
            swarms_by_id: Arc::clone(swarms_by_id),
            event_history: Arc::clone(event_history),
            event_counter: Arc::clone(event_counter),
            event_tx: event_tx.clone(),
        }
    }
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
    let Ok(_admission) = crate::runtime_activity::global().try_enter() else {
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

    let event_tx = session_event_fanout_sender(session_id.to_string(), Arc::clone(&swarm.members));
    let session_id = session_id.to_string();
    crate::runtime_activity::spawn(async move {
        let start_message_index = agent.message_count();
        let pending_queue = agent.soft_interrupt_queue();
        let (result, stop_reason) = catch_live_turn_panic(async {
            if let Some(display_role) = display_role {
                agent
                    .run_once_streaming_mpsc_with_display_role(
                        &message,
                        vec![],
                        system_reminder,
                        event_tx.clone(),
                        Some(display_role),
                    )
                    .await
            } else {
                process_locked_message_with_server_failover(
                    &mut agent,
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
        let swarm = LiveTurnSwarmContext::new(
            &members,
            &swarms_by_id,
            &event_history,
            &event_counter,
            &swarm_event_tx,
        );
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
                match event_rx.recv().await.expect("live turn event stream") {
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
