use super::{
    SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan, broadcast_swarm_plan,
    record_swarm_event,
};
use crate::agent::Agent;
use crate::protocol::{NotificationType, ServerEvent};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

async fn ensure_same_swarm_access(
    id: u64,
    req_session_id: &str,
    target_session: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) -> bool {
    let (req_swarm, target_swarm) = {
        let members = swarm_members.read().await;
        (
            members
                .get(req_session_id)
                .and_then(|member| member.swarm_id.clone()),
            members
                .get(target_session)
                .and_then(|member| member.swarm_id.clone()),
        )
    };

    if req_swarm.is_some() && req_swarm == target_swarm {
        true
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!(
                "Session '{}' is not in the same swarm as requester '{}'",
                target_session, req_session_id
            ),
            retry_after_secs: None,
        });
        false
    }
}

async fn can_read_full_context(
    req_session_id: &str,
    target_session: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    if req_session_id == target_session {
        return true;
    }

    let members = swarm_members.read().await;
    members
        .get(req_session_id)
        .map(|member| member.role == "coordinator" || member.role == "worktree_manager")
        .unwrap_or(false)
}

pub(super) async fn handle_comm_summary(
    id: u64,
    req_session_id: String,
    target_session: String,
    limit: Option<usize>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if !ensure_same_swarm_access(
        id,
        &req_session_id,
        &target_session,
        swarm_members,
        client_event_tx,
    )
    .await
    {
        return;
    }

    let limit = limit.unwrap_or(10);
    let agent_sessions = sessions.read().await;
    if let Some(agent) = agent_sessions.get(&target_session) {
        let tool_calls = if let Ok(agent) = agent.try_lock() {
            agent.get_tool_call_summaries(limit)
        } else {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "Session '{}' is busy; try summary again shortly",
                    target_session
                ),
                retry_after_secs: Some(1),
            });
            return;
        };
        let _ = client_event_tx.send(ServerEvent::CommSummaryResponse {
            id,
            session_id: target_session,
            tool_calls,
        });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!("Unknown session '{target_session}'"),
            retry_after_secs: None,
        });
    }
}

pub(super) async fn handle_comm_read_context(
    id: u64,
    req_session_id: String,
    target_session: String,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if !ensure_same_swarm_access(
        id,
        &req_session_id,
        &target_session,
        swarm_members,
        client_event_tx,
    )
    .await
    {
        return;
    }

    if !can_read_full_context(&req_session_id, &target_session, swarm_members).await {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Only the coordinator, worktree manager, or the target session may read full context. Use summary for lightweight access.".to_string(),
            retry_after_secs: None,
        });
        return;
    }

    let agent_sessions = sessions.read().await;
    if let Some(agent) = agent_sessions.get(&target_session) {
        let messages = if let Ok(agent) = agent.try_lock() {
            agent.get_history()
        } else {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "Session '{}' is busy; try read_context again shortly",
                    target_session
                ),
                retry_after_secs: Some(1),
            });
            return;
        };
        let _ = client_event_tx.send(ServerEvent::CommContextHistory {
            id,
            session_id: target_session,
            messages,
        });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!("Unknown session '{target_session}'"),
            retry_after_secs: None,
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_resync_plan(
    id: u64,
    req_session_id: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = {
        let members = swarm_members.read().await;
        members
            .get(&req_session_id)
            .and_then(|member| member.swarm_id.clone())
    };

    if let Some(swarm_id) = swarm_id {
        let plan_state = {
            let mut plans = swarm_plans.write().await;
            plans.get_mut(&swarm_id).map(|plan| {
                plan.participants.insert(req_session_id.clone());
                (plan.version, plan.items.len())
            })
        };
        if let Some((version, item_count)) = plan_state {
            if let Some(member) = swarm_members.read().await.get(&req_session_id) {
                let _ = member.event_tx.send(ServerEvent::Notification {
                    from_session: req_session_id.clone(),
                    from_name: member.friendly_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("plan".to_string()),
                        channel: None,
                    },
                    message: format!(
                        "Plan attached to this session (v{}, {} items).",
                        version, item_count
                    ),
                });
            }
            broadcast_swarm_plan(
                &swarm_id,
                Some("resync".to_string()),
                swarm_plans,
                swarm_members,
                swarms_by_id,
            )
            .await;
            record_swarm_event(
                event_history,
                event_counter,
                swarm_event_tx,
                req_session_id.clone(),
                None,
                Some(swarm_id.clone()),
                SwarmEventType::PlanUpdate {
                    swarm_id: swarm_id.clone(),
                    item_count,
                },
            )
            .await;
            let _ = client_event_tx.send(ServerEvent::Done { id });
        } else {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: "No swarm plan exists for this swarm.".to_string(),
                retry_after_secs: None,
            });
        }
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm.".to_string(),
            retry_after_secs: None,
        });
    }
}
