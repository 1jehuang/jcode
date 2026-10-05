#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::client_state::{handle_get_history, spawn_model_prefetch_update};
use super::{
    ClientConnectionInfo, ClientDebugState, FileTouchService, SessionInterruptQueues, SwarmEvent,
    SwarmMember, SwarmState, VersionedPlan, persist_swarm_state_for,
    register_background_tool_signal, register_session_event_sender,
    register_session_interrupt_queue, remove_background_tool_signal, remove_plan_participant,
    remove_session_channel_subscriptions, remove_session_interrupt_queue,
    rename_background_tool_signal, rename_plan_participant, rename_session_interrupt_queue,
    send_swarm_plan_to_session, swarm_id_for_session, update_member_status,
};
use crate::agent::Agent;
use crate::message::ContentBlock;
use crate::protocol::ServerEvent;
use crate::provider::Provider;
use crate::tool::Registry;
use crate::transport::WriteHalf;
use anyhow::Result;
use futures::FutureExt;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

use super::subscribe_working_dir::{
    cleanup_detached_source_session_if_unused, rename_swarm_member_session,
};
pub(super) type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
pub(super) type ChannelSubscriptions =
    Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;
const RELOAD_RESTORE_MARKER_MAX_AGE: Duration = Duration::from_secs(60);

pub(super) fn session_was_interrupted_by_reload(agent: &Agent) -> bool {
    let messages = agent.messages();
    let Some(last) = messages.last() else {
        return false;
    };

    last.content.iter().any(|block| match block {
        ContentBlock::Text { text, .. } => {
            text.ends_with("[generation interrupted - server reloading]")
        }
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            content == "Reload initiated. Process restarting..."
                || (is_error.unwrap_or(false)
                    && (content.contains("interrupted by server reload")
                        || content.contains("Skipped - server reloading")))
        }
        _ => false,
    })
}

pub(super) fn restored_session_was_interrupted(
    session_id: &str,
    previous_status: &crate::session::SessionStatus,
    agent: &Agent,
) -> bool {
    let last_is_user = agent
        .last_message_role()
        .as_ref()
        .map(|role| *role == crate::message::Role::User)
        .unwrap_or(false);
    let last_is_reload_interrupted = session_was_interrupted_by_reload(agent);
    let closed_pending_user_during_reload =
        matches!(previous_status, crate::session::SessionStatus::Closed)
            && last_is_user
            && crate::server::reload_marker_active(RELOAD_RESTORE_MARKER_MAX_AGE);

    if last_is_user && matches!(previous_status, crate::session::SessionStatus::Active) {
        crate::logging::info(&format!(
            "Session {} was Active with pending user message - treating as interrupted",
            session_id
        ));
    }

    if last_is_reload_interrupted {
        crate::logging::info(&format!(
            "Session {} contains reload interruption markers - will auto-resume",
            session_id
        ));
    }

    if closed_pending_user_during_reload {
        crate::logging::info(&format!(
            "Session {} was Closed with a pending user message during a recent reload - treating as interrupted",
            session_id
        ));
    }

    matches!(
        previous_status,
        crate::session::SessionStatus::Crashed { .. }
    ) || (matches!(previous_status, crate::session::SessionStatus::Active) && last_is_user)
        || last_is_reload_interrupted
        || closed_pending_user_during_reload
}

pub(super) fn mark_remote_reload_started(request_id: &str) {
    crate::server::write_reload_state(
        request_id,
        jcode_build_meta::version(),
        crate::server::ReloadPhase::Starting,
        None,
    );
}

