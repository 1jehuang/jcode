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
//!
//! A turn that finishes while its member is still `stopping` keeps that marker
//! instead of publishing its own end status: the stop timed out waiting for
//! this turn, so the worker must not be advertised as reusable.
//!
//! It owns the stop side of the same reservation. A `swarm stop` must cancel
//! the live turn and clear its queued work without letting a new turn start in
//! between, so quiescing a session for coordination reuses that reservation
//! instead of duplicating the wake-turn bookkeeping.

use super::client_lifecycle::process_locked_message_streaming_mpsc;
use super::state::begin_session_interrupt_stop;
use super::swarm_mutation_state::PersistedSwarmMutationResponse;
use super::{
    ChannelSubscriptions, SessionControlHandle, SessionInterruptQueues, SwarmEvent, SwarmEventType,
    SwarmMember, VersionedPlan, begin_session_interrupt_delivery, record_swarm_event,
    remove_background_tool_signal, remove_session_channel_subscriptions, remove_session_from_swarm,
    remove_session_interrupt_queue, session_event_fanout_sender, truncate_detail,
    update_member_status, update_member_status_with_report,
};
use crate::agent::Agent;
use crate::protocol::ServerEvent;
use futures::FutureExt;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex as StdMutex};
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
    // Reserve against stop before looking up or locking the Agent. A stop that
    // starts after this point waits for the reservation to finish, then keeps
    // re-firing cancellation until the reserved turn registers its signal.
    let _reservation = begin_session_interrupt_delivery(session_id)?;
    let agent = {
        let guard = sessions.read().await;
        guard.get(session_id).cloned()
    }?;

    let has_live_attachments = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .map(|member| !member.event_txs.is_empty() || !member.event_tx.is_closed())
            .unwrap_or(false)
    };
    if !has_live_attachments {
        return None;
    }

    agent.try_lock_owned().ok()
}

