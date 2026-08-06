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

use super::client_lifecycle::process_message_streaming_mpsc;
use super::{
    SwarmEvent, SwarmMember, session_event_fanout_sender, truncate_detail, update_member_status,
    update_member_status_with_report,
};
use crate::agent::Agent;
use crate::protocol::ServerEvent;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::{Mutex, RwLock, broadcast};

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

/// Return the live agent for `session_id` when the session has at least one
/// live client attachment and its agent is currently idle (lock not held).
pub(super) async fn idle_live_agent(
    session_id: &str,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<Arc<Mutex<Agent>>> {
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

    let is_idle = agent.try_lock().is_ok();
    is_idle.then_some(agent)
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
    agent: Arc<Mutex<Agent>>,
    message: String,
    system_reminder: Option<String>,
    display_role: Option<crate::session::StoredDisplayRole>,
    status_detail: Option<String>,
    swarm: LiveTurnSwarmContext,
) {
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
        let start_message_index = {
            let agent_guard = agent.lock().await;
            agent_guard.message_count()
        };
        let result = if let Some(display_role) = display_role {
            let mut agent = agent.lock().await;
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
            process_message_streaming_mpsc(
                Arc::clone(&agent),
                &message,
                vec![],
                system_reminder,
                event_tx.clone(),
            )
            .await
        };
        match result {
            Ok(()) => {
                let completion_report = {
                    let agent_guard = agent.lock().await;
                    agent_guard.latest_assistant_text_after(start_message_index)
                };
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
                crate::logging::error(&format!(
                    "Server-initiated turn failed for live session {}: {}",
                    session_id, error
                ));
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
                let _ = event_tx.send(ServerEvent::Error {
                    id: 0,
                    message: crate::util::format_error_chain(&error),
                    retry_after_secs: None,
                });
            }
        }
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