async fn rename_shutdown_signal(
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    old_session_id: &str,
    new_session_id: &str,
) {
    if old_session_id == new_session_id {
        return;
    }

    let mut signals = shutdown_signals.write().await;
    if let Some(signal) = signals.remove(old_session_id) {
        signals.insert(new_session_id.to_string(), signal);
    }
    drop(signals);
    rename_background_tool_signal(old_session_id, new_session_id);
    // In-flight turns are registered in the process-global cancel registry by
    // session id. Attaching to / resuming a session renames it underneath a
    // still-streaming turn, so the registration must follow, or a later Esc
    // finds no active-turn signal for the new id and the model keeps
    // generating (issue #732, regression of issue #428).
    crate::turn_cancel_registry::rename_active_turns(old_session_id, new_session_id);
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_clear_session(
    id: u64,
    client_selfdev: bool,
    client_session_id: &mut String,
    client_connection_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &SessionAgents,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let clear_start = Instant::now();
    let old_session_id = client_session_id.clone();
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "clear_start".to_string()),
            ("request_id", id.to_string()),
            ("session_id", old_session_id.clone()),
            ("client_connection_id", client_connection_id.to_string()),
            ("client_selfdev", client_selfdev.to_string()),
        ],
    );
    let (preserve_debug, working_dir) = {
        let agent_guard = agent.lock().await;
        (
            agent_guard.is_debug(),
            agent_guard.working_dir().map(str::to_string),
        )
    };

    {
        let mut agent_guard = agent.lock().await;
        agent_guard.mark_closed();
    }

    let mut new_agent = Agent::new_with_initial_working_dir(
        Arc::clone(provider),
        registry.clone(),
        working_dir.as_deref(),
    );
    let new_id = new_agent.session_id().to_string();

    if client_selfdev {
        new_agent.set_canary("self-dev");
    }
    if preserve_debug {
        new_agent.set_debug(true);
    }

    let mut agent_guard = agent.lock().await;
    *agent_guard = new_agent;
    drop(agent_guard);

    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.remove(client_session_id);
        sessions_guard.insert(new_id.clone(), Arc::clone(agent));
    }
    crate::runtime_memory_log::emit_event(
        crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
            "session_cleared",
            "session_replaced_with_fresh_agent",
        )
        .with_session_id(new_id.clone())
        .force_attribution(),
    );
    {
        let agent_guard = agent.lock().await;
        register_session_interrupt_queue(
            soft_interrupt_queues,
            &new_id,
            agent_guard.soft_interrupt_queue(),
        )
        .await;

        let mut signals = shutdown_signals.write().await;
        signals.remove(client_session_id);
        signals.insert(new_id.clone(), agent_guard.graceful_shutdown_signal());
        drop(signals);
        remove_background_tool_signal(client_session_id);
        register_background_tool_signal(&new_id, agent_guard.background_tool_signal());
    }
    remove_session_interrupt_queue(soft_interrupt_queues, client_session_id).await;

    // `/clear` creates a genuinely fresh session. Do not migrate the old
    // session's swarm membership or plan participation to the replacement:
    // doing so lets a subsequent plan snapshot repopulate the cleared UI.
    let (swarm_id_for_update, swarm_enabled, friendly_name) = {
        let mut members = swarm_members.write().await;
        match members.remove(client_session_id) {
            Some(member) => (member.swarm_id, member.swarm_enabled, member.friendly_name),
            None => (None, false, None),
        }
    };
    if let Some(ref swarm_id) = swarm_id_for_update {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(swarm_id) {
            swarm.remove(client_session_id);
            if swarm.is_empty() {
                swarms.remove(swarm_id);
            }
        }
    }
    file_touch.clear_session(client_session_id).await;
    remove_session_channel_subscriptions(
        client_session_id,
        channel_subscriptions,
        channel_subscriptions_by_session,
    )
    .await;
    // The connection remains subscribed across `/clear`, so there is no later
    // subscribe request to register the replacement session. Register it as a
    // fresh root while deliberately leaving the old swarm and plan behind.
    ensure_client_swarm_member(
        &new_id,
        client_connection_id,
        &friendly_name,
        client_event_tx,
        agent,
        swarm_enabled,
        swarm_members,
        swarms_by_id,
        event_history,
        event_counter,
        swarm_event_tx,
    )
    .await;
    update_member_status(
        &new_id,
        "ready",
        None,
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;
    if let Some(ref swarm_id) = swarm_id_for_update {
        remove_plan_participant(swarm_id, client_session_id, swarm_plans).await;
    }

    *client_session_id = new_id.clone();
    {
        let mut connections = client_connections.write().await;
        if let Some(info) = connections.get_mut(client_connection_id) {
            info.session_id = new_id.clone();
            info.last_seen = Instant::now();
        }
    }
    let _ = client_event_tx.send(ServerEvent::SessionId { session_id: new_id });
    let _ = client_event_tx.send(ServerEvent::Done { id });
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "clear_done".to_string()),
            ("request_id", id.to_string()),
            ("old_session_id", old_session_id),
            ("new_session_id", client_session_id.clone()),
            ("client_connection_id", client_connection_id.to_string()),
            ("preserve_debug", preserve_debug.to_string()),
            (
                "swarm_id_updated",
                swarm_id_for_update.is_some().to_string(),
            ),
            ("elapsed_ms", clear_start.elapsed().as_millis().to_string()),
        ],
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn ensure_client_swarm_member(
    client_session_id: &str,
    client_connection_id: &str,
    friendly_name: &Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    agent: &Arc<Mutex<Agent>>,
    swarm_enabled: bool,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) -> bool {
    let (working_dir, derived_swarm_id, fallback_name) = {
        // A target-aware subscribe can attach to an agent that is in the middle
        // of a turn. Never wait for that turn's agent lock just to populate
        // connection metadata: doing so prevents the subscribe request from
        // completing, so subsequent state requests sit unread until the desktop
        // client times out. The persisted startup stub has the same immutable
        // identity metadata and is safe to read while the live agent is busy.
        let (working_dir, fallback_name) = match agent.try_lock() {
            Ok(agent_guard) => (
                agent_guard.working_dir().map(PathBuf::from),
                agent_guard
                    .session_short_name()
                    .map(|value| value.to_string()),
            ),
            Err(_) => {
                crate::logging::info(&format!(
                    "Subscribe metadata for busy session {} is using the persisted startup stub",
                    client_session_id
                ));
                crate::session::Session::load_startup_stub(client_session_id)
                    .map(|session| (session.working_dir.map(PathBuf::from), session.short_name))
                    .unwrap_or((None, None))
            }
        };
        let derived_swarm_id = if swarm_enabled {
            swarm_id_for_session(client_session_id)
        } else {
            None
        };
        (working_dir, derived_swarm_id, fallback_name)
    };

    // Prefer the currently restored agent/session identity over the temporary
    // name captured at raw socket accept time. During resume/reconnect bursts,
    // the temporary pre-resume session name can otherwise leak onto the real
    // resumed session and corrupt swarm metadata.
    let member_name = fallback_name.or_else(|| friendly_name.clone());
    let mut inserted = false;
    {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(client_session_id) {
            member.event_tx = client_event_tx.clone();
            member
                .event_txs
                .insert(client_connection_id.to_string(), client_event_tx.clone());
            member.swarm_enabled = swarm_enabled;
            member.is_headless = false;
            if member_name.is_some() {
                member.friendly_name = member_name.clone();
            }
        } else {
            let now = Instant::now();
            members.insert(
                client_session_id.to_string(),
                SwarmMember {
                    session_id: client_session_id.to_string(),
                    event_tx: client_event_tx.clone(),
                    event_txs: HashMap::from([(
                        client_connection_id.to_string(),
                        client_event_tx.clone(),
                    )]),
                    working_dir: working_dir.clone(),
                    swarm_id: derived_swarm_id.clone(),
                    swarm_enabled,
                    status: "ready".to_string(),
                    detail: None,
                    task_label: None,
                    friendly_name: member_name.clone(),
                    report_back_to_session_id: None,
                    latest_completion_report: None,
                    role: "agent".to_string(),
                    joined_at: now,
                    last_status_change: now,
                    is_headless: false,
                    output_tail: None,
                    todo_progress: None,
                    todo_items: Vec::new(),
                    runtime: crate::protocol::SwarmMemberRuntime::default(),
                },
            );
            inserted = true;
        }
    }

    if inserted && let Some(ref swarm_id_ref) = derived_swarm_id {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(swarm_id_ref.to_string())
            .or_insert_with(HashSet::new)
            .insert(client_session_id.to_string());
        drop(swarms);
        super::record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            client_session_id.to_string(),
            member_name,
            Some(swarm_id_ref.to_string()),
            crate::server::SwarmEventType::MemberChange {
                action: "joined".to_string(),
            },
        )
        .await;
    }

    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "swarm_member_registered".to_string()),
            ("session_id", client_session_id.to_string()),
            ("client_connection_id", client_connection_id.to_string()),
            ("inserted", inserted.to_string()),
            ("swarm_enabled", swarm_enabled.to_string()),
            (
                "swarm_id",
                derived_swarm_id.unwrap_or_else(|| "none".to_string()),
            ),
        ],
    );

    inserted
}

