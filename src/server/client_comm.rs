use super::client_lifecycle::process_message_streaming_mpsc;
use super::{
    ClientConnectionInfo, FileAccess, SessionInterruptQueues, SharedContext, SwarmEvent,
    SwarmEventType, SwarmMember, queue_soft_interrupt_for_session, record_swarm_event,
    truncate_detail, update_member_status,
};
use crate::agent::{Agent, SoftInterruptSource};
use crate::protocol::{AgentInfo, ContextEntry, NotificationType, ServerEvent};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

async fn swarm_id_for_session(
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<String> {
    let members = swarm_members.read().await;
    members.get(session_id).and_then(|m| m.swarm_id.clone())
}

async fn friendly_name_for_session(
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<String> {
    let members = swarm_members.read().await;
    members
        .get(session_id)
        .and_then(|member| member.friendly_name.clone())
}

pub(super) async fn handle_comm_share(
    id: u64,
    req_session_id: String,
    key: String,
    value: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let friendly_name = friendly_name_for_session(&req_session_id, swarm_members).await;

        {
            let mut ctx = shared_context.write().await;
            let swarm_ctx = ctx.entry(swarm_id.clone()).or_insert_with(HashMap::new);
            let now = Instant::now();
            let created_at = swarm_ctx.get(&key).map(|c| c.created_at).unwrap_or(now);
            swarm_ctx.insert(
                key.clone(),
                SharedContext {
                    key: key.clone(),
                    value: value.clone(),
                    from_session: req_session_id.clone(),
                    from_name: friendly_name.clone(),
                    created_at,
                    updated_at: now,
                },
            );
        }

        let swarm_session_ids: Vec<String> = {
            let swarms = swarms_by_id.read().await;
            swarms
                .get(&swarm_id)
                .map(|sessions| sessions.iter().cloned().collect())
                .unwrap_or_default()
        };

        let members = swarm_members.read().await;
        for sid in &swarm_session_ids {
            if sid != &req_session_id {
                if let Some(member) = members.get(sid) {
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: req_session_id.clone(),
                        from_name: friendly_name.clone(),
                        notification_type: NotificationType::SharedContext {
                            key: key.clone(),
                            value: value.clone(),
                        },
                        message: format!("Shared context: {} = {}", key, value),
                    });
                }
            }
        }

        record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            req_session_id.clone(),
            friendly_name.clone(),
            Some(swarm_id.clone()),
            SwarmEventType::ContextUpdate {
                swarm_id: swarm_id.clone(),
                key: key.clone(),
            },
        )
        .await;

        let _ = client_event_tx.send(ServerEvent::Done { id });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
        });
    }
}

pub(super) async fn handle_comm_read(
    id: u64,
    req_session_id: String,
    key: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    let entries = if let Some(swarm_id) = swarm_id {
        let ctx = shared_context.read().await;
        if let Some(swarm_ctx) = ctx.get(&swarm_id) {
            if let Some(k) = key {
                swarm_ctx
                    .get(&k)
                    .map(|c| {
                        vec![ContextEntry {
                            key: c.key.clone(),
                            value: c.value.clone(),
                            from_session: c.from_session.clone(),
                            from_name: c.from_name.clone(),
                        }]
                    })
                    .unwrap_or_default()
            } else {
                swarm_ctx
                    .values()
                    .map(|c| ContextEntry {
                        key: c.key.clone(),
                        value: c.value.clone(),
                        from_session: c.from_session.clone(),
                        from_name: c.from_name.clone(),
                    })
                    .collect()
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let _ = client_event_tx.send(ServerEvent::CommContext { id, entries });
}

pub(super) async fn handle_comm_list(
    id: u64,
    req_session_id: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let swarm_session_ids: Vec<String> = {
            let swarms = swarms_by_id.read().await;
            swarms
                .get(&swarm_id)
                .map(|sessions| sessions.iter().cloned().collect())
                .unwrap_or_default()
        };

        let members = swarm_members.read().await;
        let touches = file_touches.read().await;

        let member_list: Vec<AgentInfo> = swarm_session_ids
            .iter()
            .filter_map(|sid| {
                members.get(sid).map(|member| {
                    let files: Vec<String> = touches
                        .iter()
                        .filter_map(|(path, accesses)| {
                            if accesses.iter().any(|access| &access.session_id == sid) {
                                Some(path.display().to_string())
                            } else {
                                None
                            }
                        })
                        .collect();

                    AgentInfo {
                        session_id: sid.clone(),
                        friendly_name: member.friendly_name.clone(),
                        files_touched: files,
                        role: Some(member.role.clone()),
                    }
                })
            })
            .collect();

        let _ = client_event_tx.send(ServerEvent::CommMembers {
            id,
            members: member_list,
        });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
        });
    }
}