/// Admission for a client-initiated turn on a session the client holds a handle
/// to.
///
/// A client keeps its own `Arc<Mutex<Agent>>` after attaching, so a stop that
/// closed the Agent and removed the session from routing does not detach that
/// handle: without this check a later client turn would dispatch provider work
/// for a stopped session, outside swarm membership and routing. Admission runs
/// under the same lock the turn would take, before the message is recorded or
/// run, and it owns the skill validation so both rejections are decided in one
/// lock acquisition.
///
/// Returns the rejection to send back when the turn is refused, or `None` when
/// it is admitted. The caller sends the event; admission itself never touches
/// the client channel.
pub(super) async fn admit_client_turn(
    agent: &Arc<Mutex<Agent>>,
    active_skill: Option<String>,
    id: u64,
) -> Option<ServerEvent> {
    let mut agent = agent.lock().await;
    if agent.is_closed() {
        return Some(ServerEvent::Error {
            id,
            message: "This session was stopped and no longer accepts new turns".to_string(),
            retry_after_secs: None,
        });
    }
    if agent.set_remote_active_skill(active_skill.clone()) {
        return None;
    }
    let skill_name = active_skill.as_deref().unwrap_or("");
    Some(ServerEvent::Error {
        id,
        message: format!("Skill '{skill_name}' is not installed on the server"),
        retry_after_secs: None,
    })
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
) {
    // Re-check after reservation. Stop can begin after `idle_live_agent`
    // returns but before this function receives its guard; in that window the
    // reserved wake must be discarded rather than starting a provider turn.
    let Some(_delivery) = begin_session_interrupt_delivery(session_id) else {
        return;
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

    let event_tx = session_event_fanout_sender(session_id.to_string(), Arc::clone(&swarm.members));
    let session_id = session_id.to_string();
    tokio::spawn(async move {
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
                process_locked_message_streaming_mpsc(
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
        let completion_report = result
            .is_ok()
            .then(|| agent.latest_assistant_text_after(start_message_index))
            .flatten();
        // Keep the reservation until after the terminal status is published.
        // Releasing it earlier lets a follow-up wake reserve the agent and
        // publish `running`, which this turn's later `ready`/`failed` would
        // then overwrite, hiding the newer turn and suppressing its
        // coordinator completion notification.
        let reservation = agent;
        let pending = pending_queue.lock().map(|queue| queue.len()).unwrap_or(0);
        if pending > 0 {
            let _ = event_tx.send(ServerEvent::SoftInterruptQueue { pending });
        }
        // A stop that could not quiesce this turn within its timeout left the
        // member in `stopping` and told the caller not to schedule replacement
        // work. This turn is the work that outlived that timeout, so its own end
        // status must not overwrite the marker: publishing `ready` (with a
        // coordinator completion report) or `failed` here would advertise a
        // worker that is still stopping as reusable.
        let quiescing = member_is_stopping(&session_id, &swarm.members).await;
        match result {
            Ok(()) => {
                if !quiescing {
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
                }
                let _ = event_tx.send(ServerEvent::Done { id: 0 });
            }
            Err(error) => {
                crate::logging::error(&format!(
                    "Server-initiated turn failed for live session {}: {}",
                    session_id, error
                ));
                if !quiescing {
                    update_member_status(
                        &session_id,
                        "failed",
                        Some(truncate_detail(&error.to_string(), 120)),
                        &swarm.members,
                        &swarm.swarms_by_id,
                        Some(&swarm.event_history),
                        Some(&swarm.event_counter),
                        Some(&swarm.event_tx),
                    )
                    .await;
                }
                let _ = event_tx.send(ServerEvent::TurnStopped {
                    reason: stop_reason,
                    message: crate::util::format_error_chain(&error),
                    provider_stop_reason: None,
                });
                let _ = event_tx.send(ServerEvent::Error {
                    id: 0,
                    message: crate::util::format_error_chain(&error),
                    retry_after_secs: None,
                });
            }
        }
        drop(reservation);
    });
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
    .await;
    true
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
    .await;
    true
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

/// Quiesce `target_session` and remove it from every routing table.
///
/// Returns the mutation response for the stop and the Agent guard, which the
/// caller must keep alive until that response has been persisted and delivered:
/// releasing it earlier would let a new turn start while the caller is still
/// reporting the stop. A failed stop returns an `Error` response instead, and
/// leaves the target resolvable in `stopping` state so a retry resumes where
/// this attempt stopped.
#[allow(clippy::too_many_arguments)]
pub(super) async fn quiesce_and_stop_session(
    target_session: &str,
    req_session_id: &str,
    sessions: &SessionAgents,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: &Arc<AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) -> (
    PersistedSwarmMutationResponse,
    Option<OwnedMutexGuard<Agent>>,
) {
    // Publish `stopping` before any cancellation: the status is the recoverable
    // marker for a stop that has not quiesced yet, and it keeps a partially
    // stopped member from being treated as an ordinary live worker.
    update_member_status(
        target_session,
        "stopping",
        Some(format!("Cancellation requested by {req_session_id}")),
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;

    // Close interrupt delivery before clearing. Producers already admitted drain
    // first, and later live or persisted producers are refused until an explicit
    // resume reopens the session lifecycle.
    begin_session_interrupt_stop(target_session).await;
    let live_agent = sessions.read().await.get(target_session).cloned();
    let soft_interrupt_queue = soft_interrupt_queues
        .read()
        .await
        .get(target_session)
        .cloned()
        .unwrap_or_else(|| Arc::new(StdMutex::new(Vec::new())));
    // A lock-free handle is enough: `request_cancel` fires the registered
    // in-flight turn signals for the session, which is what reaches a turn this
    // connection never locked.
    let session_control = SessionControlHandle::cancel_only(
        target_session,
        soft_interrupt_queue.clone(),
        InterruptSignal::new(),
    );
    if let Err(error) = clear_live_stop_queue(&soft_interrupt_queue) {
        crate::logging::warn(&format!(
            "Initial live soft-interrupt clear failed while stopping session {target_session}: {error}"
        ));
    }
    if let Err(error) = clear_persisted_stop_queue(target_session) {
        crate::logging::warn(&format!(
            "Initial persisted soft-interrupt clear failed while stopping session {target_session}: {error}"
        ));
    }

    // Wait for the running turn to release the Agent while re-firing
    // cancellation: a turn that registers its cancellation signal after stop
    // began must not miss it. On timeout the target stays resolvable in
    // `stopping` state so a later stop can retry; it is already cancelled, so
    // no replacement work should be scheduled yet.
    let mut terminal_agent = match acquire_terminal_agent_guard(live_agent, &session_control).await
    {
        Ok(agent) => agent,
        Err(()) => {
            return (
                PersistedSwarmMutationResponse::Error {
                    message: format!(
                        "Cancellation was requested for session '{target_session}', but its active turn did not stop within 5 seconds. The target remains in stopping state and may be stopped again; do not schedule replacement work yet."
                    ),
                    retry_after_secs: None,
                },
                None,
            );
        }
    };

    // Delivery is still closed and every earlier producer has drained, so these
    // verified clears are stable through routing removal and the final response.
    if let Err(error) = clear_live_stop_queue(&soft_interrupt_queue) {
        return (
            PersistedSwarmMutationResponse::Error {
                message: format!(
                    "Session '{target_session}' quiesced, but queued live work could not be cleared: {error}. The target remains retryable in stopping state."
                ),
                retry_after_secs: None,
            },
            None,
        );
    }
    if let Err(error) = clear_persisted_stop_queue(target_session) {
        return (
            PersistedSwarmMutationResponse::Error {
                message: format!(
                    "Session '{target_session}' quiesced, but persisted queued work could not be cleared: {error}. The target remains retryable in stopping state."
                ),
                retry_after_secs: None,
            },
            None,
        );
    }

    let extraction = terminal_agent.as_mut().and_then(|agent| {
        agent.mark_closed();
        agent.memory_enabled().then(|| {
            (
                agent.build_transcript_for_extraction(),
                target_session.to_string(),
                agent.working_dir().map(str::to_string),
            )
        })
    });

    let removed_live_agent = super::remove_session_entry(sessions, target_session)
        .await
        .is_some();
    remove_session_interrupt_queue(soft_interrupt_queues, target_session).await;
    remove_background_tool_signal(target_session);

    let (removed_swarm_id, removed_name) = {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.remove(target_session) {
            (member.swarm_id, member.friendly_name)
        } else {
            (None, None)
        }
    };
    if let Some(ref swarm_id) = removed_swarm_id {
        record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            target_session.to_string(),
            removed_name.clone(),
            Some(swarm_id.clone()),
            SwarmEventType::MemberChange {
                action: "left".to_string(),
            },
        )
        .await;
        remove_session_from_swarm(
            target_session,
            swarm_id,
            swarm_members,
            swarms_by_id,
            swarm_coordinators,
            swarm_plans,
        )
        .await;
    }
    remove_session_channel_subscriptions(
        target_session,
        channel_subscriptions,
        channel_subscriptions_by_session,
    )
    .await;

    if let Some((transcript, session_id, working_dir)) = extraction {
        crate::memory_agent::trigger_final_extraction_with_dir(transcript, session_id, working_dir);
    }

    let response = if removed_live_agent || removed_swarm_id.is_some() {
        PersistedSwarmMutationResponse::Done
    } else {
        PersistedSwarmMutationResponse::Error {
            message: format!("Unknown session '{target_session}'"),
            retry_after_secs: None,
        }
    };
    (response, terminal_agent)
}

/// Remove every persisted soft interrupt queued for `session_id`, then verify
/// that the store is empty.
fn clear_persisted_stop_queue(session_id: &str) -> anyhow::Result<()> {
    crate::soft_interrupt_store::clear(session_id)?;
    let remaining = crate::soft_interrupt_store::load(session_id)?;
    anyhow::ensure!(
        remaining.is_empty(),
        "{} queued soft interrupt(s) remain after clear",
        remaining.len()
    );
    Ok(())
}

/// Empty the live soft-interrupt queue, then verify that it stayed empty.
fn clear_live_stop_queue(queue: &jcode_agent_runtime::SoftInterruptQueue) -> anyhow::Result<()> {
    let mut pending = queue
        .lock()
        .map_err(|_| anyhow::anyhow!("live soft-interrupt queue lock is poisoned"))?;
    pending.clear();
    anyhow::ensure!(
        pending.is_empty(),
        "live soft-interrupt queue remains nonempty"
    );
    Ok(())
}

/// Whether a stop has begun for `session_id` and has not quiesced it yet.
///
/// A stop publishes `stopping` before it cancels anything and only clears it
/// (by removing the member) once the turn released the Agent. A turn that
/// finishes while the marker is still set therefore ended after a stop timed
/// out, and must leave the marker alone.
async fn member_is_stopping(
    session_id: &str,
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    let members = members.read().await;
    members
        .get(session_id)
        .is_some_and(|member| member.status == "stopping")
}

/// Acquire the terminal Agent guard for a session being stopped.
///
/// Returns `Err(())` when the active turn does not release the Agent within
/// five seconds, in which case the caller leaves the target in `stopping` state
/// so a later stop can retry.
async fn acquire_terminal_agent_guard(
    agent: Option<Arc<Mutex<Agent>>>,
    session_control: &SessionControlHandle,
) -> std::result::Result<Option<tokio::sync::OwnedMutexGuard<Agent>>, ()> {
    let Some(agent) = agent else {
        return Ok(None);
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            // A live wake can reserve the Agent before its run_turn signal is
            // registered. Re-fire so that late registration cannot miss stop.
            session_control.request_cancel();
            if let Ok(agent) = Arc::clone(&agent).try_lock_owned() {
                return agent;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .map(Some)
    .map_err(|_| ())
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
