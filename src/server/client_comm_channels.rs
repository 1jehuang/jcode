use super::state::ChannelIndex;
use super::{
    SwarmEvent, SwarmEventType, SwarmMember, record_swarm_event, subscribe_session_to_channel,
    unsubscribe_session_from_channel,
};
use crate::protocol::{AgentInfo, ServerEvent, SwarmChannelInfo};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};

type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

async fn swarm_id_for_session(
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<String> {
    let members = swarm_members.read().await;
    members.get(session_id).and_then(|m| m.swarm_id.clone())
}

pub(super) async fn handle_comm_list_channels(
    id: u64,
    req_session_id: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    channel_subscriptions: &ChannelSubscriptions,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let channels = {
            let subs = channel_subscriptions.read().await;
            let index = ChannelIndex {
                by_swarm_channel: subs.clone(),
                by_session: HashMap::new(),
            };
            let mut channels: Vec<SwarmChannelInfo> = Vec::new();
            if let Some(swarm_channels) = index.by_swarm_channel.get(&swarm_id) {
                for (channel, members) in swarm_channels {
                    channels.push(SwarmChannelInfo {
                        channel: channel.clone(),
                        member_count: members.len(),
                    });
                }
            }
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
    channel_subscriptions: &ChannelSubscriptions,
) {
    let swarm_id = swarm_id_for_session(&req_session_id, swarm_members).await;

    if let Some(swarm_id) = swarm_id {
        let member_ids: Vec<String> = {
            let subs = channel_subscriptions.read().await;
            let index = ChannelIndex {
                by_swarm_channel: subs.clone(),
                by_session: HashMap::new(),
            };
            index.members(&swarm_id, &channel)
        };

        let members = swarm_members.read().await;
        let entries: Vec<AgentInfo> = member_ids
            .iter()
            .filter_map(|sid: &String| {
                members.get(sid).map(|member| AgentInfo {
                    session_id: sid.clone(),
                    friendly_name: member.friendly_name.clone(),
                    files_touched: Vec::new(),
                    status: Some(member.status.clone()),
                    detail: member.detail.clone(),
                    role: Some(member.role.clone()),
                    is_headless: Some(member.is_headless),
                    live_attachments: Some(member.event_txs.len()),
                    status_age_secs: Some(member.last_status_change.elapsed().as_secs()),
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

#[expect(
    clippy::too_many_arguments,
    reason = "channel subscribe updates membership, delivery, and swarm event history together"
)]
pub(super) async fn handle_comm_subscribe_channel(
    id: u64,
    req_session_id: String,
    channel: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
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

#[expect(
    clippy::too_many_arguments,
    reason = "channel unsubscribe updates membership, delivery, and swarm event history together"
)]
pub(super) async fn handle_comm_unsubscribe_channel(
    id: u64,
    req_session_id: String,
    channel: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
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