/// Resolve the working directory a subscribe should actually bind to.
///
/// Returns the reported dir when it is acceptable, or the session's existing
/// dir when the report is rejected by [`subscribe_working_dir_replacement`].
/// Every consumer of a subscribe cwd (agent state, swarm id, project-local MCP
/// resolution) must agree on this one answer, otherwise the session's tools,
/// swarm grouping, and MCP config can each resolve against a different
/// directory (issue #481).

/// Atomically reserves an existing live target for this connection.
///
/// Reserving under the connection write lock prevents another connection's
/// detached-source cleanup from observing no users after we have selected the
/// target but before our connection record is updated.
async fn claim_live_target_agent(
    session_id: &str,
    client_connection_id: &str,
    client_instance_id: Option<&str>,
    source_agent: &Arc<Mutex<Agent>>,
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
) -> Option<Arc<Mutex<Agent>>> {
    let mut connections = client_connections.write().await;
    let sessions_guard = sessions.read().await;
    let target = sessions_guard
        .get(session_id)
        .filter(|existing| !Arc::ptr_eq(existing, source_agent))
        .cloned()?;
    // A session that migrated back from another machine has a newer transcript
    // on disk than this live agent. Never reattach to the stale copy. The
    // caller then restores from disk and replaces the map entry.
    if target
        .try_lock()
        .is_ok_and(|agent| agent.session_copy_is_stale())
    {
        crate::logging::info(&format!(
            "Resume of {session_id}: live agent is older than the migrated transcript on disk; reloading"
        ));
        return None;
    }

    let info = connections.get_mut(client_connection_id)?;
    info.session_id = session_id.to_string();
    info.client_instance_id = client_instance_id.map(str::to_string);
    info.last_seen = Instant::now();
    Some(target)
}

