use super::client_lifecycle::process_message_streaming_mpsc;
use super::{
    broadcast_swarm_status, remove_session_from_swarm, swarm_id_for_dir, truncate_detail,
    update_member_status, SwarmEvent, SwarmMember, VersionedPlan,
};
use crate::agent::{Agent, StreamError};
use crate::protocol::{FeatureToggle, NotificationType, ServerEvent};
use crate::session::Session;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

pub(super) async fn handle_set_feature(
    id: u64,
    feature: FeatureToggle,
    enabled: bool,
    agent: &Arc<Mutex<Agent>>,
    client_session_id: &str,
    friendly_name: &Option<String>,
    swarm_enabled: &mut bool,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    match feature {
        FeatureToggle::Memory => {
            let mut agent_guard = agent.lock().await;
            agent_guard.set_memory_enabled(enabled);
            drop(agent_guard);
            if !enabled {
                crate::memory::clear_pending_memory(client_session_id);
            }
            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
        FeatureToggle::Swarm => {
            if *swarm_enabled == enabled {
                let _ = client_event_tx.send(ServerEvent::Done { id });
                return;
            }
            *swarm_enabled = enabled;

            let (old_swarm_id, working_dir) = {
                let mut members = swarm_members.write().await;
                if let Some(member) = members.get_mut(client_session_id) {
                    let old = member.swarm_id.clone();
                    let wd = member.working_dir.clone();
                    member.swarm_enabled = enabled;
                    if !enabled {
                        member.swarm_id = None;
                        member.role = "agent".to_string();
                    }
                    (old, wd)
                } else {
                    (None, None)
                }
            };

            if let Some(ref old_id) = old_swarm_id {
                remove_session_from_swarm(
                    client_session_id,
                    old_id,
                    swarm_members,
                    swarms_by_id,
                    swarm_coordinators,
                    swarm_plans,
                )
                .await;
            }

            if enabled {
                let new_swarm_id = swarm_id_for_dir(working_dir);
                if let Some(ref id) = new_swarm_id {
                    {
                        let mut swarms = swarms_by_id.write().await;
                        swarms
                            .entry(id.clone())
                            .or_insert_with(HashSet::new)
                            .insert(client_session_id.to_string());
                    }

                    let mut is_new_coordinator = false;
                    {
                        let mut coordinators = swarm_coordinators.write().await;
                        if coordinators.get(id).is_none() {
                            coordinators.insert(id.clone(), client_session_id.to_string());
                            is_new_coordinator = true;
                        }
                    }

                    {
                        let mut members = swarm_members.write().await;
                        if let Some(member) = members.get_mut(client_session_id) {
                            member.swarm_id = Some(id.clone());
                            member.role = if is_new_coordinator {
                                "coordinator".to_string()
                            } else {
                                "agent".to_string()
                            };
                        }
                    }

                    broadcast_swarm_status(id, swarm_members, swarms_by_id).await;

                    if is_new_coordinator {
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
                } else {
                    let _ = client_event_tx.send(ServerEvent::SwarmStatus {
                        members: Vec::new(),
                    });
                }
            } else {
                let _ = client_event_tx.send(ServerEvent::SwarmStatus {
                    members: Vec::new(),
                });
            }

            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
    }
}

pub(super) async fn handle_split(
    id: u64,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let (new_session_id, new_session_name) = {
        let agent_guard = agent.lock().await;
        let parent_session_id = agent_guard.session_id().to_string();
        let messages = agent_guard.messages().to_vec();
        let working_dir = agent_guard.working_dir().map(|s| s.to_string());
        let model = Some(agent_guard.provider_model());

        let mut child = Session::create(Some(parent_session_id), None);
        child.messages = messages;
        child.working_dir = working_dir;
        child.model = model;
        child.status = crate::session::SessionStatus::Closed;

        if let Err(e) = child.save() {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Failed to save split session: {e}"),
                retry_after_secs: None,
            });
            return;
        }

        let name = child.display_name().to_string();
        (child.id.clone(), name)
    };

    let _ = client_event_tx.send(ServerEvent::SplitResponse {
        id,
        new_session_id,
        new_session_name,
    });
}

pub(super) fn handle_compact(
    id: u64,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let agent = Arc::clone(agent);
    let tx = client_event_tx.clone();
    tokio::spawn(async move {
        let agent_guard = agent.lock().await;
        let provider = agent_guard.provider_fork();
        let compaction = agent_guard.registry().compaction();
        let messages = agent_guard.provider_messages();
        drop(agent_guard);

        if !provider.supports_compaction() {
            let _ = tx.send(ServerEvent::CompactResult {
                id,
                message: "Manual compaction is not available for this provider.".to_string(),
                success: false,
            });
            return;
        }

        let result = match compaction.try_write() {
            Ok(mut manager) => {
                let stats = manager.stats_with(&messages);
                let status_msg = format!(
                    "**Context Status:**\n\
                    • Messages: {} (active), {} (total history)\n\
                    • Token usage: ~{}k (estimate ~{}k) / {}k ({:.1}%)\n\
                    • Has summary: {}\n\
                    • Compacting: {}",
                    stats.active_messages,
                    stats.total_turns,
                    stats.effective_tokens / 1000,
                    stats.token_estimate / 1000,
                    manager.token_budget() / 1000,
                    stats.context_usage * 100.0,
                    if stats.has_summary { "yes" } else { "no" },
                    if stats.is_compacting {
                        "in progress..."
                    } else {
                        "no"
                    }
                );

                match manager.force_compact_with(&messages, provider) {
                    Ok(()) => ServerEvent::CompactResult {
                        id,
                        message: format!(
                            "{}\n\n✓ **Compaction started** — summarizing older messages in background.\n\
                            The summary will be applied automatically when ready.",
                            status_msg
                        ),
                        success: true,
                    },
                    Err(reason) => ServerEvent::CompactResult {
                        id,
                        message: format!("{status_msg}\n\n⚠ **Cannot compact:** {reason}"),
                        success: false,
                    },
                }
            }
            Err(_) => ServerEvent::CompactResult {
                id,
                message: "⚠ Cannot access compaction manager (lock held)".to_string(),
                success: false,
            },
        };
        let _ = tx.send(result);
    });
}

pub(super) async fn handle_stdin_response(
    id: u64,
    request_id: String,
    input: String,
    stdin_responses: &Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Some(tx) = stdin_responses.lock().await.remove(&request_id) {
        let _ = tx.send(input);
    }
    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_agent_task(
    id: u64,
    task: String,
    client_session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    update_member_status(
        client_session_id,
        "running",
        Some(truncate_detail(&task, 120)),
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;

    let result =
        process_message_streaming_mpsc(Arc::clone(agent), &task, vec![], client_event_tx.clone())
            .await;
    match result {
        Ok(()) => {
            update_member_status(
                client_session_id,
                "completed",
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
        Err(e) => {
            update_member_status(
                client_session_id,
                "failed",
                Some(truncate_detail(&e.to_string(), 120)),
                swarm_members,
                swarms_by_id,
                Some(event_history),
                Some(event_counter),
                Some(swarm_event_tx),
            )
            .await;
            let retry_after_secs = e
                .downcast_ref::<StreamError>()
                .and_then(|stream_error| stream_error.retry_after_secs);
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: e.to_string(),
                retry_after_secs,
            });
        }
    }
}
