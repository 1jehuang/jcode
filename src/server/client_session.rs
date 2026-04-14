#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::client_state::{
    HistoryPayloadMode, send_history, session_activity_snapshot, spawn_model_prefetch_update,
};
use super::{
    ClientConnectionInfo, ClientDebugState, FileAccess, SessionInterruptQueues, SwarmEvent,
    SwarmMember, VersionedPlan, broadcast_swarm_status, fanout_live_client_event,
    register_session_event_sender, register_session_interrupt_queue, remove_plan_participant,
    remove_session_channel_subscriptions, remove_session_file_touches, remove_session_from_swarm,
    remove_session_interrupt_queue, rename_plan_participant, rename_session_interrupt_queue,
    swarm_id_for_dir, unregister_session_event_sender, update_member_status,
};
use crate::agent::Agent;
use crate::message::ContentBlock;
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::tool::Registry;
use crate::transport::WriteHalf;
use anyhow::Result;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

fn session_was_interrupted_by_reload(agent: &Agent) -> bool {
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
            is_error.unwrap_or(false)
                && (content.contains("interrupted by server reload")
                    || content.contains("Skipped - server reloading"))
        }
        _ => false,
    })
}

fn restored_session_was_interrupted(
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

    matches!(
        previous_status,
        crate::session::SessionStatus::Crashed { .. }
    ) || (matches!(previous_status, crate::session::SessionStatus::Active) && last_is_user)
        || last_is_reload_interrupted
}

