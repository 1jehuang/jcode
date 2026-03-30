use super::client_lifecycle::process_message_streaming_mpsc;
use super::{
    ClientConnectionInfo, SessionInterruptQueues, SharedContext, SwarmEvent, SwarmEventType,
    SwarmMember, queue_soft_interrupt_for_session, record_swarm_event,
    subscribe_session_to_channel, truncate_detail, unsubscribe_session_from_channel,
};
use crate::agent::Agent;
use crate::protocol::{
    AgentInfo, CommDeliveryMode, ContextEntry, NotificationType, ServerEvent, SwarmChannelInfo,
};
use jcode_agent_runtime::SoftInterruptSource;
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

async fn run_message_in_live_session_if_idle(
    session_id: &str,
    message: &str,
    reminder: Option<String>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    let agent = {
        let guard = sessions.read().await;
        guard.get(session_id).cloned()
    };
    let Some(agent) = agent else {
        return false;
    };

    let event_tx = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .map(|member| member.event_tx.clone())
    };
    let Some(event_tx) = event_tx else {
        return false;
    };

    let is_idle = match agent.try_lock() {
        Ok(guard) => {
            drop(guard);
            true
        }
        Err(_) => false,
    };

    if !is_idle {
        return false;
    }

    let session_id = session_id.to_string();
    let message = message.to_string();
    tokio::spawn(async move {
        if let Err(err) =
            process_message_streaming_mpsc(agent, &message, vec![], reminder, event_tx).await
        {
            crate::logging::error(&format!(
                "Failed to run comm message immediately for live session {}: {}",
                session_id, err
            ));
        }
    });

    true
}

fn resolve_comm_delivery_mode(
    scope: &str,
    delivery: Option<CommDeliveryMode>,
    wake: Option<bool>,
) -> CommDeliveryMode {
    if let Some(mode) = delivery {
        return mode;
    }
    if let Some(should_wake) = wake {
        return if should_wake {
            CommDeliveryMode::Wake
        } else {
            CommDeliveryMode::Notify
        };
    }
    match scope {
        "dm" => CommDeliveryMode::Wake,
        _ => CommDeliveryMode::Notify,
    }
}

