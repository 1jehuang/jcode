use super::state::ChannelIndex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

async fn with_channel_index_mut(
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    mutate: impl FnOnce(&mut ChannelIndex),
) {
    let mut subs = channel_subscriptions.write().await;
    let mut reverse = channel_subscriptions_by_session.write().await;
    let mut index = ChannelIndex {
        by_swarm_channel: std::mem::take(&mut *subs),
        by_session: std::mem::take(&mut *reverse),
    };
    mutate(&mut index);
    *subs = index.by_swarm_channel;
    *reverse = index.by_session;
}

pub(super) async fn remove_session_channel_subscriptions(
    session_id: &str,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
) {
    with_channel_index_mut(
        channel_subscriptions,
        channel_subscriptions_by_session,
        |index| index.remove_session(session_id),
    )
    .await;
}

pub(super) async fn subscribe_session_to_channel(
    session_id: &str,
    swarm_id: &str,
    channel: &str,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
) {
    with_channel_index_mut(
        channel_subscriptions,
        channel_subscriptions_by_session,
        |index| index.subscribe(session_id, swarm_id, channel),
    )
    .await;
}

pub(super) async fn unsubscribe_session_from_channel(
    session_id: &str,
    swarm_id: &str,
    channel: &str,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
) {
    with_channel_index_mut(
        channel_subscriptions,
        channel_subscriptions_by_session,
        |index| index.unsubscribe(session_id, swarm_id, channel),
    )
    .await;
}

pub(super) async fn list_channels_for_swarm(
    swarm_id: &str,
    channel_subscriptions: &ChannelSubscriptions,
) -> Vec<(String, usize)> {
    let subs = channel_subscriptions.read().await;
    let index = ChannelIndex {
        by_swarm_channel: subs.clone(),
        by_session: HashMap::new(),
    };
    let mut channels = index
        .by_swarm_channel
        .get(swarm_id)
        .map(|swarm_channels| {
            swarm_channels
                .iter()
                .map(|(channel, members)| (channel.clone(), members.len()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    channels.sort_by(|left, right| left.0.cmp(&right.0));
    channels
}