fn mark_remote_reload_started(request_id: &str) {
    crate::server::write_reload_state(
        request_id,
        env!("JCODE_VERSION"),
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
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let preserve_debug = {
        let agent_guard = agent.lock().await;
        agent_guard.is_debug()
    };

    {
        let mut agent_guard = agent.lock().await;
        agent_guard.mark_closed();
    }

    let mut new_agent = Agent::new(Arc::clone(provider), registry.clone());
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
    }
    remove_session_interrupt_queue(soft_interrupt_queues, client_session_id).await;

    let swarm_id_for_update = {
        let mut members = swarm_members.write().await;
        if let Some(mut member) = members.remove(client_session_id) {
            let swarm_id = member.swarm_id.clone();
            member.session_id = new_id.clone();
            member.status = "ready".to_string();
            member.detail = None;
            members.insert(new_id.clone(), member);
            swarm_id
        } else {
            None
        }
    };
    if let Some(ref swarm_id) = swarm_id_for_update {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(swarm_id) {
            swarm.remove(client_session_id);
            swarm.insert(new_id.clone());
        }
    }
    remove_session_file_touches(client_session_id, file_touches, files_touched_by_session).await;
    remove_session_channel_subscriptions(
        client_session_id,
        channel_subscriptions,
        channel_subscriptions_by_session,
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
    if let Some(swarm_id) = swarm_id_for_update {
        rename_plan_participant(&swarm_id, client_session_id, &new_id, swarm_plans).await;
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
}

#[allow(clippy::too_many_arguments)]
async fn ensure_client_swarm_member(
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
        let agent_guard = agent.lock().await;
        let working_dir = agent_guard.working_dir().map(PathBuf::from);
        let derived_swarm_id = if swarm_enabled {
            swarm_id_for_dir(working_dir.clone())
        } else {
            None
        };
        let fallback_name = agent_guard
            .session_short_name()
            .map(|value| value.to_string());
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
                    friendly_name: member_name.clone(),
                    role: "agent".to_string(),
                    joined_at: now,
                    last_status_change: now,
                    is_headless: false,
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

    inserted
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_subscribe(
    id: u64,
    subscribe_working_dir: Option<String>,
    selfdev: Option<bool>,
    register_mcp_tools: bool,
    client_selfdev: &mut bool,
    client_session_id: &str,
    client_connection_id: &str,
    friendly_name: &Option<String>,
    agent: &Arc<Mutex<Agent>>,
    registry: &Registry,
    swarm_enabled: bool,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let subscribe_start = Instant::now();
    ensure_client_swarm_member(
        client_session_id,
        client_connection_id,
        friendly_name,
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

    if let Some(ref dir) = subscribe_working_dir {
        let mut agent_guard = agent.lock().await;
        agent_guard.set_working_dir(dir);
        drop(agent_guard);

        let new_path = PathBuf::from(dir);
        let new_swarm_id = swarm_id_for_dir(Some(new_path.clone()));
        let mut old_swarm_id: Option<String> = None;
        let mut updated_swarm_id: Option<String> = None;
        {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(client_session_id) {
                old_swarm_id = member.swarm_id.clone();
                member.working_dir = Some(new_path);
                member.swarm_id = if member.swarm_enabled {
                    new_swarm_id.clone()
                } else {
                    None
                };
                updated_swarm_id = member.swarm_id.clone();
            }
        }

        if let Some(ref old_id) = old_swarm_id {
            if updated_swarm_id.as_ref() != Some(old_id) {
                remove_session_channel_subscriptions(
                    client_session_id,
                    channel_subscriptions,
                    channel_subscriptions_by_session,
                )
                .await;
            }
            let mut swarms = swarms_by_id.write().await;
            if let Some(swarm) = swarms.get_mut(old_id) {
                swarm.remove(client_session_id);
                if swarm.is_empty() {
                    swarms.remove(old_id);
                }
            }
        }

        if let Some(ref new_id) = updated_swarm_id {
            let mut swarms = swarms_by_id.write().await;
            swarms
                .entry(new_id.clone())
                .or_insert_with(HashSet::new)
                .insert(client_session_id.to_string());
        }

        if updated_swarm_id != old_swarm_id {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(client_session_id) {
                member.role = "agent".to_string();
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            let was_coordinator = {
                let coordinators = swarm_coordinators.read().await;
                coordinators
                    .get(&old_id)
                    .map(|session_id| session_id == client_session_id)
                    .unwrap_or(false)
            };
            if was_coordinator {
                let mut new_coordinator: Option<String> = None;
                {
                    let swarms = swarms_by_id.read().await;
                    if let Some(swarm) = swarms.get(&old_id) {
                        new_coordinator = swarm.iter().min().cloned();
                    }
                }
                {
                    let mut coordinators = swarm_coordinators.write().await;
                    coordinators.remove(&old_id);
                    if let Some(ref new_id) = new_coordinator {
                        coordinators.insert(old_id.clone(), new_id.clone());
                    }
                }
                if let Some(new_id) = new_coordinator.clone() {
                    let members = swarm_members.read().await;
                    if let Some(member) = members.get(&new_id) {
                        let _ = member.event_tx.send(ServerEvent::Notification {
                            from_session: new_id.clone(),
                            from_name: member.friendly_name.clone(),
                            notification_type: NotificationType::Message {
                                scope: Some("swarm".to_string()),
                                channel: None,
                            },
                            message: "You are now the coordinator for this swarm.".to_string(),
                        });
                    }
                }
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            if updated_swarm_id.as_ref() != Some(&old_id) {
                remove_plan_participant(&old_id, client_session_id, swarm_plans).await;
            }
            broadcast_swarm_status(&old_id, swarm_members, swarms_by_id).await;
        }
        if let Some(new_id) = updated_swarm_id
            && old_swarm_id.as_ref() != Some(&new_id)
        {
            broadcast_swarm_status(&new_id, swarm_members, swarms_by_id).await;
        }
    }

    let should_selfdev = *client_selfdev || matches!(selfdev, Some(true));

    if should_selfdev {
        *client_selfdev = true;
        let mut agent_guard = agent.lock().await;
        if !agent_guard.is_canary() {
            agent_guard.set_canary("self-dev");
        }
        drop(agent_guard);
        registry.register_selfdev_tools().await;
    }

    let mcp_register_ms = if register_mcp_tools {
        let mcp_register_start = Instant::now();
        registry
            .register_mcp_tools(
                Some(client_event_tx.clone()),
                Some(Arc::clone(mcp_pool)),
                Some(client_session_id.to_string()),
            )
            .await;
        mcp_register_start.elapsed().as_millis()
    } else {
        0
    };

    crate::logging::info(&format!(
        "[TIMING] handle_subscribe: session={}, working_dir_set={}, selfdev={}, mcp_register={}ms, total={}ms",
        client_session_id,
        subscribe_working_dir.is_some(),
        should_selfdev,
        mcp_register_ms,
        subscribe_start.elapsed().as_millis(),
    ));

    update_member_status(
        client_session_id,
        "ready",
        None,
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

pub(super) async fn handle_reload(
    id: u64,
    agent: &Arc<Mutex<Agent>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let request_id = crate::id::new_id("reload");
    mark_remote_reload_started(&request_id);

    let (triggering_session, prefer_selfdev_binary) = {
        let agent_guard = agent.lock().await;
        (
            Some(agent_guard.session_id().to_string()),
            agent_guard.is_canary(),
        )
    };

    let live_sessions = {
        let members = swarm_members.read().await;
        members
            .iter()
            .filter_map(|(session_id, member)| {
                if member.event_txs.is_empty() {
                    None
                } else {
                    Some(session_id.clone())
                }
            })
            .collect::<Vec<_>>()
    };

    let mut delivered = 0;
    for session_id in &live_sessions {
        delivered += fanout_live_client_event(
            swarm_members,
            session_id,
            ServerEvent::Reloading { new_socket: None },
        )
        .await;
    }
    if delivered == 0 {
        let _ = client_event_tx.send(ServerEvent::Reloading { new_socket: None });
    }

    let hash = env!("JCODE_GIT_HASH").to_string();
    let signal_request_id =
        crate::server::send_reload_signal(hash, triggering_session.clone(), prefer_selfdev_binary);

    crate::logging::info(&format!(
        "handle_reload: queued reload signal {} from remote client request {} (triggering_session={:?}, prefer_selfdev_binary={}, reload_notified_sessions={}, reload_notified_clients={})",
        signal_request_id,
        request_id,
        triggering_session,
        prefer_selfdev_binary,
        live_sessions.len(),
        delivered
    ));

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
async fn cleanup_detached_source_session_if_unused(
    old_session_id: &str,
    client_connection_id: &str,
    source_agent: &Arc<Mutex<Agent>>,
    sessions: &SessionAgents,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) {
    unregister_session_event_sender(swarm_members, old_session_id, client_connection_id).await;

    let other_live_clients = {
        let connections = client_connections.read().await;
        connections
            .values()
            .any(|info| info.client_id != client_connection_id && info.session_id == old_session_id)
    };

    if other_live_clients {
        return;
    }

    {
        let mut sessions_guard = sessions.write().await;
        if sessions_guard
            .get(old_session_id)
            .map(|existing| Arc::ptr_eq(existing, source_agent))
            .unwrap_or(false)
        {
            sessions_guard.remove(old_session_id);
        }
    }

    {
        let mut agent_guard = source_agent.lock().await;
        agent_guard.mark_closed();
    }

    {
        let mut signals = shutdown_signals.write().await;
        signals.remove(old_session_id);
    }
    remove_session_interrupt_queue(soft_interrupt_queues, old_session_id).await;
    remove_session_channel_subscriptions(
        old_session_id,
        channel_subscriptions,
        channel_subscriptions_by_session,
    )
    .await;
    remove_session_file_touches(old_session_id, file_touches, files_touched_by_session).await;

    let removed_swarm_id = {
        let mut members = swarm_members.write().await;
        members
            .remove(old_session_id)
            .and_then(|member| member.swarm_id)
    };
    if let Some(swarm_id) = removed_swarm_id {
        remove_session_from_swarm(
            old_session_id,
            &swarm_id,
            swarm_members,
            swarms_by_id,
            swarm_coordinators,
            swarm_plans,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_resume_session(
    id: u64,
    session_id: String,
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
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
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
) -> Result<Arc<Mutex<Agent>>> {
    let incoming_client_instance_id = client_instance_id.map(str::to_string);
    let live_target_agent = {
        let sessions_guard = sessions.read().await;
        sessions_guard.get(&session_id).cloned()
    };

    if let Some(live_target_agent) = live_target_agent
        .as_ref()
        .filter(|existing| !Arc::ptr_eq(existing, agent))
    {
        let old_session_id = client_session_id.clone();

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
            file_touches,
            files_touched_by_session,
            channel_subscriptions,
            channel_subscriptions_by_session,
            swarm_plans,
            swarm_coordinators,
        )
        .await;

        {
            let mut connections = client_connections.write().await;
            if let Some(info) = connections.get_mut(client_connection_id) {
                info.session_id = session_id.clone();
                info.client_instance_id = incoming_client_instance_id.clone();
                info.last_seen = Instant::now();
            }
        }

        register_session_event_sender(
            swarm_members,
            &session_id,
            client_connection_id,
            client_event_tx.clone(),
        )
        .await;

        let is_canary = {
            let agent_guard = live_target_agent.lock().await;
            agent_guard.is_canary()
        };
        if is_canary {
            *client_selfdev = true;
            registry.register_selfdev_tools().await;
        }

        registry
            .register_mcp_tools(
                Some(client_event_tx.clone()),
                Some(Arc::clone(mcp_pool)),
                Some(session_id.clone()),
            )
            .await;

        *client_session_id = session_id.clone();

        let activity = session_activity_snapshot(client_connections, &session_id, false).await;

        send_history(
            id,
            &session_id,
            live_target_agent,
            sessions,
            client_count,
            writer,
            server_name,
            server_icon,
            None,
            activity,
            if client_has_local_history {
                HistoryPayloadMode::MetadataOnly
            } else {
                HistoryPayloadMode::Full
            },
            true,
        )
        .await?;
        let _ = client_event_tx.send(ServerEvent::Done { id });
        spawn_model_prefetch_update(Arc::clone(provider), Arc::clone(live_target_agent));
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
            return Ok(Arc::clone(agent));
        }
    }

    {
        let mut agent_guard = agent.lock().await;
        agent_guard.mark_closed();
    }

    let (result, is_canary) = {
        let mut agent_guard = agent.lock().await;
        let result = agent_guard.restore_session(&session_id);
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

    if result.is_ok() {
        registry
            .register_mcp_tools(
                Some(client_event_tx.clone()),
                Some(Arc::clone(mcp_pool)),
                Some(session_id.clone()),
            )
            .await;
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

            {
                let mut members = swarm_members.write().await;
                if let Some(mut member) = members.remove(&old_session_id) {
                    if let Some(ref swarm_id) = member.swarm_id {
                        let mut swarms = swarms_by_id.write().await;
                        if let Some(swarm) = swarms.get_mut(swarm_id) {
                            swarm.remove(&old_session_id);
                            swarm.insert(session_id.clone());
                        }
                    }
                    member.session_id = session_id.clone();
                    member.status = "ready".to_string();
                    member.detail = None;
                    members.insert(session_id.clone(), member);
                }
            }
            remove_session_channel_subscriptions(
                &old_session_id,
                channel_subscriptions,
                channel_subscriptions_by_session,
            )
            .await;
            remove_session_file_touches(&old_session_id, file_touches, files_touched_by_session)
                .await;
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
            }

            register_session_event_sender(
                swarm_members,
                &session_id,
                client_connection_id,
                client_event_tx.clone(),
            )
            .await;

            let activity = session_activity_snapshot(client_connections, &session_id, false).await;

            send_history(
                id,
                &session_id,
                agent,
                sessions,
                client_count,
                writer,
                server_name,
                server_icon,
                if was_interrupted { Some(true) } else { None },
                activity,
                if client_has_local_history {
                    HistoryPayloadMode::MetadataOnly
                } else {
                    HistoryPayloadMode::Full
                },
                true,
            )
            .await?;
            let _ = client_event_tx.send(ServerEvent::Done { id });
            spawn_model_prefetch_update(Arc::clone(provider), Arc::clone(agent));
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
        }
    }

    Ok(Arc::clone(agent))
}

#[cfg(test)]
#[path = "client_session_tests.rs"]
mod tests;
