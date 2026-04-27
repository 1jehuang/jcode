use super::client_lifecycle::process_message_streaming_mpsc;
use super::swarm_mutation_state::{
    PersistedSwarmMutationResponse, SwarmMutationRuntime, begin_or_replay, finish_request,
    request_key,
};
use super::{
    SessionInterruptQueues, SwarmEvent, SwarmEventType, SwarmMember, SwarmState, VersionedPlan,
    broadcast_swarm_plan, broadcast_swarm_status, create_headless_session, persist_swarm_state_for,
    record_swarm_event, record_swarm_event_for_session, remove_session_channel_subscriptions,
    remove_session_from_swarm, remove_session_interrupt_queue, truncate_detail,
    update_member_status,
};
use crate::agent::Agent;
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::session::Session;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

fn create_visible_spawn_session(
    working_dir: Option<&str>,
    model_override: Option<&str>,
    selfdev_requested: bool,
) -> anyhow::Result<(String, PathBuf)> {
    let cwd = working_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut session = Session::create(None, None);
    session.working_dir = Some(cwd.display().to_string());
    if let Some(model) = model_override {
        session.model = Some(model.to_string());
    }
    if selfdev_requested {
        session.set_canary("self-dev");
    }
    session.save()?;

    Ok((session.id.clone(), cwd))
}

fn spawn_visible_session_window(
    session_id: &str,
    cwd: &std::path::Path,
    selfdev_requested: bool,
) -> anyhow::Result<bool> {
    let exe = crate::build::client_update_candidate(selfdev_requested)
        .map(|(path, _label)| path)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("jcode"));
    if selfdev_requested {
        crate::cli::tui_launch::spawn_selfdev_in_new_terminal(&exe, session_id, cwd)
    } else {
        crate::cli::tui_launch::spawn_resume_in_new_terminal(&exe, session_id, cwd)
    }
}

fn persist_headed_startup_message(session_id: &str, message: &str) {
    crate::tui::App::save_startup_submission_for_session(
        session_id,
        message.to_string(),
        Vec::new(),
    );
}

fn clear_headed_startup_message(session_id: &str) {
    if let Ok(jcode_dir) = crate::storage::jcode_dir() {
        let path = jcode_dir.join(format!("client-input-{}", session_id));
        let _ = std::fs::remove_file(path);
    }
}

