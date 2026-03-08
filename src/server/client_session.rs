use super::client_state::send_history;
use super::reload::{do_server_reload_with_progress, normalize_model_arg, provider_cli_arg};
use super::{
    broadcast_swarm_status, is_jcode_repo_or_parent, is_selfdev_env, remove_plan_participant,
    rename_plan_participant, socket_path, swarm_id_for_dir, update_member_status,
    ClientConnectionInfo, SwarmEvent, SwarmMember, VersionedPlan,
};
use crate::agent::Agent;
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::tool::Registry;
use crate::transport::WriteHalf;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_clear_session(
    id: u64,
    client_selfdev: bool,
    client_session_id: &mut String,
    client_connection_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
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
pub(super) async fn handle_subscribe(
    id: u64,
    subscribe_working_dir: Option<String>,
    selfdev: Option<bool>,
    client_selfdev: &mut bool,
    client_session_id: &str,
    friendly_name: &Option<String>,
    agent: &Arc<Mutex<Agent>>,
    registry: &Registry,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
) {
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

        if let Some(new_id) = updated_swarm_id.clone() {
            let mut coordinators = swarm_coordinators.write().await;
            if coordinators.get(&new_id).is_none() {
                coordinators.insert(new_id.clone(), client_session_id.to_string());
                let _ = client_event_tx.send(ServerEvent::Notification {
                    from_session: client_session_id.to_string(),
                    from_name: friendly_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("swarm".to_string()),
                        channel: None,
                    },
                    message: "You are the coordinator for this swarm.".to_string(),
                });
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            if updated_swarm_id.as_ref() != Some(&old_id) {
                remove_plan_participant(&old_id, client_session_id, swarm_plans).await;
            }
            broadcast_swarm_status(&old_id, swarm_members, swarms_by_id).await;
        }
        if let Some(new_id) = updated_swarm_id {
            if old_swarm_id.as_ref() != Some(&new_id) {
                broadcast_swarm_status(&new_id, swarm_members, swarms_by_id).await;
            }
        }
    }

    let mut should_selfdev = *client_selfdev;
    if matches!(selfdev, Some(true)) {
        should_selfdev = true;
    }

    if !should_selfdev {
        if let Some(ref dir) = subscribe_working_dir {
            let path = PathBuf::from(dir);
            if is_jcode_repo_or_parent(&path) {
                should_selfdev = true;
            }
        }
    }

    if should_selfdev {
        *client_selfdev = true;
        let mut agent_guard = agent.lock().await;
        if !agent_guard.is_canary() {
            agent_guard.set_canary("self-dev");
        }
        drop(agent_guard);
        registry.register_selfdev_tools().await;
    }

    registry
        .register_mcp_tools(
            Some(client_event_tx.clone()),
            Some(Arc::clone(mcp_pool)),
            Some(client_session_id.to_string()),
        )
        .await;

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

pub(super) async fn handle_reload(
    id: u64,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let _ = client_event_tx.send(ServerEvent::Reloading { new_socket: None });

    let (provider_arg, model_arg) = {
        let agent_guard = agent.lock().await;
        (
            provider_cli_arg(&agent_guard.provider_name()),
            normalize_model_arg(agent_guard.provider_model()),
        )
    };

    let progress_tx = client_event_tx.clone();
    let socket_arg = socket_path().to_string_lossy().to_string();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Err(error) =
            do_server_reload_with_progress(progress_tx.clone(), provider_arg, model_arg, socket_arg)
                .await
        {
            let _ = progress_tx.send(ServerEvent::ReloadProgress {
                step: "error".to_string(),
                message: format!("Reload failed: {}", error),
                success: Some(false),
                output: None,
            });
            crate::logging::error(&format!("Reload failed: {}", error));
        }
    });

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_resume_session(
    id: u64,
    session_id: String,
    client_selfdev: &mut bool,
    client_session_id: &mut String,
    client_connection_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) -> Result<()> {
    {
        let mut agent_guard = agent.lock().await;
        agent_guard.mark_closed();
    }

    let (result, is_canary) = {
        let mut agent_guard = agent.lock().await;
        let result = agent_guard.restore_session(&session_id);
        if *client_selfdev || is_selfdev_env() {
            agent_guard.set_canary("self-dev");
        }
        let is_canary = agent_guard.is_canary();
        (result, is_canary)
    };

    let was_interrupted = match &result {
        Ok(status) => match status {
            crate::session::SessionStatus::Crashed { .. } => true,
            crate::session::SessionStatus::Active => {
                let agent_guard = agent.lock().await;
                let last_role = agent_guard.last_message_role();
                let last_is_user = last_role
                    .as_ref()
                    .map(|role| *role == crate::message::Role::User)
                    .unwrap_or(false);
                let last_is_reload_interrupted = last_role
                    .as_ref()
                    .map(|role| *role == crate::message::Role::Assistant)
                    .unwrap_or(false)
                    && agent_guard
                        .last_message_text()
                        .map(|text| text.ends_with("[generation interrupted - server reloading]"))
                        .unwrap_or(false);
                if last_is_user {
                    crate::logging::info(&format!(
                        "Session {} was Active with pending user message - treating as interrupted",
                        session_id
                    ));
                }
                if last_is_reload_interrupted {
                    crate::logging::info(&format!(
                        "Session {} was interrupted by reload - will auto-resume",
                        session_id
                    ));
                }
                last_is_user || last_is_reload_interrupted
            }
            _ => false,
        },
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
                Some(client_session_id.clone()),
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
            {
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(client_connection_id) {
                    info.session_id = session_id.clone();
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

            let _ = provider.prefetch_models().await;
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
            )
            .await?;
        }
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Failed to restore session: {}", error),
                retry_after_secs: None,
            });
        }
    }

    Ok(())
}
