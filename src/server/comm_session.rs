use super::client_lifecycle::process_message_streaming_mpsc;
use super::{
    SessionInterruptQueues, SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan,
    broadcast_swarm_plan, broadcast_swarm_status, create_headless_session, record_swarm_event,
    record_swarm_event_for_session, remove_plan_participant, remove_session_channel_subscriptions,
    remove_session_interrupt_queue, truncate_detail, update_member_status,
};
use crate::agent::Agent;
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_spawn(
    id: u64,
    req_session_id: String,
    working_dir: Option<String>,
    initial_message: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn Provider>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    _channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    soft_interrupt_queues: &SessionInterruptQueues,
) {
    let swarm_id = match ensure_spawn_coordinator_swarm(
        id,
        &req_session_id,
        "Only the coordinator can spawn new agents.",
        client_event_tx,
        swarm_members,
        swarms_by_id,
        swarm_coordinators,
    )
    .await
    {
        Some(swarm_id) => swarm_id,
        None => return,
    };

    let cmd = if let Some(ref dir) = working_dir {
        format!("create_session:{dir}")
    } else {
        "create_session".to_string()
    };
    let coordinator_model = {
        let agent_sessions = sessions.read().await;
        agent_sessions.get(&req_session_id).and_then(|agent| {
            agent
                .try_lock()
                .ok()
                .map(|agent_guard| agent_guard.provider_model())
        })
    };
    let coordinator_is_canary = {
        let agent_sessions = sessions.read().await;
        agent_sessions
            .get(&req_session_id)
            .and_then(|agent| {
                agent
                    .try_lock()
                    .ok()
                    .map(|agent_guard| agent_guard.is_canary())
            })
            .unwrap_or(false)
    };

    match create_headless_session(
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
        coordinator_model,
        Some(Arc::clone(mcp_pool)),
    )
    .await
    {
        Ok(result_json) => {
            let new_session_id = serde_json::from_str::<serde_json::Value>(&result_json)
                .ok()
                .and_then(|value| {
                    value
                        .get("session_id")
                        .and_then(|session_id| session_id.as_str())
                        .map(|session_id| session_id.to_string())
                })
                .unwrap_or_default();

            {
                let mut plans = swarm_plans.write().await;
                if let Some(plan) = plans.get_mut(&swarm_id) {
                    if !plan.items.is_empty() || !plan.participants.is_empty() {
                        plan.participants.insert(req_session_id.clone());
                        plan.participants.insert(new_session_id.clone());
                    }
                }
            }

            broadcast_swarm_plan(
                &swarm_id,
                Some("participant_spawned".to_string()),
                swarm_plans,
                swarm_members,
                swarms_by_id,
            )
            .await;
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

            if let Some(initial_msg) = initial_message {
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
                        let (drain_tx, mut drain_rx) =
                            tokio::sync::mpsc::unbounded_channel::<ServerEvent>();
                        tokio::spawn(async move { while drain_rx.recv().await.is_some() {} });
                        let result = process_message_streaming_mpsc(
                            Arc::clone(&agent_arc),
                            &initial_msg,
                            vec![],
                            None,
                            drain_tx,
                        )
                        .await;
                        let (new_status, new_detail) = match result {
                            Ok(()) => ("ready", None),
                            Err(ref error) => {
                                ("failed", Some(truncate_detail(&error.to_string(), 120)))
                            }
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

            let _ = client_event_tx.send(ServerEvent::CommSpawnResponse {
                id,
                session_id: req_session_id,
                new_session_id,
            });
        }
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Failed to spawn agent: {error}"),
                retry_after_secs: None,
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_stop(
    id: u64,
    req_session_id: String,
    target_session: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    soft_interrupt_queues: &SessionInterruptQueues,
) {
    if require_coordinator_swarm(
        id,
        &req_session_id,
        "Only the coordinator can stop agents.",
        client_event_tx,
        swarm_members,
        swarm_coordinators,
    )
    .await
    .is_none()
    {
        return;
    }

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
            remove_plan_participant(swarm_id, &target_session, swarm_plans).await;
            {
                let mut swarms = swarms_by_id.write().await;
                if let Some(swarm) = swarms.get_mut(swarm_id) {
                    swarm.remove(&target_session);
                    if swarm.is_empty() {
                        swarms.remove(swarm_id);
                    }
                }
            }
            let was_coordinator = {
                let coordinators = swarm_coordinators.read().await;
                coordinators
                    .get(swarm_id)
                    .map(|coordinator| coordinator == &target_session)
                    .unwrap_or(false)
            };
            if was_coordinator {
                let new_coordinator = {
                    let swarms = swarms_by_id.read().await;
                    swarms
                        .get(swarm_id)
                        .and_then(|swarm| swarm.iter().min().cloned())
                };
                let mut coordinators = swarm_coordinators.write().await;
                coordinators.remove(swarm_id);
                if let Some(ref new_id) = new_coordinator {
                    coordinators.insert(swarm_id.clone(), new_id.clone());
                    let mut members = swarm_members.write().await;
                    if let Some(member) = members.get_mut(new_id) {
                        member.role = "coordinator".to_string();
                    }
                    let mut plans = swarm_plans.write().await;
                    if let Some(plan) = plans.get_mut(swarm_id) {
                        plan.participants.insert(new_id.clone());
                    }
                }
            }
            broadcast_swarm_status(swarm_id, swarm_members, swarms_by_id).await;
        }
        remove_session_channel_subscriptions(&target_session, channel_subscriptions).await;
        let _ = client_event_tx.send(ServerEvent::Done { id });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!("Unknown session '{target_session}'"),
            retry_after_secs: None,
        });
    }
}