pub(super) async fn handle_comm_share(
    id: u64,
    req_session_id: String,
    key: String,
    value: String,
    append: bool,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
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
            let stored_value = if append {
                swarm_ctx
                    .get(&key)
                    .map(|existing| {
                        if existing.value.is_empty() {
                            value.clone()
                        } else {
                            format!("{}\n{}", existing.value, value)
                        }
                    })
                    .unwrap_or_else(|| value.clone())
            } else {
                value.clone()
            };
            swarm_ctx.insert(
                key.clone(),
                SharedContext {
                    key: key.clone(),
                    value: stored_value.clone(),
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
                            value: if append {
                                format!("(appended) {}", value)
                            } else {
                                value.clone()
                            },
                        },
                        message: if append {
                            format!("Appended shared context: {} += {}", key, value)
                        } else {
                            format!("Shared context: {} = {}", key, value)
                        },
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
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
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
        let touches = files_touched_by_session.read().await;

        let member_list: Vec<AgentInfo> = swarm_session_ids
            .iter()
            .filter_map(|sid| {
                members.get(sid).map(|member| {
                    let mut files: Vec<String> = touches
                        .get(sid)
                        .into_iter()
                        .flat_map(|paths| paths.iter())
                        .map(|path| path.display().to_string())
                        .collect();
                    files.sort();

                    AgentInfo {
                        session_id: sid.clone(),
                        friendly_name: member.friendly_name.clone(),
                        files_touched: files,
                        status: Some(member.status.clone()),
                        detail: member.detail.clone(),
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

pub(super) async fn handle_comm_list_channels(
    id: u64,
    req_session_id: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let channels = {
            let subs = channel_subscriptions.read().await;
            let mut channels: Vec<SwarmChannelInfo> = subs
                .get(&swarm_id)
                .map(|swarm_channels| {
                    swarm_channels
                        .iter()
                        .map(|(channel, members)| SwarmChannelInfo {
                            channel: channel.clone(),
                            member_count: members.len(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            channels.sort_by(|left, right| left.channel.cmp(&right.channel));
            channels
        };

        let _ = client_event_tx.send(ServerEvent::CommChannels { id, channels });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
        });
    }
}

pub(super) async fn handle_comm_channel_members(
    id: u64,
    req_session_id: String,
    channel: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let member_ids: Vec<String> = {
            let subs = channel_subscriptions.read().await;
            subs.get(&swarm_id)
                .and_then(|swarm_channels| swarm_channels.get(&channel))
                .map(|members| {
                    let mut ids: Vec<String> = members.iter().cloned().collect();
                    ids.sort();
                    ids
                })
                .unwrap_or_default()
        };

        let members = swarm_members.read().await;
        let entries: Vec<AgentInfo> = member_ids
            .iter()
            .filter_map(|sid| {
                members.get(sid).map(|member| AgentInfo {
                    session_id: sid.clone(),
                    friendly_name: member.friendly_name.clone(),
                    files_touched: Vec::new(),
                    status: Some(member.status.clone()),
                    detail: member.detail.clone(),
                    role: Some(member.role.clone()),
                })
            })
            .collect();

        let _ = client_event_tx.send(ServerEvent::CommMembers {
            id,
            members: entries,
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
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        subscribe_session_to_channel(
            &req_session_id,
            &swarm_id,
            &channel,
            channel_subscriptions,
            channel_subscriptions_by_session,
        )
        .await;

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
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        unsubscribe_session_from_channel(
            &req_session_id,
            &swarm_id,
            &channel,
            channel_subscriptions,
            channel_subscriptions_by_session,
        )
        .await;

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
    delivery: Option<CommDeliveryMode>,
    wake: Option<bool>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    _client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
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

        for session_id in &target_sessions {
            if !swarm_session_ids.contains(session_id) {
                continue;
            }
            if let Some(member) = members.get(session_id) {
                let from_label = friendly_name
                    .clone()
                    .unwrap_or_else(|| from_session[..8.min(from_session.len())].to_string());
                let scope_label = match (scope, channel.as_deref()) {
                    ("channel", Some(channel_name)) => format!("#{}", channel_name),
                    ("dm", _) => "DM".to_string(),
                    _ => "broadcast".to_string(),
                };
                let delivery_mode = resolve_comm_delivery_mode(scope, delivery, wake);
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

                let sender_name = friendly_name
                    .clone()
                    .unwrap_or_else(|| from_session.clone());
                let reminder = match scope {
                    "dm" => Some(format!(
                        "You just received a direct swarm message from {}. Review it and respond or act if useful.",
                        sender_name
                    )),
                    "channel" => Some(format!(
                        "You just received a swarm channel message in #{} from {}. Review it and respond or act if useful.",
                        channel.clone().unwrap_or_else(|| "channel".to_string()),
                        sender_name
                    )),
                    _ => Some(format!(
                        "You just received a swarm broadcast from {}. Review it and respond or act if useful.",
                        sender_name
                    )),
                };

                match delivery_mode {
                    CommDeliveryMode::Notify => {}
                    CommDeliveryMode::Interrupt => {
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
                    CommDeliveryMode::Wake => {
                        let woke_immediately = run_message_in_live_session_if_idle(
                            session_id,
                            &notification_msg,
                            reminder,
                            sessions,
                            swarm_members,
                        )
                        .await;

                        if !woke_immediately {
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
                    }
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
    use super::{handle_comm_list, handle_comm_message};
    use crate::agent::Agent;
    use crate::message::{Message, ToolDefinition};
    use crate::protocol::{CommDeliveryMode, NotificationType, ServerEvent};
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
    async fn comm_message_default_does_not_queue_soft_interrupt_for_connected_session() {
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
        let event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>> =
            Arc::new(RwLock::new(std::collections::VecDeque::new()));
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
            None,
            None,
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

    #[tokio::test]
    async fn comm_message_with_wake_queues_soft_interrupt_for_busy_connected_session() {
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
        crate::server::register_session_interrupt_queue(
            &soft_interrupt_queues,
            &target_id,
            target_queue.clone(),
        )
        .await;

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
        let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
        let event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>> =
            Arc::new(RwLock::new(std::collections::VecDeque::new()));
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

        let _busy_guard = target.lock().await;

        handle_comm_message(
            1,
            sender_id.clone(),
            "hello now".to_string(),
            Some(target_id.clone()),
            None,
            Some(CommDeliveryMode::Wake),
            None,
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
                        assert_eq!(scope.as_deref(), Some("dm"));
                        assert_eq!(channel, None);
                    }
                    other => panic!("unexpected notification type: {:?}", other),
                }
                assert_eq!(message, "DM from falcon: hello now");
            }
            other => panic!("unexpected event: {:?}", other),
        }

        match client_event_rx.recv().await.expect("done event") {
            ServerEvent::Done { id } => assert_eq!(id, 1),
            other => panic!("unexpected client event: {:?}", other),
        }

        let pending = target_queue.lock().expect("target queue lock");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "DM from falcon: hello now");
        assert_eq!(
            pending[0].source,
            jcode_agent_runtime::SoftInterruptSource::System
        );
    }

    #[tokio::test]
    async fn comm_list_includes_member_status_and_detail() {
        let requester = test_agent().await;
        let peer = test_agent().await;

        let requester_id = requester.lock().await.session_id().to_string();
        let peer_id = peer.lock().await.session_id().to_string();
        let swarm_id = "swarm-test".to_string();

        let (requester_event_tx, _requester_event_rx) = mpsc::unbounded_channel();
        let (peer_event_tx, _peer_event_rx) = mpsc::unbounded_channel();
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            (
                requester_id.clone(),
                SwarmMember {
                    session_id: requester_id.clone(),
                    event_tx: requester_event_tx,
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
                peer_id.clone(),
                SwarmMember {
                    session_id: peer_id.clone(),
                    event_tx: peer_event_tx,
                    working_dir: None,
                    swarm_id: Some(swarm_id.clone()),
                    swarm_enabled: true,
                    status: "running".to_string(),
                    detail: Some("working on tests".to_string()),
                    friendly_name: Some("bear".to_string()),
                    role: "agent".to_string(),
                    joined_at: Instant::now(),
                    last_status_change: Instant::now(),
                    is_headless: false,
                },
            ),
        ])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            swarm_id,
            HashSet::from([requester_id.clone(), peer_id.clone()]),
        )])));
        let file_touches = Arc::new(RwLock::new(HashMap::new()));

        handle_comm_list(
            1,
            requester_id,
            &client_event_tx,
            &swarm_members,
            &swarms_by_id,
            &file_touches,
        )
        .await;

        match client_event_rx.recv().await.expect("comm list response") {
            ServerEvent::CommMembers { id, members } => {
                assert_eq!(id, 1);
                let peer = members
                    .into_iter()
                    .find(|member| member.friendly_name.as_deref() == Some("bear"))
                    .expect("peer entry present");
                assert_eq!(peer.status.as_deref(), Some("running"));
                assert_eq!(peer.detail.as_deref(), Some("working on tests"));
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }
}