fn cleanup_prepared_visible_spawn_session(session_id: &str) {
    clear_headed_startup_message(session_id);
    if let Ok(path) = crate::session::session_path(session_id) {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(path) = crate::session::session_journal_path(session_id) {
        let _ = std::fs::remove_file(path);
    }
}

fn prepare_visible_spawn_session<F>(
    working_dir: Option<&str>,
    model_override: Option<&str>,
    selfdev_requested: bool,
    startup_message: Option<&str>,
    launch_visible: F,
) -> anyhow::Result<(String, bool)>
where
    F: FnOnce(&str, &std::path::Path, bool) -> anyhow::Result<bool>,
{
    let (new_session_id, cwd) =
        create_visible_spawn_session(working_dir, model_override, selfdev_requested)?;

    if let Some(message) = startup_message {
        persist_headed_startup_message(&new_session_id, message);
    }

    match launch_visible(&new_session_id, &cwd, selfdev_requested) {
        Ok(launched) => {
            if !launched {
                cleanup_prepared_visible_spawn_session(&new_session_id);
            }
            Ok((new_session_id, launched))
        }
        Err(error) => {
            cleanup_prepared_visible_spawn_session(&new_session_id);
            Err(error)
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "visible spawn registration updates swarm state, event history, and UI delivery metadata together"
)]
async fn register_visible_spawned_member(
    session_id: &str,
    swarm_id: &str,
    working_dir: Option<&str>,
    has_startup_message: bool,
    report_back_to_session_id: Option<&str>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    let friendly_name = crate::id::extract_session_name(session_id)
        .map(|name| name.to_string())
        .unwrap_or_else(|| session_id.to_string());
    let (status, detail) = if has_startup_message {
        ("running".to_string(), Some("startup queued".to_string()))
    } else {
        ("spawned".to_string(), Some("launching client".to_string()))
    };

    {
        let mut members = swarm_members.write().await;
        members.insert(
            session_id.to_string(),
            SwarmMember {
                session_id: session_id.to_string(),
                event_tx,
                event_txs: HashMap::new(),
                working_dir: working_dir.map(PathBuf::from),
                swarm_id: Some(swarm_id.to_string()),
                swarm_enabled: true,
                status,
                detail,
                friendly_name: Some(friendly_name),
                report_back_to_session_id: report_back_to_session_id.map(str::to_string),
                role: "agent".to_string(),
                joined_at: now,
                last_status_change: now,
                is_headless: false,
            },
        );
    }

    {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(swarm_id.to_string())
            .or_insert_with(HashSet::new)
            .insert(session_id.to_string());
    }

    record_swarm_event_for_session(
        session_id,
        SwarmEventType::MemberChange {
            action: "joined".to_string(),
        },
        swarm_members,
        event_history,
        event_counter,
        swarm_event_tx,
    )
    .await;
    broadcast_swarm_status(swarm_id, swarm_members, swarms_by_id).await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "server-side swarm spawning needs session, swarm state, provider, and event sinks together"
)]
pub(super) async fn spawn_swarm_agent(
    req_session_id: &str,
    swarm_id: &str,
    working_dir: Option<String>,
    initial_message: Option<String>,
    sessions: &SessionAgents,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn Provider>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    soft_interrupt_queues: &SessionInterruptQueues,
) -> anyhow::Result<String> {
    let coordinator_model = {
        let agent_sessions = sessions.read().await;
        agent_sessions.get(req_session_id).and_then(|agent| {
            agent
                .try_lock()
                .ok()
                .map(|agent_guard| agent_guard.provider_model())
        })
    };
    let spawn_model = crate::config::config()
        .agents
        .swarm_model
        .clone()
        .or(coordinator_model.clone());
    let coordinator_is_canary = {
        let agent_sessions = sessions.read().await;
        agent_sessions
            .get(req_session_id)
            .and_then(|agent| {
                agent
                    .try_lock()
                    .ok()
                    .map(|agent_guard| agent_guard.is_canary())
            })
            .unwrap_or(false)
    };

    let visible_spawn = prepare_visible_spawn_session(
        working_dir.as_deref(),
        spawn_model.as_deref(),
        coordinator_is_canary,
        initial_message.as_deref(),
        spawn_visible_session_window,
    );

    let (new_session_id, is_headless_fallback) = match visible_spawn {
        Ok((new_session_id, true)) => Ok((new_session_id, false)),
        Ok((_, false)) | Err(_) => {
            let cmd = if let Some(ref dir) = working_dir {
                format!("create_session:{dir}")
            } else {
                "create_session".to_string()
            };
            create_headless_session(
                sessions,
                global_session_id,
                provider_template,
                &cmd,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
                soft_interrupt_queues,
                coordinator_is_canary,
                spawn_model.clone(),
                Some(Arc::clone(mcp_pool)),
                Some(req_session_id.to_string()),
            )
            .await
            .and_then(|result_json| {
                serde_json::from_str::<serde_json::Value>(&result_json)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("session_id")
                            .and_then(|session_id| session_id.as_str())
                            .map(|session_id| session_id.to_string())
                    })
                    .map(|session_id| (session_id, true))
                    .ok_or_else(|| anyhow::anyhow!("Failed to parse spawned session id"))
            })
        }
    }?;

    let startup_message = initial_message.clone();
    {
        let mut plans = swarm_plans.write().await;
        if let Some(plan) = plans.get_mut(swarm_id)
            && (!plan.items.is_empty() || !plan.participants.is_empty())
        {
            plan.participants.insert(req_session_id.to_string());
            plan.participants.insert(new_session_id.clone());
        }
    }

    broadcast_swarm_plan(
        swarm_id,
        Some("participant_spawned".to_string()),
        swarm_plans,
        swarm_members,
        swarms_by_id,
    )
    .await;
    if !is_headless_fallback {
        register_visible_spawned_member(
            &new_session_id,
            swarm_id,
            working_dir.as_deref(),
            startup_message.is_some(),
            Some(req_session_id),
            swarm_members,
            swarms_by_id,
            event_history,
            event_counter,
            swarm_event_tx,
        )
        .await;
    }
    let swarm_state = SwarmState {
        members: Arc::clone(swarm_members),
        swarms_by_id: Arc::clone(swarms_by_id),
        plans: Arc::clone(swarm_plans),
        coordinators: Arc::clone(swarm_coordinators),
    };
    persist_swarm_state_for(swarm_id, &swarm_state).await;

    if let Some(initial_msg) = startup_message
        && is_headless_fallback
    {
        record_swarm_event_for_session(
            &new_session_id,
            SwarmEventType::MemberChange {
                action: "joined".to_string(),
            },
            swarm_members,
            event_history,
            event_counter,
            swarm_event_tx,
        )
        .await;

        let agent_arc = {
            let agent_sessions = sessions.read().await;
            agent_sessions.get(&new_session_id).cloned()
        };
        if let Some(agent_arc) = agent_arc {
            let sid_clone = new_session_id.clone();
            let swarm_members2 = Arc::clone(swarm_members);
            let swarms_by_id2 = Arc::clone(swarms_by_id);
            let event_history2 = Arc::clone(event_history);
            let event_counter2 = Arc::clone(event_counter);
            let swarm_event_tx2 = swarm_event_tx.clone();
            tokio::spawn(async move {
                update_member_status(
                    &sid_clone,
                    "running",
                    Some(truncate_detail(&initial_msg, 120)),
                    &swarm_members2,
                    &swarms_by_id2,
                    Some(&event_history2),
                    Some(&event_counter2),
                    Some(&swarm_event_tx2),
                )
                .await;
                let event_tx = super::session_event_fanout_sender(
                    sid_clone.clone(),
                    Arc::clone(&swarm_members2),
                );
                let result = process_message_streaming_mpsc(
                    Arc::clone(&agent_arc),
                    &initial_msg,
                    vec![],
                    None,
                    event_tx,
                )
                .await;
                let (new_status, new_detail) = match result {
                    Ok(()) => ("ready", None),
                    Err(ref error) => ("failed", Some(truncate_detail(&error.to_string(), 120))),
                };
                update_member_status(
                    &sid_clone,
                    new_status,
                    new_detail,
                    &swarm_members2,
                    &swarms_by_id2,
                    Some(&event_history2),
                    Some(&event_counter2),
                    Some(&swarm_event_tx2),
                )
                .await;
            });
        }
    }

    Ok(new_session_id)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_spawn(
    id: u64,
    req_session_id: String,
    working_dir: Option<String>,
    initial_message: Option<String>,
    request_nonce: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn Provider>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    _channel_subscriptions: &ChannelSubscriptions,
    _channel_subscriptions_by_session: &ChannelSubscriptions,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_mutation_runtime: &SwarmMutationRuntime,
) {
    let swarm_id = match ensure_spawn_coordinator_swarm(
        id,
        &req_session_id,
        "Only the coordinator can spawn new agents.",
        client_event_tx,
        swarm_members,
        swarms_by_id,
        swarm_coordinators,
        swarm_plans,
    )
    .await
    {
        Some(swarm_id) => swarm_id,
        None => return,
    };

    let mutation_key = request_key(
        &req_session_id,
        "spawn",
        &[
            swarm_id.clone(),
            working_dir.clone().unwrap_or_default(),
            initial_message.clone().unwrap_or_default(),
            request_nonce.clone().unwrap_or_default(),
        ],
    );
    let Some(mutation_state) = begin_or_replay(
        swarm_mutation_runtime,
        &mutation_key,
        "spawn",
        &req_session_id,
        id,
        client_event_tx,
    )
    .await
    else {
        return;
    };

    let response = match spawn_swarm_agent(
        &req_session_id,
        &swarm_id,
        working_dir,
        initial_message,
        sessions,
        global_session_id,
        provider_template,
        swarm_members,
        swarms_by_id,
        swarm_coordinators,
        swarm_plans,
        event_history,
        event_counter,
        swarm_event_tx,
        mcp_pool,
        soft_interrupt_queues,
    )
    .await
    {
        Ok(new_session_id) => PersistedSwarmMutationResponse::Spawn { new_session_id },
        Err(error) => PersistedSwarmMutationResponse::Error {
            message: format!("Failed to spawn agent: {error}"),
            retry_after_secs: None,
        },
    };

    finish_request(swarm_mutation_runtime, &mutation_state, response).await;
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_stop(
    id: u64,
    req_session_id: String,
    target_session: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_mutation_runtime: &SwarmMutationRuntime,
) {
    let swarm_id = if let Some(swarm_id) = require_coordinator_swarm(
        id,
        &req_session_id,
        "Only the coordinator can stop agents.",
        client_event_tx,
        swarm_members,
        swarm_coordinators,
    )
    .await
    {
        swarm_id
    } else {
        return;
    };

    let mutation_key = request_key(&req_session_id, "stop", &[swarm_id, target_session.clone()]);
    let Some(mutation_state) = begin_or_replay(
        swarm_mutation_runtime,
        &mutation_key,
        "stop",
        &req_session_id,
        id,
        client_event_tx,
    )
    .await
    else {
        return;
    };

    let mut sessions_guard = sessions.write().await;
    let removed_agent = sessions_guard.remove(&target_session);
    drop(sessions_guard);
    if let Some(agent_arc) = removed_agent {
        remove_session_interrupt_queue(soft_interrupt_queues, &target_session).await;
        if let Ok(agent) = agent_arc.try_lock() {
            let memory_enabled = agent.memory_enabled();
            let transcript = if memory_enabled {
                Some(agent.build_transcript_for_extraction())
            } else {
                None
            };
            let sid = target_session.clone();
            let working_dir = agent.working_dir().map(|dir| dir.to_string());
            drop(agent);
            if let Some(transcript) = transcript {
                crate::memory_agent::trigger_final_extraction_with_dir(
                    transcript,
                    sid,
                    working_dir,
                );
            }
        }

        let (removed_swarm_id, removed_name) = {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.remove(&target_session) {
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
                target_session.clone(),
                removed_name.clone(),
                Some(swarm_id.clone()),
                SwarmEventType::MemberChange {
                    action: "left".to_string(),
                },
            )
            .await;
            remove_session_from_swarm(
                &target_session,
                swarm_id,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
            )
            .await;
        }
        remove_session_channel_subscriptions(
            &target_session,
            channel_subscriptions,
            channel_subscriptions_by_session,
        )
        .await;
        finish_request(
            swarm_mutation_runtime,
            &mutation_state,
            PersistedSwarmMutationResponse::Done,
        )
        .await;
    } else {
        finish_request(
            swarm_mutation_runtime,
            &mutation_state,
            PersistedSwarmMutationResponse::Error {
                message: format!("Unknown session '{target_session}'"),
                retry_after_secs: None,
            },
        )
        .await;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "spawn coordinator resolution checks swarm membership, coordinator state, and promotion side effects together"
)]
async fn ensure_spawn_coordinator_swarm(
    id: u64,
    req_session_id: &str,
    permission_error: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Option<String> {
    let (swarm_id, from_name, coordinator_id) = {
        let members = swarm_members.read().await;
        let swarm_id = members
            .get(req_session_id)
            .and_then(|member| member.swarm_id.clone());
        let from_name = members
            .get(req_session_id)
            .and_then(|member| member.friendly_name.clone());
        let coordinator_id = if let Some(ref swarm_id) = swarm_id {
            let coordinators = swarm_coordinators.read().await;
            coordinators.get(swarm_id).cloned()
        } else {
            None
        };
        (swarm_id, from_name, coordinator_id)
    };

    let Some(swarm_id) = swarm_id else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm.".to_string(),
            retry_after_secs: None,
        });
        return None;
    };

    if coordinator_id.as_deref() == Some(req_session_id) {
        return Some(swarm_id);
    }

    if coordinator_id.is_some() {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: permission_error.to_string(),
            retry_after_secs: None,
        });
        return None;
    }

    let promoted = {
        let mut coordinators = swarm_coordinators.write().await;
        match coordinators.get(&swarm_id) {
            Some(existing) if existing == req_session_id => false,
            Some(_) => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: permission_error.to_string(),
                    retry_after_secs: None,
                });
                return None;
            }
            None => {
                coordinators.insert(swarm_id.clone(), req_session_id.to_string());
                true
            }
        }
    };

    if promoted {
        {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(req_session_id) {
                member.role = "coordinator".to_string();
            }
        }
        let swarm_state = SwarmState {
            members: Arc::clone(swarm_members),
            swarms_by_id: Arc::clone(swarms_by_id),
            plans: Arc::clone(swarm_plans),
            coordinators: Arc::clone(swarm_coordinators),
        };
        persist_swarm_state_for(&swarm_id, &swarm_state).await;
        broadcast_swarm_status(&swarm_id, swarm_members, swarms_by_id).await;
        let _ = client_event_tx.send(ServerEvent::Notification {
            from_session: req_session_id.to_string(),
            from_name,
            notification_type: NotificationType::Message {
                scope: Some("swarm".to_string()),
                channel: None,
            },
            message: "You are the coordinator for this swarm.".to_string(),
        });
    }

    Some(swarm_id)
}

async fn require_coordinator_swarm(
    id: u64,
    req_session_id: &str,
    permission_error: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) -> Option<String> {
    let (swarm_id, is_coordinator) = {
        let members = swarm_members.read().await;
        let swarm_id = members
            .get(req_session_id)
            .and_then(|member| member.swarm_id.clone());
        let is_coordinator = if let Some(ref swarm_id) = swarm_id {
            let coordinators = swarm_coordinators.read().await;
            coordinators
                .get(swarm_id)
                .map(|coordinator| coordinator == req_session_id)
                .unwrap_or(false)
        } else {
            false
        };
        (swarm_id, is_coordinator)
    };

    if !is_coordinator {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: permission_error.to_string(),
            retry_after_secs: None,
        });
        return None;
    }

    match swarm_id {
        Some(swarm_id) => Some(swarm_id),
        None => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: "Not in a swarm.".to_string(),
                retry_after_secs: None,
            });
            None
        }
    }
}

#[cfg(test)]
#[path = "comm_session_tests.rs"]
mod comm_session_tests;
