use super::{
    ClientConnectionInfo, SessionInterruptQueues, SwarmEvent, SwarmEventType, SwarmMember,
    fanout_session_event, queue_soft_interrupt_for_session, record_swarm_event,
    session_event_fanout_sender, truncate_detail,
};
use crate::agent::Agent;
use crate::protocol::{CommDeliveryMode, NotificationType, ServerEvent};
use jcode_agent_runtime::SoftInterruptSource;
use jcode_swarm_core::ChannelIndex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

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

async fn resolve_dm_target_session(
    target: &str,
    swarm_session_ids: &[String],
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> anyhow::Result<String> {
    if swarm_session_ids
        .iter()
        .any(|session_id| session_id == target)
    {
        return Ok(target.to_string());
    }

    let members = swarm_members.read().await;
    let mut matches: Vec<(String, String)> = swarm_session_ids
        .iter()
        .filter_map(|session_id| {
            let member = members.get(session_id)?;
            member
                .friendly_name
                .as_deref()
                .filter(|friendly_name| *friendly_name == target)
                .map(|friendly_name| (session_id.clone(), friendly_name.to_string()))
        })
        .collect();
    matches.sort_by(|(left_session, _), (right_session, _)| left_session.cmp(right_session));
    matches.dedup_by(|(left_session, _), (right_session, _)| left_session == right_session);
    match matches.len() {
        1 => Ok(matches.remove(0).0),
        0 => Err(anyhow::anyhow!(
            "Unknown target '{}' - use an exact session_id or a unique friendly name within the swarm.",
            target
        )),
        _ => {
            let match_list = matches
                .iter()
                .map(|(session_id, friendly_name)| format!("{} [{}]", friendly_name, session_id))
                .collect::<Vec<_>>()
                .join(", ");
            Err(anyhow::anyhow!(
                "Friendly name '{}' is ambiguous in swarm. Use an exact session id instead. Matches: {}",
                target,
                match_list
            ))
        }
    }
}

async fn run_message_in_live_session_if_idle(
    session_id: &str,
    message: &str,
    system_reminder: Option<String>,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    let agent = {
        let guard = sessions.read().await;
        guard.get(session_id).cloned()
    };
    let Some(agent) = agent else {
        return false;
    };

    let has_live_attachments = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .map(|member| !member.event_txs.is_empty() || !member.event_tx.is_closed())
            .unwrap_or(false)
    };
    if !has_live_attachments {
        return false;
    }

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
    let event_tx = session_event_fanout_sender(session_id.clone(), Arc::clone(swarm_members));
    tokio::spawn(async move {
        let _ = super::client_lifecycle::process_message_streaming_mpsc(
            agent,
            &message,
            vec![],
            system_reminder,
            event_tx,
        )
        .await;
    });

    true
}

fn resolve_comm_delivery_mode(
    scope: &str,
    delivery: Option<CommDeliveryMode>,
    wake: Option<bool>,
) -> CommDeliveryMode {
    if let Some(delivery) = delivery {
        return delivery;
    }
    if wake.unwrap_or(false) {
        return CommDeliveryMode::Wake;
    }
    match scope {
        "dm" => CommDeliveryMode::Wake,
        _ => CommDeliveryMode::Notify,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "comm message routes DM, channel, and broadcast delivery with session fanout state"
)]
pub(super) async fn handle_comm_message(
    id: u64,
    from_session: String,
    message: String,
    to_session: Option<String>,
    channel: Option<String>,
    delivery: Option<CommDeliveryMode>,
    wake: Option<bool>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    channel_subscriptions: &ChannelSubscriptions,
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

        let resolved_to_session = if let Some(ref target) = to_session {
            match resolve_dm_target_session(target, &swarm_session_ids, swarm_members).await {
                Ok(session_id) => Some(session_id),
                Err(message) => {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: message.to_string(),
                        retry_after_secs: None,
                    });
                    return;
                }
            }
        } else {
            None
        };

        if let Some(ref target) = resolved_to_session
            && !swarm_session_ids.contains(target)
        {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("DM failed: session '{}' not in swarm", target),
                retry_after_secs: None,
            });
            return;
        }

        let scope = if resolved_to_session.is_some() {
            "dm"
        } else if channel.is_some() {
            "channel"
        } else {
            "broadcast"
        };

        let known_member_ids: std::collections::HashSet<String> = {
            let members = swarm_members.read().await;
            members.keys().cloned().collect()
        };

        let target_sessions: Vec<String> = if let Some(target) = resolved_to_session {
            vec![target]
        } else if let Some(ref channel_name) = channel {
            let subs = channel_subscriptions.read().await;
            let index = ChannelIndex {
                by_swarm_channel: subs.clone(),
                by_session: HashMap::new(),
            };
            let channel_members = index.members(&swarm_id, channel_name);
            if channel_members.is_empty() {
                swarm_session_ids
                    .iter()
                    .filter(|session_id| *session_id != &from_session)
                    .cloned()
                    .collect()
            } else {
                channel_members
                    .into_iter()
                    .filter(|session_id| session_id != &from_session)
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
            if known_member_ids.contains(session_id) {
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
                let _ = fanout_session_event(
                    swarm_members,
                    session_id,
                    ServerEvent::Notification {
                        from_session: from_session.clone(),
                        from_name: friendly_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some(scope.to_string()),
                            channel: channel.clone(),
                        },
                        message: notification_msg.clone(),
                    },
                )
                .await;

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
