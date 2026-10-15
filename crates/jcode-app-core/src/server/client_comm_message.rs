use super::live_turn::{LiveTurnSwarmContext, run_live_turn_if_idle};
use super::{
    ClientConnectionInfo, SessionInterruptQueues, SwarmEvent, SwarmEventType, SwarmMember,
    fanout_session_event, queue_soft_interrupt_for_session, record_swarm_event, truncate_detail,
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
    tldr: Option<String>,
    to_swarm: Option<String>,
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
    let started = std::time::Instant::now();
    crate::logging::event_info(
        "COMM_LIFECYCLE",
        vec![
            ("phase", "message_start".to_string()),
            ("request_id", id.to_string()),
            ("from_session", from_session.clone()),
            (
                "to_session",
                to_session.clone().unwrap_or_else(|| "none".to_string()),
            ),
            (
                "channel",
                channel.clone().unwrap_or_else(|| "none".to_string()),
            ),
            (
                "delivery",
                delivery
                    .map(|mode| format!("{:?}", mode))
                    .unwrap_or_else(|| "default".to_string()),
            ),
            ("wake", wake.unwrap_or(false).to_string()),
            ("message_chars", message.chars().count().to_string()),
            (
                "tldr_chars",
                tldr.as_deref()
                    .map(|t| t.chars().count())
                    .unwrap_or(0)
                    .to_string(),
            ),
        ],
    );
    let swarm_id = swarm_id_for_session(&from_session, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let friendly_name = friendly_name_for_session(&from_session, swarm_members).await;

        // Cross-swarm DMs: resolve `to_swarm` (label or id). When it names a
        // different swarm, the DM targets `to_session` inside that swarm, or
        // its coordinator when no session is given. Channels and broadcasts
        // never cross swarm boundaries.
        let cross_swarm_id = match to_swarm.as_deref().map(str::trim) {
            Some(target) if !target.is_empty() => {
                let live_swarm_ids: HashSet<String> = {
                    let swarms = swarms_by_id.read().await;
                    swarms
                        .iter()
                        .filter(|(_, sessions)| !sessions.is_empty())
                        .map(|(id, _)| id.clone())
                        .collect()
                };
                match super::swarm_labels::resolve_swarm_target(target, &live_swarm_ids) {
                    Ok(resolved) if resolved == swarm_id => None,
                    Ok(resolved) => Some(resolved),
                    Err(err) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Cross-swarm DM failed: {err}"),
                            retry_after_secs: None,
                            server_resumes: false,
                        });
                        return;
                    }
                }
            }
            _ => None,
        };
        if cross_swarm_id.is_some() && channel.is_some() && to_session.is_none() {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: "Channels are swarm-local. Cross-swarm messages are DMs: pass to_swarm with an optional to_session.".to_string(),
                retry_after_secs: None,
                server_resumes: false,
            });
            return;
        }
        let delivery_swarm_id = cross_swarm_id.clone().unwrap_or_else(|| swarm_id.clone());

        let swarm_session_ids: Vec<String> = {
            let swarms = swarms_by_id.read().await;
            swarms
                .get(&delivery_swarm_id)
                .map(|sessions| sessions.iter().cloned().collect())
                .unwrap_or_default()
        };

        let cross_swarm_default_target = if cross_swarm_id.is_some() && to_session.is_none() {
            let members = swarm_members.read().await;
            let mut coordinators: Vec<&String> = swarm_session_ids
                .iter()
                .filter(|id| members.get(*id).is_some_and(|m| m.role == "coordinator"))
                .collect();
            coordinators.sort();
            let fallback = || {
                let mut ids: Vec<&String> = swarm_session_ids
                    .iter()
                    .filter(|id| {
                        members
                            .get(*id)
                            .is_some_and(|m| m.report_back_to_session_id.is_none())
                    })
                    .collect();
                ids.sort();
                ids.first().map(|id| (*id).clone())
            };
            match coordinators.first() {
                Some(id) => Some((*id).clone()),
                None => fallback().or_else(|| swarm_session_ids.iter().min().cloned()),
            }
        } else {
            None
        };

        let resolved_to_session = if let Some(target) = cross_swarm_default_target {
            Some(target)
        } else if let Some(ref target) = to_session {
            match resolve_dm_target_session(target, &swarm_session_ids, swarm_members).await {
                Ok(session_id) => Some(session_id),
                Err(message) => {
                    crate::logging::event_warn(
                        "COMM_LIFECYCLE",
                        vec![
                            ("phase", "message_resolve_error".to_string()),
                            ("request_id", id.to_string()),
                            ("from_session", from_session.clone()),
                            ("target", target.clone()),
                            ("error", message.to_string()),
                            ("elapsed_ms", started.elapsed().as_millis().to_string()),
                        ],
                    );
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: message.to_string(),
                        retry_after_secs: None,
                        server_resumes: false,
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
            crate::logging::event_warn(
                "COMM_LIFECYCLE",
                vec![
                    ("phase", "message_target_not_in_swarm".to_string()),
                    ("request_id", id.to_string()),
                    ("from_session", from_session.clone()),
                    ("target_session", target.clone()),
                    ("swarm_id", delivery_swarm_id.clone()),
                    ("elapsed_ms", started.elapsed().as_millis().to_string()),
                ],
            );
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "DM failed: session '{}' not in swarm '{}'",
                    target,
                    super::swarm_labels::swarm_display_name(&delivery_swarm_id)
                ),
                retry_after_secs: None,
                server_resumes: false,
            });
            return;
        }

        if cross_swarm_id.is_some() && resolved_to_session.is_none() {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "Cross-swarm DM failed: swarm '{}' has no reachable members.",
                    super::swarm_labels::swarm_display_name(&delivery_swarm_id)
                ),
                retry_after_secs: None,
                server_resumes: false,
            });
            return;
        }
        let sender_swarm_display = super::swarm_labels::swarm_display_name(&swarm_id);

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

        // Broadcast-style sends are subtree-scoped: a sender reaches only the
        // agents it (transitively) spawned, via the report-back ancestry chain.
        // The swarm coordinator keeps whole-swarm reach as an escape hatch.
        // This prevents one agent from producing a member-cap-sized
        // notification storm (see docs/SWARM_TASK_GRAPH.md section 8a).
        let subtree_broadcast_targets: Vec<String> = {
            let members = swarm_members.read().await;
            let sender_is_coordinator = members
                .get(&from_session)
                .is_some_and(|member| member.role == "coordinator");
            swarm_session_ids
                .iter()
                .filter(|session_id| *session_id != &from_session)
                .filter(|session_id| {
                    sender_is_coordinator
                        || super::swarm_is_self_or_ancestor(&members, &from_session, session_id)
                })
                .cloned()
                .collect()
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
                // No subscribers: fall back to the subtree scope rather than
                // blasting the whole swarm.
                subtree_broadcast_targets.clone()
            } else {
                channel_members
                    .into_iter()
                    .filter(|session_id| session_id != &from_session)
                    .collect()
            }
        } else {
            subtree_broadcast_targets
        };

        let mut delivered_targets = 0usize;
        for session_id in &target_sessions {
            if !swarm_session_ids.contains(session_id) {
                continue;
            }
            if known_member_ids.contains(session_id) {
                let from_label = friendly_name
                    .clone()
                    .unwrap_or_else(|| from_session[..8.min(from_session.len())].to_string());
                let scope_label = match (scope, channel.as_deref()) {
                    _ if cross_swarm_id.is_some() => "Cross-swarm DM".to_string(),
                    ("channel", Some(channel_name)) => format!("#{}", channel_name),
                    ("dm", _) => "DM".to_string(),
                    _ => "broadcast".to_string(),
                };
                let from_label = if cross_swarm_id.is_some() {
                    format!("{} (swarm '{}')", from_label, sender_swarm_display)
                } else {
                    from_label
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
                            tldr: tldr.clone(),
                        },
                        message: notification_msg.clone(),
                    },
                )
                .await;

                let sender_name = friendly_name
                    .clone()
                    .unwrap_or_else(|| from_session.clone());
                let reminder = match scope {
                    _ if cross_swarm_id.is_some() => Some(format!(
                        "You just received a cross-swarm direct message from {} in swarm '{}'. To reply, use swarm action=dm with to_swarm='{}' and to_session='{}'.",
                        sender_name, sender_swarm_display, sender_swarm_display, from_session
                    )),
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
                        if crate::config::config().server.wake_mode
                            == crate::config::WakeMode::External
                        {
                            let _ = fanout_session_event(
                                swarm_members,
                                session_id,
                                ServerEvent::WakeRequested {
                                    session_id: session_id.to_string(),
                                    reason: "communication_delivery".to_string(),
                                    notification: notification_msg.clone(),
                                },
                            )
                            .await;
                            delivered_targets += 1;
                            continue;
                        }
                        let woke_immediately = run_live_turn_if_idle(
                            session_id,
                            &notification_msg,
                            reminder,
                            sessions,
                            LiveTurnSwarmContext::new(
                                swarm_members,
                                swarms_by_id,
                                event_history,
                                event_counter,
                                swarm_event_tx,
                            ),
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
                delivered_targets += 1;
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
                notification_type: scope_value.clone(),
                message: truncate_detail(&message, 220),
            },
        )
        .await;
        if let Some(ref target_swarm) = cross_swarm_id {
            record_swarm_event(
                event_history,
                event_counter,
                swarm_event_tx,
                from_session.clone(),
                friendly_name.clone(),
                Some(target_swarm.clone()),
                SwarmEventType::Notification {
                    notification_type: "cross_swarm_dm".to_string(),
                    message: truncate_detail(&message, 220),
                },
            )
            .await;
        }

        let _ = client_event_tx.send(ServerEvent::Done { id });
        crate::logging::event_info(
            "COMM_LIFECYCLE",
            vec![
                ("phase", "message_done".to_string()),
                ("request_id", id.to_string()),
                ("from_session", from_session),
                ("swarm_id", swarm_id),
                ("scope", scope.to_string()),
                ("channel", channel.unwrap_or_else(|| "none".to_string())),
                ("target_count", target_sessions.len().to_string()),
                ("delivered_targets", delivered_targets.to_string()),
                ("elapsed_ms", started.elapsed().as_millis().to_string()),
            ],
        );
    } else {
        crate::logging::event_warn(
            "COMM_LIFECYCLE",
            vec![
                ("phase", "message_error".to_string()),
                ("request_id", id.to_string()),
                ("from_session", from_session),
                ("error", "not_in_swarm".to_string()),
                ("elapsed_ms", started.elapsed().as_millis().to_string()),
            ],
        );
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
            server_resumes: false,
        });
    }
}
