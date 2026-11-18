//! Bounded swarm event history publication.

use super::*;

pub(super) fn log_swarm_lifecycle(phase: &str, fields: Vec<(&str, String)>) {
    crate::logging::event_info(
        "SWARM_LIFECYCLE",
        Vec::from([("phase", phase.to_string())])
            .into_iter()
            .chain(fields)
            .collect::<Vec<_>>(),
    );
}

pub(in crate::server) async fn record_swarm_event(
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    session_id: String,
    session_name: Option<String>,
    swarm_id: Option<String>,
    event: SwarmEventType,
) {
    let swarm_event = SwarmEvent {
        id: event_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        session_id,
        session_name,
        swarm_id,
        event,
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    };
    let _ = swarm_event_tx.send(swarm_event.clone());
    let mut history = event_history.write().await;
    history.push_back(swarm_event);
    if history.len() > MAX_EVENT_HISTORY {
        history.pop_front();
    }
}

pub(in crate::server) async fn record_swarm_event_for_session(
    session_id: &str,
    event: SwarmEventType,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let (session_name, swarm_id) = {
        let members = swarm_members.read().await;
        if let Some(member) = members.get(session_id) {
            (member.friendly_name.clone(), member.swarm_id.clone())
        } else {
            (None, None)
        }
    };
    record_swarm_event(
        event_history,
        event_counter,
        swarm_event_tx,
        session_id.to_string(),
        session_name,
        swarm_id,
        event,
    )
    .await;
}