pub(super) async fn handle_comm_subscribe_channel(
    id: u64,
    req_session_id: String,
    channel: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let mut subs = channel_subscriptions.write().await;
        subs.entry(swarm_id.clone())
            .or_default()
            .entry(channel.clone())
            .or_default()
            .insert(req_session_id.clone());
        drop(subs);

        record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            req_session_id.clone(),
            None,
            Some(swarm_id.clone()),
            SwarmEventType::Notification {
                notification_type: "channel_subscribe".to_string(),
                message: channel.clone(),
            },
        )
        .await;

        let _ = client_event_tx.send(ServerEvent::Done { id });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm.".to_string(),
            retry_after_secs: None,
        });
    }
}

pub(super) async fn handle_comm_unsubscribe_channel(
    id: u64,
    req_session_id: String,
    channel: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let mut subs = channel_subscriptions.write().await;
        if let Some(swarm_subs) = subs.get_mut(&swarm_id) {
            if let Some(channel_subs) = swarm_subs.get_mut(&channel) {
                channel_subs.remove(&req_session_id);
                if channel_subs.is_empty() {
                    swarm_subs.remove(&channel);
                }
            }
        }
        drop(subs);

        record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            req_session_id.clone(),
            None,
            Some(swarm_id.clone()),
            SwarmEventType::Notification {
                notification_type: "channel_unsubscribe".to_string(),
                message: channel.clone(),
            },
        )
        .await;

        let _ = client_event_tx.send(ServerEvent::Done { id });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm.".to_string(),
            retry_after_secs: None,
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_message(
    id: u64,
    from_session: String,
    message: String,
    to_session: Option<String>,
    channel: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
) {
    let swarm_id = swarm_id_for_session(&from_session, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let friendly_name = friendly_name_for_session(&from_session, swarm_members).await;

        let swarm_session_ids: Vec<String> = {
            let swarms = swarms_by_id.read().await;
            swarms
                .get(&swarm_id)
                .map(|sessions| sessions.iter().cloned().collect())
                .unwrap_or_default()
        };

        if let Some(ref target) = to_session {
            if !swarm_session_ids.contains(target) {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!("DM failed: session '{}' not in swarm", target),
                    retry_after_secs: None,
                });
                return;
            }
        }

        let scope = if to_session.is_some() {
            "dm"
        } else if channel.is_some() {
            "channel"
        } else {
            "broadcast"
        };

        let members = swarm_members.read().await;

        let target_sessions: Vec<String> = if let Some(target) = to_session.clone() {
            vec![target]
        } else if let Some(ref channel_name) = channel {
            let subs = channel_subscriptions.read().await;
            if let Some(channel_subs) = subs
                .get(&swarm_id)
                .and_then(|channels| channels.get(channel_name))
            {
                channel_subs
                    .iter()
                    .filter(|session_id| *session_id != &from_session)
                    .cloned()
                    .collect()
            } else {
                swarm_session_ids
                    .iter()
                    .filter(|session_id| *session_id != &from_session)
                    .cloned()
                    .collect()
            }
        } else {
            swarm_session_ids
                .iter()
                .filter(|session_id| *session_id != &from_session)
                .cloned()
                .collect()
        };

        let connected_sessions: HashSet<String> = {
            let connections = client_connections.read().await;
            connections
                .values()
                .map(|connection| connection.session_id.clone())
                .collect()
        };

        for session_id in &target_sessions {
            if !swarm_session_ids.contains(session_id) {
                continue;
            }
            if let Some(member) = members.get(session_id) {
                let from_label = friendly_name
                    .clone()
                    .unwrap_or_else(|| from_session[..8.min(from_session.len())].to_string());
                let target_has_client = connected_sessions.contains(session_id);
                let target_is_headless = member.is_headless;
                let scope_label = match (scope, channel.as_deref()) {
                    ("channel", Some(channel_name)) => format!("#{}", channel_name),
                    ("dm", _) => "DM".to_string(),
                    _ => "broadcast".to_string(),
                };
                let notification_msg = format!("{} from {}: {}", scope_label, from_label, message);
                let _ = member.event_tx.send(ServerEvent::Notification {
                    from_session: from_session.clone(),
                    from_name: friendly_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some(scope.to_string()),
                        channel: channel.clone(),
                    },
                    message: notification_msg.clone(),
                });

                if !target_has_client {
                    let _ = queue_soft_interrupt_for_session(
                        session_id,
                        notification_msg.clone(),
                        false,
                        SoftInterruptSource::System,
                        soft_interrupt_queues,
                        sessions,
                    )
                    .await;
                }

                if target_is_headless && !target_has_client {
                    let target_session = session_id.clone();
                    let notification_msg = notification_msg.clone();
                    let scope_string = scope.to_string();
                    let channel_name = channel.clone();
                    let from_session_clone = from_session.clone();
                    let from_name_clone = friendly_name.clone();
                    let sessions_for_run = Arc::clone(sessions);
                    let swarm_members_for_run = Arc::clone(swarm_members);
                    let swarms_for_run = Arc::clone(swarms_by_id);
                    let event_history_for_run = Arc::clone(event_history);
                    let event_counter_for_run = Arc::clone(event_counter);
                    let swarm_event_tx_for_run = swarm_event_tx.clone();
                    tokio::spawn(async move {
                        let agent_arc = {
                            let agent_sessions = sessions_for_run.read().await;
                            agent_sessions.get(&target_session).cloned()
                        };
                        let Some(agent_arc) = agent_arc else {
                            return;
                        };

                        let detail = match scope_string.as_str() {
                            "dm" => format!("DM from {}", from_label),
                            "channel" => format!(
                                "#{} from {}",
                                channel_name
                                    .clone()
                                    .unwrap_or_else(|| "channel".to_string()),
                                from_label
                            ),
                            _ => format!("broadcast from {}", from_label),
                        };

                        update_member_status(
                            &target_session,
                            "running",
                            Some(truncate_detail(&detail, 120)),
                            &swarm_members_for_run,
                            &swarms_for_run,
                            Some(&event_history_for_run),
                            Some(&event_counter_for_run),
                            Some(&swarm_event_tx_for_run),
                        )
                        .await;

                        let sender_name = from_name_clone
                            .clone()
                            .unwrap_or_else(|| from_session_clone.clone());
                        let reminder = match scope_string.as_str() {
                            "dm" => format!(
                                "You just received a direct swarm message from {}. The latest swarm notification context has already been queued into this turn. Review it and respond or act if useful.",
                                sender_name
                            ),
                            "channel" => format!(
                                "You just received a swarm channel message in #{} from {}. The latest swarm notification context has already been queued into this turn. Review it and respond or act if useful.",
                                channel_name
                                    .clone()
                                    .unwrap_or_else(|| "channel".to_string()),
                                sender_name
                            ),
                            _ => format!(
                                "You just received a swarm broadcast from {}. The latest swarm notification context has already been queued into this turn. Review it and respond or act if useful.",
                                sender_name
                            ),
                        };

                        let (drain_tx, mut drain_rx) =
                            tokio::sync::mpsc::unbounded_channel::<ServerEvent>();
                        tokio::spawn(async move { while drain_rx.recv().await.is_some() {} });

                        let result = process_message_streaming_mpsc(
                            Arc::clone(&agent_arc),
                            &notification_msg,
                            vec![],
                            Some(reminder),
                            drain_tx,
                        )
                        .await;

                        let (status, detail) = match result {
                            Ok(()) => ("ready", None),
                            Err(error) => {
                                ("failed", Some(truncate_detail(&error.to_string(), 120)))
                            }
                        };
                        update_member_status(
                            &target_session,
                            status,
                            detail,
                            &swarm_members_for_run,
                            &swarms_for_run,
                            Some(&event_history_for_run),
                            Some(&event_counter_for_run),
                            Some(&swarm_event_tx_for_run),
                        )
                        .await;
                    });
                }
            }
        }

        let scope_value = if scope == "channel" {
            format!("#{}", channel.clone().unwrap_or_default())
        } else {
            scope.to_string()
        };
        record_swarm_event(
            event_history,
            event_counter,
            swarm_event_tx,
            from_session.clone(),
            friendly_name.clone(),
            Some(swarm_id.clone()),
            SwarmEventType::Notification {
                notification_type: scope_value,
                message: truncate_detail(&message, 220),
            },
        )
        .await;

        let _ = client_event_tx.send(ServerEvent::Done { id });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::handle_comm_message;
    use crate::agent::Agent;
    use crate::message::{Message, ToolDefinition};
    use crate::protocol::{NotificationType, ServerEvent};
    use crate::provider::{EventStream, Provider};
    use crate::server::{ClientConnectionInfo, SessionInterruptQueues, SwarmEvent, SwarmMember};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, atomic::AtomicU64};
    use std::time::Instant;
    use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

    struct TestProvider;

    #[async_trait]
    impl Provider for TestProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            unimplemented!("test provider")
        }

        fn name(&self) -> &str {
            "test"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(TestProvider)
        }
    }

    async fn test_agent() -> Arc<Mutex<Agent>> {
        let provider: Arc<dyn Provider> = Arc::new(TestProvider);
        let registry = Registry::new(provider.clone()).await;
        Arc::new(Mutex::new(Agent::new(provider, registry)))
    }

    #[tokio::test]
    async fn comm_message_does_not_queue_soft_interrupt_for_connected_session() {
        let sender = test_agent().await;
        let target = test_agent().await;

        let sender_id = sender.lock().await.session_id().to_string();
        let target_id = target.lock().await.session_id().to_string();
        let target_queue = target.lock().await.soft_interrupt_queue();

        let sessions = Arc::new(RwLock::new(HashMap::from([
            (sender_id.clone(), sender.clone()),
            (target_id.clone(), target.clone()),
        ])));
        let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));

        let (sender_event_tx, _sender_event_rx) = mpsc::unbounded_channel();
        let (target_event_tx, mut target_event_rx) = mpsc::unbounded_channel();
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

        let swarm_id = "swarm-test".to_string();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            (
                sender_id.clone(),
                SwarmMember {
                    session_id: sender_id.clone(),
                    event_tx: sender_event_tx,
                    working_dir: None,
                    swarm_id: Some(swarm_id.clone()),
                    swarm_enabled: true,
                    status: "ready".to_string(),
                    detail: None,
                    friendly_name: Some("falcon".to_string()),
                    role: "coordinator".to_string(),
                    joined_at: Instant::now(),
                    last_status_change: Instant::now(),
                    is_headless: false,
                },
            ),
            (
                target_id.clone(),
                SwarmMember {
                    session_id: target_id.clone(),
                    event_tx: target_event_tx,
                    working_dir: None,
                    swarm_id: Some(swarm_id.clone()),
                    swarm_enabled: true,
                    status: "ready".to_string(),
                    detail: None,
                    friendly_name: Some("bear".to_string()),
                    role: "agent".to_string(),
                    joined_at: Instant::now(),
                    last_status_change: Instant::now(),
                    is_headless: false,
                },
            ),
        ])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            swarm_id.clone(),
            HashSet::from([sender_id.clone(), target_id.clone()]),
        )])));
        let channel_subscriptions = Arc::new(RwLock::new(HashMap::from([(
            swarm_id.clone(),
            HashMap::from([(
                "religion-debate".to_string(),
                HashSet::from([target_id.clone()]),
            )]),
        )])));
        let event_history: Arc<RwLock<Vec<SwarmEvent>>> = Arc::new(RwLock::new(Vec::new()));
        let event_counter = Arc::new(AtomicU64::new(0));
        let (swarm_event_tx, _) = broadcast::channel(16);
        let client_connections = Arc::new(RwLock::new(HashMap::from([(
            "client-1".to_string(),
            ClientConnectionInfo {
                client_id: "client-1".to_string(),
                session_id: target_id.clone(),
                debug_client_id: None,
                connected_at: Instant::now(),
                last_seen: Instant::now(),
            },
        )])));

        handle_comm_message(
            1,
            sender_id.clone(),
            "hello".to_string(),
            None,
            Some("religion-debate".to_string()),
            &client_event_tx,
            &sessions,
            &soft_interrupt_queues,
            &swarm_members,
            &swarms_by_id,
            &channel_subscriptions,
            &event_history,
            &event_counter,
            &swarm_event_tx,
            &client_connections,
        )
        .await;

        match target_event_rx.recv().await.expect("target notification") {
            ServerEvent::Notification {
                from_session,
                from_name,
                notification_type,
                message,
            } => {
                assert_eq!(from_session, sender_id);
                assert_eq!(from_name.as_deref(), Some("falcon"));
                match notification_type {
                    NotificationType::Message { scope, channel } => {
                        assert_eq!(scope.as_deref(), Some("channel"));
                        assert_eq!(channel.as_deref(), Some("religion-debate"));
                    }
                    other => panic!("unexpected notification type: {:?}", other),
                }
                assert_eq!(message, "#religion-debate from falcon: hello");
            }
            other => panic!("unexpected event: {:?}", other),
        }

        match client_event_rx.recv().await.expect("done event") {
            ServerEvent::Done { id } => assert_eq!(id, 1),
            other => panic!("unexpected client event: {:?}", other),
        }

        let pending = target_queue.lock().expect("target queue lock");
        assert!(
            pending.is_empty(),
            "connected interactive session should not get synthetic user-message interrupt"
        );
    }
}