async fn ensure_spawn_coordinator_swarm(
    id: u64,
    req_session_id: &str,
    permission_error: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
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
mod tests {
    use super::ensure_spawn_coordinator_swarm;
    use crate::protocol::{NotificationType, ServerEvent};
    use crate::server::SwarmMember;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::{RwLock, mpsc};

    fn member(
        session_id: &str,
        swarm_id: Option<&str>,
        role: &str,
    ) -> (SwarmMember, mpsc::UnboundedReceiver<ServerEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        (
            SwarmMember {
                session_id: session_id.to_string(),
                event_tx,
                working_dir: None,
                swarm_id: swarm_id.map(|id| id.to_string()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some(session_id.to_string()),
                role: role.to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
            },
            event_rx,
        )
    }

    #[tokio::test]
    async fn spawn_bootstraps_coordinator_when_swarm_has_none() {
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from(["req".to_string()]),
        )])));
        let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
        let (req_member, _req_rx) = member("req", Some("swarm-1"), "agent");
        swarm_members
            .write()
            .await
            .insert("req".to_string(), req_member);
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

        let swarm_id = ensure_spawn_coordinator_swarm(
            1,
            "req",
            "Only the coordinator can spawn new agents.",
            &client_event_tx,
            &swarm_members,
            &swarms_by_id,
            &swarm_coordinators,
        )
        .await;

        assert_eq!(swarm_id.as_deref(), Some("swarm-1"));
        assert_eq!(
            swarm_coordinators
                .read()
                .await
                .get("swarm-1")
                .map(String::as_str),
            Some("req")
        );
        assert_eq!(
            swarm_members
                .read()
                .await
                .get("req")
                .map(|member| member.role.as_str()),
            Some("coordinator")
        );
        assert!(matches!(
            client_event_rx.recv().await,
            Some(ServerEvent::Notification {
                notification_type: NotificationType::Message { .. },
                message,
                ..
            }) if message == "You are the coordinator for this swarm."
        ));
    }

    #[tokio::test]
    async fn spawn_requires_existing_coordinator_when_one_is_set() {
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from(["req".to_string(), "coord".to_string()]),
        )])));
        let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            "coord".to_string(),
        )])));
        let (req_member, _req_rx) = member("req", Some("swarm-1"), "agent");
        let (coord_member, _coord_rx) = member("coord", Some("swarm-1"), "coordinator");
        let mut members = swarm_members.write().await;
        members.insert("req".to_string(), req_member);
        members.insert("coord".to_string(), coord_member);
        drop(members);
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

        let swarm_id = ensure_spawn_coordinator_swarm(
            2,
            "req",
            "Only the coordinator can spawn new agents.",
            &client_event_tx,
            &swarm_members,
            &swarms_by_id,
            &swarm_coordinators,
        )
        .await;

        assert!(swarm_id.is_none());
        assert!(matches!(
            client_event_rx.recv().await,
            Some(ServerEvent::Error { message, .. })
                if message == "Only the coordinator can spawn new agents."
        ));
        assert_eq!(
            swarm_members
                .read()
                .await
                .get("req")
                .map(|member| member.role.as_str()),
            Some("agent")
        );
    }
}
