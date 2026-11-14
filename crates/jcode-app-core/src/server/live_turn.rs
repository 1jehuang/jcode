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

#[cfg(test)]
#[path = "headless_live_turn_tests.rs"]
mod headless_live_turn_tests;

#[cfg(test)]
#[path = "headless_durable_recovery_tests.rs"]
mod headless_durable_recovery_tests;

use super::turn_failover::process_reserved_locked_context_with_server_failover;
use super::usage_limit_resume::ResumeOutcome;
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
use tokio::sync::{OwnedMutexGuard, RwLock, broadcast, mpsc};

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
    pub recovered_assignment: Option<super::recovered_assignment::RecoveredAssignment>,
    pub recovery_reload_id: Option<String>,
    pub recovery_swarm: Option<super::SwarmState>,
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
            recovered_assignment: None,
            recovery_reload_id: None,
            recovery_swarm: None,
        }
    }

    /// Owned callback future, also usable by the restored provider-wait runner.
    pub(super) fn on_wait(
        &self,
        plan: &super::usage_limit_resume::UsageLimitResumePlan,
    ) -> futures::future::BoxFuture<'static, ()> {
        let assignment = self.recovered_assignment.clone();
        let plan = plan.clone();
        Box::pin(async move {
            if let Some(assignment) = assignment {
                assignment.on_wait(&plan).await;
            }
        })
    }
}

/// A restored headless Agent is live without any client event receiver. A
/// detached TUI is not. Terminal or deliberately suspended workers must not
/// restart merely because a notification or resume-all sweep reaches them.
pub(super) fn member_accepts_automatic_turn(member: &SwarmMember) -> bool {
    !matches!(
        member.status.as_str(),
        "stopped" | "completed" | "done" | "suspended" | "paused"
    ) && (member.is_headless
        || !member.event_tx.is_closed()
        || member.event_txs.values().any(|tx| !tx.is_closed()))
        && !super::usage_limit_resume::has_pending_resume(&member.session_id)
}

/// An explicit resume sweep continues unanswered work, never an empty or
/// already answered conversation. The caller still owns the Agent reservation.
pub(super) fn live_session_owes_continuation(agent: &Agent) -> bool {
    if agent.visible_conversation_message_count() == 0 {
        return false;
    }
    super::reload_recovery::peek_for_session(agent.session_id())
        .ok()
        .flatten()
        .is_some_and(|record| {
            record.status == super::reload_recovery::ReloadRecoveryStatus::Pending
        })
        || super::client_session::session_was_interrupted_by_reload(agent)
        || matches!(
            agent.last_visible_conversation_role(),
            Some(crate::message::Role::User)
        )
}