/// Decide which working directory a client-reported directory may bind an
/// existing session to.
///
/// This is the working-dir counterpart of the rule the request handler already
/// applies to a system prompt: "overrides are creation-only. In particular, never
/// apply one to a target attachment." The system-prompt path got that rule; the
/// working-dir path did not, so a client attaching to a session could still move
/// it.
///
/// The rule is scoped to session *creation* on purpose. `create_working_dir`
/// distinguishes the two cases: at creation the reported directory is the only
/// description of the project the user is working in and must be adopted, while a
/// directory reported later describes whichever client happened to reconnect. A
/// client cannot deliberately retarget an existing session's project, because the
/// wire protocol has no request that means "move this session to another project"
/// (the only requests carrying a `working_dir` are Subscribe, CommSpawn, and
/// spawn-agent, and all three either create a session or attach to one). Making a
/// deliberate project move possible later means adding an explicit request for it,
/// not letting an ordinary reconnect imply one.
///
/// A session belongs to a project. A client's directory describes the project
/// that *client* is sitting in, not the project the session belongs to.
/// Overwriting re-points that session's bash and file tools, its project-local
/// MCP config, its memory scope, and its swarm grouping at the client's project,
/// so a later message in the original project would read and write the wrong tree
/// while the header still showed the original path.
///
/// The session's stored directory therefore wins whenever it has one. A client's
/// directory is adopted only when the session has none to lose, which is the
/// creation case (including remote continuation, where the client has no local
/// copy of the session it is resuming).
///
/// Both sides are canonicalized before comparing, so a symlinked checkout, a
/// `..` segment, or a Windows short/long path pair is recognized as the same
/// project instead of looking like a cross-project attach.
pub(super) fn session_working_dir_for_client(
    session_dir: Option<&str>,
    client_dir: Option<&str>,
    create_working_dir: bool,
) -> Option<String> {
    let session = session_dir.map(str::trim).filter(|dir| !dir.is_empty());
    let client = client_dir.map(str::trim).filter(|dir| !dir.is_empty());
    match (session, client) {
        (Some(session), _) => Some(session.to_string()),
        (None, Some(client)) if create_working_dir => Some(client.to_string()),
        (None, _) => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_resume_session(
    id: u64,
    session_id: String,
    working_dir_override: Option<&str>,
    client_instance_id: Option<&str>,
    client_has_local_history: bool,
    allow_session_takeover: bool,
    client_selfdev: &mut bool,
    client_session_id: &mut String,
    client_connection_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &SessionAgents,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    supports_pdf_panels: bool,
) -> Result<Arc<Mutex<Agent>>> {
    let resume_start = Instant::now();
    let incoming_client_instance_id = client_instance_id.map(str::to_string);
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "resume_start".to_string()),
            ("request_id", id.to_string()),
            ("source_session_id", client_session_id.clone()),
            ("target_session_id", session_id.clone()),
            ("client_connection_id", client_connection_id.to_string()),
            (
                "client_instance_id",
                incoming_client_instance_id
                    .clone()
                    .unwrap_or_else(|| "none".to_string()),
            ),
            (
                "client_has_local_history",
                client_has_local_history.to_string(),
            ),
            ("allow_takeover", allow_session_takeover.to_string()),
        ],
    );
    let live_target_agent = claim_live_target_agent(
        &session_id,
        client_connection_id,
        incoming_client_instance_id.as_deref(),
        agent,
        sessions,
        client_connections,
    )
    .await;

    // Resolve the directory this resume will bind to ONCE, and use that single
    // answer for every consumer below (the restored session, project-local MCP
    // resolution, and the swarm member). Two subsystems choosing different answers
    // is the isolation bug: the agent could stay in project B while MCP discovery
    // ran against project A.
    //
    // The target's own directory is read without blocking on the agent mutex: a
    // generating target owns its lock, and awaiting it here would deadlock the
    // attach behind a model turn. Fall back to the on-disk copy for a session that
    // is not live in memory.
    let bound_working_dir = {
        let target_dir = live_target_agent
            .as_ref()
            .and_then(|live| live.try_lock().ok())
            .and_then(|agent_guard| agent_guard.working_dir().map(str::to_string))
            .or_else(|| {
                crate::session::Session::load_startup_stub(&session_id)
                    .ok()
                    .and_then(|session| session.working_dir)
            });
        session_working_dir_for_client(target_dir.as_deref(), working_dir_override, false)
    };
    if let (Some(target), Some(subscriber)) = (
        bound_working_dir.as_deref(),
        working_dir_override
            .map(str::trim)
            .filter(|dir| !dir.is_empty()),
    ) && super::util::canonicalize_or(target.into())
        != super::util::canonicalize_or(subscriber.into())
    {
        crate::logging::warn(&format!(
            "Preserving session {} working_dir {target}: a subscriber in {subscriber} attached to it (cross-project attach)",
            session_id
        ));
    }

    if let Some(live_target_agent) = live_target_agent.as_ref() {
        let old_session_id = client_session_id.clone();

        let conflicting_live_client = {
            let connections = client_connections.read().await;
            connections
                .values()
                .find(|info| {
                    info.client_id != client_connection_id && info.session_id == session_id
                })
                .cloned()
        };
        let live_target_busy = live_target_agent.try_lock().is_err();
        crate::logging::info(&format!(
            "Resume attach to existing live session {} from temporary {} on connection {}: live_target_busy={}, conflict_owner={}, conflict_processing={}, allow_takeover={}, local_history={}, incoming_instance={:?}",
            session_id,
            old_session_id,
            client_connection_id,
            live_target_busy,
            conflicting_live_client
                .as_ref()
                .map(|info| info.client_id.as_str())
                .unwrap_or("<none>"),
            conflicting_live_client
                .as_ref()
                .map(|info| info.is_processing)
                .unwrap_or(false),
            allow_session_takeover,
            client_has_local_history,
            incoming_client_instance_id
        ));

        cleanup_detached_source_session_if_unused(
            &old_session_id,
            client_connection_id,
            agent,
            sessions,
            shutdown_signals,
            soft_interrupt_queues,
            client_connections,
            swarm_members,
            swarms_by_id,
            file_touch,
            channel_subscriptions,
            channel_subscriptions_by_session,
            swarm_plans,
            swarm_coordinators,
        )
        .await;

        if let Some(conflict) = conflicting_live_client {
            let incoming_instance_id = incoming_client_instance_id.as_deref();
            let existing_instance_id = conflict.client_instance_id.as_deref();
            let distinct_client_instances = incoming_instance_id
                .zip(existing_instance_id)
                .map(|(incoming, existing)| incoming != existing)
                .unwrap_or(false);
            let can_take_over_live_session =
                allow_session_takeover && client_has_local_history && !distinct_client_instances;

            if can_take_over_live_session {
                let (disconnect_tx, debug_client_id, transferred_processing, transferred_tool_name) = {
                    let mut connections = client_connections.write().await;
                    let removed = connections.remove(&conflict.client_id);
                    if let Some(info) = removed {
                        (
                            Some(info.disconnect_tx),
                            info.debug_client_id,
                            info.is_processing,
                            info.current_tool_name,
                        )
                    } else {
                        (
                            None,
                            conflict.debug_client_id,
                            conflict.is_processing,
                            conflict.current_tool_name,
                        )
                    }
                };
                if transferred_processing {
                    crate::logging::warn(&format!(
                        "Taking over live session {} from {} while old owner reports processing; new connection receives status/tool metadata but not the old processing task handle",
                        session_id, conflict.client_id
                    ));
                } else {
                    crate::logging::info(&format!(
                        "Taking over live session {} from idle owner {}",
                        session_id, conflict.client_id
                    ));
                }

                {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(client_connection_id) {
                        info.is_processing = transferred_processing;
                        info.current_tool_name = transferred_tool_name;
                    }
                }

                if let Some(debug_client_id) = debug_client_id.as_deref() {
                    let mut debug_state = client_debug_state.write().await;
                    debug_state.unregister(debug_client_id);
                }

                if let Some(disconnect_tx) = disconnect_tx {
                    let _ = disconnect_tx.send(());
                }
            }
        }

        register_session_event_sender(
            swarm_members,
            &session_id,
            client_connection_id,
            client_event_tx.clone(),
        )
        .await;

        let is_canary = live_target_agent
            .try_lock()
            .ok()
            .map(|agent_guard| agent_guard.is_canary())
            .or_else(|| {
                crate::session::Session::load_startup_stub(&session_id)
                    .ok()
                    .map(|session| session.is_canary)
            })
            .unwrap_or(false);
        if is_canary {
            *client_selfdev = true;
            registry.register_selfdev_tools().await;
        }

        *client_session_id = session_id.clone();

        handle_get_history(
            id,
            &session_id,
            false,
            live_target_agent,
            provider,
            sessions,
            client_connections,
            client_count,
            writer,
            server_name,
            server_icon,
            None,
            supports_pdf_panels,
        )
        .await?;
        let _ = client_event_tx.send(ServerEvent::Done { id });
        // Resolve project-local MCP config against the resumed session's
        // working dir, not the server process cwd (issue #420).
        // Do not block on the agent lock here: the target agent may be busy
        // mid-turn (lock held), and awaiting it would deadlock the resume.
        // Must agree with the directory the restore above just bound, or the
        // session's tools and its project-local MCP config resolve against
        // different projects.
        let mcp_working_dir = bound_working_dir.clone().map(PathBuf::from);
        registry
            .register_mcp_tools_for_dir(
                Some(client_event_tx.clone()),
                Some(Arc::clone(mcp_pool)),
                Some(session_id.clone()),
                mcp_working_dir,
            )
            .await;
        spawn_model_prefetch_update(Arc::clone(provider), Arc::clone(live_target_agent));
        crate::logging::event_info(
            "SESSION_LIFECYCLE",
            vec![
                ("phase", "resume_live_attach_done".to_string()),
                ("request_id", id.to_string()),
                ("old_session_id", old_session_id),
                ("target_session_id", session_id.clone()),
                ("client_connection_id", client_connection_id.to_string()),
                ("live_target_busy", live_target_busy.to_string()),
                ("elapsed_ms", resume_start.elapsed().as_millis().to_string()),
            ],
        );
        return Ok(Arc::clone(live_target_agent));
    }

    let conflicting_live_client = {
        let connections = client_connections.read().await;
        connections
            .values()
            .find(|info| info.client_id != client_connection_id && info.session_id == session_id)
            .cloned()
    };

    if let Some(conflict) = conflicting_live_client {
        let incoming_instance_id = incoming_client_instance_id.as_deref();
        let existing_instance_id = conflict.client_instance_id.as_deref();
        let same_client_instance = incoming_instance_id
            .zip(existing_instance_id)
            .map(|(incoming, existing)| incoming == existing)
            .unwrap_or(false);
        let distinct_client_instances = incoming_instance_id
            .zip(existing_instance_id)
            .map(|(incoming, existing)| incoming != existing)
            .unwrap_or(false);
        let can_take_over_live_session = allow_session_takeover
            && (same_client_instance || (client_has_local_history && !distinct_client_instances));

        crate::logging::info(&format!(
            "Resume attach decision for session {} on connection {}: allow_takeover={}, local_history={}, same_client_instance={}, distinct_client_instances={}, incoming_instance={:?}, existing_instance={:?}, existing_owner={}",
            session_id,
            client_connection_id,
            allow_session_takeover,
            client_has_local_history,
            same_client_instance,
            distinct_client_instances,
            incoming_client_instance_id,
            conflict.client_instance_id,
            conflict.client_id,
        ));

        if can_take_over_live_session {
            crate::logging::info(&format!(
                "Taking over live session {} on connection {} by superseding {}",
                session_id, client_connection_id, conflict.client_id
            ));

            let (disconnect_tx, debug_client_id, transferred_processing, transferred_tool_name) = {
                let mut connections = client_connections.write().await;
                let removed = connections.remove(&conflict.client_id);
                if let Some(info) = removed {
                    (
                        Some(info.disconnect_tx),
                        info.debug_client_id,
                        info.is_processing,
                        info.current_tool_name,
                    )
                } else {
                    (
                        None,
                        conflict.debug_client_id,
                        conflict.is_processing,
                        conflict.current_tool_name,
                    )
                }
            };

            {
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(client_connection_id) {
                    info.is_processing = transferred_processing;
                    info.current_tool_name = transferred_tool_name;
                }
            }

            if let Some(debug_client_id) = debug_client_id.as_deref() {
                let mut debug_state = client_debug_state.write().await;
                debug_state.unregister(debug_client_id);
            }

            if let Some(disconnect_tx) = disconnect_tx {
                let _ = disconnect_tx.send(());
            }
        } else {
            if allow_session_takeover && distinct_client_instances {
                crate::logging::warn(&format!(
                    "Rejecting reconnect takeover for session {} on connection {} because the incoming client is a different live instance from the current owner; incoming_instance={:?}, existing_instance={:?}, existing live owner is {}",
                    session_id,
                    client_connection_id,
                    incoming_client_instance_id,
                    conflict.client_instance_id,
                    conflict.client_id
                ));
            } else if allow_session_takeover && !client_has_local_history && !same_client_instance {
                crate::logging::warn(&format!(
                    "Rejecting reconnect takeover for session {} on connection {} because the incoming client does not match the existing owner instance and has no local history; incoming_instance={:?}, existing_instance={:?}, existing live owner is {}",
                    session_id,
                    client_connection_id,
                    incoming_client_instance_id,
                    conflict.client_instance_id,
                    conflict.client_id
                ));
            } else {
                crate::logging::warn(&format!(
                    "Rejecting duplicate live attach for session {} on connection {} because {} is already attached",
                    session_id, client_connection_id, conflict.client_id
                ));
            }
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "Session '{}' is already live but could not be shared safely with this connection.",
                    session_id
                ),
                retry_after_secs: Some(1),
            });
            crate::logging::event_warn(
                "SESSION_LIFECYCLE",
                vec![
                    ("phase", "resume_rejected".to_string()),
                    ("request_id", id.to_string()),
                    ("target_session_id", session_id.clone()),
                    ("client_connection_id", client_connection_id.to_string()),
                    ("conflict_client_id", conflict.client_id),
                    ("elapsed_ms", resume_start.elapsed().as_millis().to_string()),
                ],
            );
            return Ok(Arc::clone(agent));
        }
    }

    let (result, is_canary) = {
        let mut agent_guard = agent.lock().await;
        let result =
            agent_guard.restore_session_with_working_dir(&session_id, bound_working_dir.as_deref());
        if *client_selfdev {
            agent_guard.set_canary("self-dev");
        }
        let is_canary = agent_guard.is_canary();
        (result, is_canary)
    };

    let was_interrupted = match &result {
        Ok(status) => {
            let agent_guard = agent.lock().await;
            restored_session_was_interrupted(&session_id, status, &agent_guard)
        }
        Err(_) => false,
    };

    if result.is_ok() && is_canary {
        *client_selfdev = true;
        registry.register_selfdev_tools().await;
    }

    match result {
        Ok(_prev_status) => {
            let old_session_id = client_session_id.clone();
            *client_session_id = session_id.clone();

            {
                let mut sessions_guard = sessions.write().await;
                sessions_guard.remove(&old_session_id);
                sessions_guard.insert(session_id.clone(), Arc::clone(agent));
            }
            crate::runtime_memory_log::emit_event(
                crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                    "session_resumed",
                    "existing_session_attached",
                )
                .with_session_id(session_id.clone())
                .force_attribution(),
            );
            rename_shutdown_signal(shutdown_signals, &old_session_id, &session_id).await;
            rename_session_interrupt_queue(soft_interrupt_queues, &old_session_id, &session_id)
                .await;
            {
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(client_connection_id) {
                    info.session_id = session_id.clone();
                    info.client_instance_id = incoming_client_instance_id.clone();
                    info.last_seen = Instant::now();
                }
            }

            rename_swarm_member_session(&old_session_id, &session_id, swarm_members, swarms_by_id)
                .await;
            remove_session_channel_subscriptions(
                &old_session_id,
                channel_subscriptions,
                channel_subscriptions_by_session,
            )
            .await;
            file_touch.clear_session(&old_session_id).await;
            {
                let mut coordinators = swarm_coordinators.write().await;
                for coordinator in coordinators.values_mut() {
                    if *coordinator == old_session_id {
                        *coordinator = session_id.clone();
                    }
                }
            }
            update_member_status(
                &session_id,
                "ready",
                None,
                swarm_members,
                swarms_by_id,
                Some(event_history),
                Some(event_counter),
                Some(swarm_event_tx),
            )
            .await;
            if let Some(swarm_id) = {
                let members = swarm_members.read().await;
                members
                    .get(&session_id)
                    .and_then(|member| member.swarm_id.clone())
            } {
                rename_plan_participant(&swarm_id, &old_session_id, &session_id, swarm_plans).await;
                let swarm_state = SwarmState {
                    members: Arc::clone(swarm_members),
                    swarms_by_id: Arc::clone(swarms_by_id),
                    plans: Arc::clone(swarm_plans),
                    coordinators: Arc::clone(swarm_coordinators),
                };
                persist_swarm_state_for(&swarm_id, &swarm_state).await;
            }

            register_session_event_sender(
                swarm_members,
                &session_id,
                client_connection_id,
                client_event_tx.clone(),
            )
            .await;

            // Captured before the history call, which takes the agent, so the restored
            // route can still be reported to a client that has never been told.
            let (
                resumed_model,
                resumed_provider_name,
                resumed_context_window,
                resumed_credential,
                resumed_reasoning_effort,
            ) = {
                let guard = agent.lock().await;
                (
                    guard.provider_model(),
                    guard.provider_name(),
                    guard.provider_context_window(),
                    guard.active_resolved_credential(),
                    guard.provider_reasoning_effort(),
                )
            };

            handle_get_history(
                id,
                &session_id,
                false,
                agent,
                provider,
                sessions,
                client_connections,
                client_count,
                writer,
                server_name,
                server_icon,
                Some(was_interrupted),
                supports_pdf_panels,
            )
            .await?;
            let _ = client_event_tx.send(ServerEvent::Done { id });
            // Re-send the swarm plan AFTER the History payload: the client
            // clears its plan snapshot on session change, so without this the
            // plan graph would stay blank until the next plan mutation.
            send_swarm_plan_to_session(&session_id, swarm_members, swarm_plans).await;
            // Report the restored route for the same reason: a resuming client
            // never saw a ModelChanged, so it would keep budgeting the session
            // from its own inert provider and fall back to the generic 200K
            // default. Measured: a route whose catalog entry carries 1000000
            // displayed 200000 after every resume.
            let _ = client_event_tx.send(ServerEvent::ModelChanged {
                id,
                model: resumed_model,
                provider_name: Some(resumed_provider_name),
                context_window: Some(resumed_context_window as u64),
                error: None,
                resolved_credential: resumed_credential,
                reasoning_effort: resumed_reasoning_effort,
            });
            // Resolve project-local MCP config against the restored session's
            // working dir, not the server process cwd (issue #420).
            let mcp_working_dir = {
                let agent_guard = agent.lock().await;
                agent_guard.working_dir().map(PathBuf::from)
            };
            registry
                .register_mcp_tools_for_dir(
                    Some(client_event_tx.clone()),
                    Some(Arc::clone(mcp_pool)),
                    Some(session_id.clone()),
                    mcp_working_dir,
                )
                .await;
            spawn_model_prefetch_update(Arc::clone(provider), Arc::clone(agent));
            crate::logging::event_info(
                "SESSION_LIFECYCLE",
                vec![
                    ("phase", "resume_restored_done".to_string()),
                    ("request_id", id.to_string()),
                    ("old_session_id", old_session_id),
                    ("target_session_id", session_id.clone()),
                    ("client_connection_id", client_connection_id.to_string()),
                    ("was_interrupted", was_interrupted.to_string()),
                    ("elapsed_ms", resume_start.elapsed().as_millis().to_string()),
                ],
            );
        }
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "Failed to restore session: {}",
                    crate::util::format_error_chain(&error)
                ),
                retry_after_secs: None,
            });
            crate::logging::event_warn(
                "SESSION_LIFECYCLE",
                vec![
                    ("phase", "resume_restore_failed".to_string()),
                    ("request_id", id.to_string()),
                    ("target_session_id", session_id),
                    ("client_connection_id", client_connection_id.to_string()),
                    ("error", crate::util::format_error_chain(&error)),
                    ("elapsed_ms", resume_start.elapsed().as_millis().to_string()),
                ],
            );
        }
    }

    Ok(Arc::clone(agent))
}

#[cfg(test)]
#[path = "client_session_tests.rs"]
mod tests;