/// Reserve a live, eligible session's idle agent, including a restored headless
/// worker whose event receivers did not survive the restart.
///
/// The returned guard *is* the reservation: it stays held until the tracked
/// turn finishes, so two concurrent wakes cannot both observe the agent as
/// idle and then serialize behind each other (#1152).
pub(super) async fn idle_live_agent(
    session_id: &str,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<OwnedMutexGuard<Agent>> {
    if !swarm_members
        .read()
        .await
        .get(session_id)
        .is_some_and(member_accepts_automatic_turn)
    {
        return None;
    }
    let agent =
        super::worker_revive::live_or_revived_agent(session_id, sessions, swarm_members).await?;

    let agent = agent.try_lock_owned().ok()?;
    let eligible = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .map(member_accepts_automatic_turn)
            .unwrap_or(false)
    };
    if !eligible {
        return None;
    }

    // A session that moved to another machine (or a stale copy) cannot run
    // turns here. Starting one only produces a lease error per wake, so leave
    // the wake queued for whichever copy is runnable.
    if agent.session_lease_blocked() {
        return None;
    }
    Some(agent)
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
    agent: OwnedMutexGuard<Agent>,
    message: String,
    system_reminder: Option<String>,
    display_role: Option<crate::session::StoredDisplayRole>,
    status_detail: Option<String>,
    swarm: LiveTurnSwarmContext,
) -> bool {
    // Direct callers can explicitly replace a waiting turn. Automatic wake
    // callers have already refused sessions whose provider wait still owns it.
    super::usage_limit_resume::cancel_pending_resume(session_id);
    let publication = super::swarm::begin_turn_publication(session_id).await;
    spawn_tracked_live_turn_with_publication(
        session_id,
        agent,
        message,
        system_reminder,
        display_role,
        status_detail,
        swarm,
        publication,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "startup reserves publication before exposing the recovered runtime"
)]
pub(super) async fn spawn_tracked_live_turn_with_publication(
    session_id: &str,
    mut agent: OwnedMutexGuard<Agent>,
    message: String,
    system_reminder: Option<String>,
    display_role: Option<crate::session::StoredDisplayRole>,
    status_detail: Option<String>,
    swarm: LiveTurnSwarmContext,
    publication: Arc<super::swarm::TurnPublication>,
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
    let mut accepted = false;
    publication
        .publish_start(async {
            if !swarm
                .members
                .read()
                .await
                .get(session_id)
                .is_some_and(|member| {
                    member_accepts_automatic_turn(member)
                        || swarm.recovery_reload_id.is_some()
                            && super::reload_recovery::has_pending_for_session(session_id)
                })
            {
                return;
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
            super::swarm_persistence::persist_member_lifecycle(session_id, &swarm.members).await;
            accepted = true;
        })
        .await;
    if !accepted {
        return false;
    }

    let (fallback, _) = mpsc::unbounded_channel();
    let events = super::state::TurnEventChannel::new(
        session_id.to_string(),
        Arc::clone(&swarm.members),
        fallback,
        &turn_lease,
    );
    let event_tx = events.sender();
    let session_id = session_id.to_string();
    let agent_arc = OwnedMutexGuard::mutex(&agent).clone();
    crate::runtime_activity::spawn_session(&session_id.clone(), async move {
        let start_message_index = agent.message_count();
        let resume_reminder = system_reminder.clone();
        let pending_queue = agent.soft_interrupt_queue();
        let signal = agent.graceful_shutdown_signal();
        let (result, stop_reason) = catch_live_turn_panic(async {
            publication
                .run(
                    signal,
                    process_reserved_locked_context_with_server_failover(
                        &mut agent,
                        &turn_lease,
                        &message,
                        system_reminder,
                        display_role.unwrap_or(crate::session::StoredDisplayRole::System),
                        event_tx.clone(),
                    ),
                )
                .await
                .unwrap_or(Ok(()))
        })
        .await;
        let completion_report = result
            .is_ok()
            .then(|| agent.latest_assistant_text_after(start_message_index))
            .flatten();
        // A usage limit that resets later is a pause, not a failure: wait for
        // the reset and continue the same turn (bounded, see
        // usage_limit_resume). Other errors and panics fail as before.
        let (result, completion_report, mut reservation) = match result {
            Err(error) if stop_reason == crate::protocol::TurnStopReason::Failure => {
                match super::usage_limit_resume::resume_turn_after_usage_limit_with_publication(
                    agent_arc,
                    Some(agent),
                    &session_id,
                    error,
                    resume_reminder,
                    &event_tx,
                    &swarm,
                    &|plan| swarm.on_wait(plan),
                    Some(&publication),
                    Some(&turn_lease),
                )
                .await
                {
                    ResumeOutcome::NotUsageLimit { error, guard } => {
                        (Err((error, None)), None, guard)
                    }
                    ResumeOutcome::Failed {
                        error,
                        retry_after_secs,
                        guard,
                    } => (Err((error, retry_after_secs)), None, guard),
                    ResumeOutcome::Completed {
                        guard,
                        completion_report,
                    } => (Ok(()), completion_report, Some(guard)),
                    ResumeOutcome::Superseded => return,
                }
            }
            Err(error) => {
                let retry_after_secs = super::usage_limit_resume::error_retry_after_secs(&error);
                (Err((error, retry_after_secs)), None, Some(agent))
            }
            Ok(()) => (Ok(()), completion_report, Some(agent)),
        };
        // Keep the reservation until after the terminal status is published.
        // Releasing it earlier lets a follow-up wake reserve the agent and
        // publish `running`, which this turn's later `ready`/`failed` would
        // then overwrite, hiding the newer turn and suppressing its
        // coordinator completion notification.
        let pending = pending_queue.lock().map(|queue| queue.len()).unwrap_or(0);
        if pending > 0 {
            let _ = event_tx.send(ServerEvent::SoftInterruptQueue { pending });
        }
        if let (Err((error, _)), Some(agent)) = (&result, reservation.as_mut()) {
            let message = crate::util::format_error_chain(error);
            publication
                .publish_start(async {
                    if let Err(persist_error) = agent.persist_turn_error(&message) {
                        crate::logging::warn(&format!(
                            "Could not persist failed turn for {session_id}: {persist_error:#}"
                        ));
                    }
                })
                .await;
        }
        complete_live_turn(
            &session_id,
            &publication,
            result,
            completion_report,
            stop_reason,
            &event_tx,
            &swarm,
        )
        .await;
        drop(reservation);
        events.finish().await;
        drop(turn_lease);
    });
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn complete_live_turn(
    session_id: &str,
    publication: &super::swarm::TurnPublication,
    result: Result<(), (anyhow::Error, Option<u64>)>,
    completion_report: Option<String>,
    stop_reason: crate::protocol::TurnStopReason,
    event_tx: &tokio::sync::mpsc::UnboundedSender<ServerEvent>,
    swarm: &LiveTurnSwarmContext,
) {
    publication
        .complete(async {
            if let Some(reload_id) = &swarm.recovery_reload_id {
                super::reload_trace::record_value(
                    reload_id,
                    if result.is_ok() {
                        "continuation_finished"
                    } else {
                        "continuation_failed"
                    },
                    serde_json::json!({
                        "session_id": session_id,
                        "source": "server_startup_headless",
                        "status": if result.is_ok() { "ready" } else { "failed" },
                        "error": result.as_ref().err().map(|(error, _)| error.to_string()),
                    }),
                );
            }
            if let Some(assignment) = &swarm.recovered_assignment {
                let error = result.as_ref().err().map(|(error, _)| error.to_string());
                assignment.complete(result.is_ok(), error.as_deref()).await;
            }
            match result {
                Ok(()) => {
                    update_member_status_with_report(
                        session_id,
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
                Err((error, retry_after_secs)) => {
                    crate::logging::error(&format!(
                        "Server-initiated turn failed for live session {}: {}",
                        session_id, error
                    ));
                    update_member_status(
                        session_id,
                        "failed",
                        Some(truncate_detail(&error.to_string(), 120)),
                        &swarm.members,
                        &swarm.swarms_by_id,
                        Some(&swarm.event_history),
                        Some(&swarm.event_counter),
                        Some(&swarm.event_tx),
                    )
                    .await;
                    let _ = event_tx.send(ServerEvent::TurnStopped {
                        reason: stop_reason,
                        message: crate::util::format_error_chain(&error),
                        provider_stop_reason: None,
                    });
                    let _ = event_tx.send(ServerEvent::Error {
                        id: 0,
                        message: crate::util::format_error_chain(&error),
                        retry_after_secs,
                        server_resumes: false,
                    });
                }
            }
            if let Some(state) = &swarm.recovery_swarm {
                let swarm_id = state
                    .members
                    .read()
                    .await
                    .get(session_id)
                    .and_then(|member| member.swarm_id.clone());
                if let Some(swarm_id) = swarm_id {
                    super::persist_swarm_state_for(&swarm_id, state).await;
                }
            } else {
                super::swarm_persistence::persist_member_lifecycle(session_id, &swarm.members)
                    .await;
            }
        })
        .await;
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
#[path = "live_context_tests.rs"]
mod context_tests;

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
        let mut wake_entries = 0;
        let error_message = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = event_rx.recv().await.expect("live turn event stream");
                let payload = match event {
                    ServerEvent::TurnEvent { event, .. } => *event,
                    event => event,
                };
                match payload {
                    ServerEvent::SoftInterruptInjected {
                        content,
                        display_role,
                        ..
                    } => {
                        assert_eq!(content, "wake worker");
                        assert_eq!(display_role.as_deref(), Some("system"));
                        assert!(!stopped, "input delivery must precede terminal events");
                        wake_entries += 1;
                    }
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
        assert_eq!(wake_entries, 1);
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
