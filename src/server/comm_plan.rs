use super::{
    SessionInterruptQueues, SharedContext, SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan,
    broadcast_swarm_plan, queue_soft_interrupt_for_session, record_swarm_event,
    summarize_plan_items,
};
use crate::agent::Agent;
use crate::plan::PlanItem;
use crate::protocol::{NotificationType, ServerEvent};
use jcode_agent_runtime::SoftInterruptSource;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_propose_plan(
    id: u64,
    req_session_id: String,
    items: Vec<PlanItem>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
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

    let swarm_id = match swarm_id.as_ref() {
        Some(swarm_id) => swarm_id.clone(),
        None => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: "Not in a swarm.".to_string(),
                retry_after_secs: None,
            });
            return;
        }
    };

    let (from_name, coordinator_id) = {
        let members = swarm_members.read().await;
        let from_name = members
            .get(&req_session_id)
            .and_then(|member| member.friendly_name.clone());
        let coordinators = swarm_coordinators.read().await;
        let coordinator_id = coordinators.get(&swarm_id).cloned();
        (from_name, coordinator_id)
    };
    let from_label = from_name
        .clone()
        .unwrap_or_else(|| req_session_id.chars().take(8).collect());

    let Some(coordinator_id) = coordinator_id else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "No coordinator for this swarm.".to_string(),
            retry_after_secs: None,
        });
        return;
    };

    if coordinator_id == req_session_id {
        let (version, participant_ids) = {
            let mut plans = swarm_plans.write().await;
            let plan = plans
                .entry(swarm_id.clone())
                .or_insert_with(VersionedPlan::new);
            plan.participants.insert(req_session_id.clone());
            for item in &items {
                if let Some(owner) = &item.assigned_to {
                    plan.participants.insert(owner.clone());
                }
            }
            plan.items = items.clone();
            plan.version += 1;
            (plan.version, plan.participants.clone())
        };

        let members = swarm_members.read().await;
        let notification_msg = format!(
            "Plan updated by {} ({} items, v{})",
            from_label,
            items.len(),
            version
        );
        for sid in participant_ids {
            if sid == req_session_id {
                continue;
            }
            if let Some(member) = members.get(&sid) {
                let _ = member.event_tx.send(ServerEvent::Notification {
                    from_session: req_session_id.clone(),
                    from_name: from_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("plan".to_string()),
                        channel: None,
                    },
                    message: notification_msg.clone(),
                });
            }
            let _ = queue_soft_interrupt_for_session(
                &sid,
                notification_msg.clone(),
                false,
                SoftInterruptSource::System,
                soft_interrupt_queues,
                sessions,
            )
            .await;
        }

        broadcast_swarm_plan(
            &swarm_id,
            Some("coordinator_direct_update".to_string()),
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
            from_name.clone(),
            Some(swarm_id.clone()),
            SwarmEventType::PlanUpdate {
                swarm_id: swarm_id.clone(),
                item_count: items.len(),
            },
        )
        .await;

        let _ = client_event_tx.send(ServerEvent::Done { id });
        return;
    }

    let proposal_key = format!("plan_proposal:{req_session_id}");
    let proposal_value = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
    {
        let mut context = shared_context.write().await;
        let swarm_context = context.entry(swarm_id.clone()).or_insert_with(HashMap::new);
        let now = Instant::now();
        swarm_context.insert(
            proposal_key.clone(),
            SharedContext {
                key: proposal_key.clone(),
                value: proposal_value,
                from_session: req_session_id.clone(),
                from_name: from_name.clone(),
                created_at: now,
                updated_at: now,
            },
        );
    }
    record_swarm_event(
        event_history,
        event_counter,
        swarm_event_tx,
        req_session_id.clone(),
        from_name.clone(),
        Some(swarm_id.clone()),
        SwarmEventType::PlanProposal {
            swarm_id: swarm_id.clone(),
            proposer_session: req_session_id.clone(),
            item_count: items.len(),
        },
    )
    .await;

    let summary = summarize_plan_items(&items, 3);
    let notification_msg = format!(
        "Plan proposal from {} ({} items). Summary: {}. Review with communicate read key '{}'.",
        from_label,
        items.len(),
        summary,
        proposal_key
    );

    let members = swarm_members.read().await;
    if let Some(member) = members.get(&coordinator_id) {
        let _ = member.event_tx.send(ServerEvent::Notification {
            from_session: req_session_id.clone(),
            from_name: from_name.clone(),
            notification_type: NotificationType::Message {
                scope: Some("plan_proposal".to_string()),
                channel: None,
            },
            message: notification_msg.clone(),
        });
        let _ = member.event_tx.send(ServerEvent::SwarmPlanProposal {
            swarm_id: swarm_id.clone(),
            proposer_session: req_session_id.clone(),
            proposer_name: from_name.clone(),
            items: items.clone(),
            summary: summary.clone(),
            proposal_key: proposal_key.clone(),
        });
    }
    let _ = queue_soft_interrupt_for_session(
        &coordinator_id,
        notification_msg.clone(),
        false,
        SoftInterruptSource::System,
        soft_interrupt_queues,
        sessions,
    )
    .await;

    let proposer_confirmation = "Plan proposal sent to coordinator (not yet applied).".to_string();
    if let Some(member) = members.get(&req_session_id) {
        let _ = member.event_tx.send(ServerEvent::Notification {
            from_session: req_session_id.clone(),
            from_name: from_name.clone(),
            notification_type: NotificationType::Message {
                scope: Some("plan_proposal".to_string()),
                channel: None,
            },
            message: proposer_confirmation.clone(),
        });
    }
    let _ = queue_soft_interrupt_for_session(
        &req_session_id,
        proposer_confirmation,
        false,
        SoftInterruptSource::System,
        soft_interrupt_queues,
        sessions,
    )
    .await;

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_approve_plan(
    id: u64,
    req_session_id: String,
    proposer_session: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = match require_coordinator_swarm(
        id,
        &req_session_id,
        "Only the coordinator can approve plan proposals.",
        client_event_tx,
        swarm_members,
        swarm_coordinators,
    )
    .await
    {
        Some(swarm_id) => swarm_id,
        None => return,
    };

    let proposal_key = format!("plan_proposal:{proposer_session}");
    let proposal_value = {
        let context = shared_context.read().await;
        context
            .get(&swarm_id)
            .and_then(|swarm_context| swarm_context.get(&proposal_key))
            .map(|context| context.value.clone())
    };

    let proposal = match proposal_value {
        Some(proposal) => proposal,
        None => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("No pending plan proposal from session '{proposer_session}'"),
                retry_after_secs: None,
            });
            return;
        }
    };

    if let Ok(items) = serde_json::from_str::<Vec<PlanItem>>(&proposal) {
        let participant_ids = {
            let mut plans = swarm_plans.write().await;
            let plan = plans
                .entry(swarm_id.clone())
                .or_insert_with(VersionedPlan::new);
            plan.items.extend(items.clone());
            plan.version += 1;
            plan.participants.insert(req_session_id.clone());
            plan.participants.insert(proposer_session.clone());
            for item in &items {
                if let Some(owner) = &item.assigned_to {
                    plan.participants.insert(owner.clone());
                }
            }
            plan.participants.clone()
        };

        {
            let mut context = shared_context.write().await;
            if let Some(swarm_context) = context.get_mut(&swarm_id) {
                swarm_context.remove(&proposal_key);
            }
        }

        broadcast_swarm_plan(
            &swarm_id,
            Some("proposal_approved".to_string()),
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
                item_count: items.len(),
            },
        )
        .await;

        let coordinator_name = {
            let members = swarm_members.read().await;
            members
                .get(&req_session_id)
                .and_then(|member| member.friendly_name.clone())
        };

        let members = swarm_members.read().await;
        for sid in participant_ids {
            if let Some(member) = members.get(&sid) {
                let message = format!(
                    "Plan approved by coordinator: {} items added from {}",
                    items.len(),
                    proposer_session
                );
                let _ = member.event_tx.send(ServerEvent::Notification {
                    from_session: req_session_id.clone(),
                    from_name: coordinator_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("plan".to_string()),
                        channel: None,
                    },
                    message: message.clone(),
                });

                let _ = queue_soft_interrupt_for_session(
                    &sid,
                    message.clone(),
                    false,
                    SoftInterruptSource::System,
                    soft_interrupt_queues,
                    sessions,
                )
                .await;
            }
        }
    }

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_reject_plan(
    id: u64,
    req_session_id: String,
    proposer_session: String,
    reason: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = match require_coordinator_swarm(
        id,
        &req_session_id,
        "Only the coordinator can reject plan proposals.",
        client_event_tx,
        swarm_members,
        swarm_coordinators,
    )
    .await
    {
        Some(swarm_id) => swarm_id,
        None => return,
    };

    let proposal_key = format!("plan_proposal:{proposer_session}");
    let proposal_exists = {
        let context = shared_context.read().await;
        context
            .get(&swarm_id)
            .and_then(|swarm_context| swarm_context.get(&proposal_key))
            .is_some()
    };

    if !proposal_exists {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!("No pending plan proposal from session '{proposer_session}'"),
            retry_after_secs: None,
        });
        return;
    }

    {
        let mut context = shared_context.write().await;
        if let Some(swarm_context) = context.get_mut(&swarm_id) {
            swarm_context.remove(&proposal_key);
        }
    }

    let coordinator_name = {
        let members = swarm_members.read().await;
        members
            .get(&req_session_id)
            .and_then(|member| member.friendly_name.clone())
    };

    let members = swarm_members.read().await;
    if let Some(member) = members.get(&proposer_session) {
        let reason_msg = reason
            .as_ref()
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default();
        let message = format!("Your plan proposal was rejected by the coordinator{reason_msg}");
        let _ = member.event_tx.send(ServerEvent::Notification {
            from_session: req_session_id.clone(),
            from_name: coordinator_name.clone(),
            notification_type: NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
            },
            message: message.clone(),
        });

        let _ = queue_soft_interrupt_for_session(
            &proposer_session,
            message,
            false,
            SoftInterruptSource::System,
            soft_interrupt_queues,
            sessions,
        )
        .await;
    }
    record_swarm_event(
        event_history,
        event_counter,
        swarm_event_tx,
        req_session_id.clone(),
        coordinator_name,
        Some(swarm_id.clone()),
        SwarmEventType::Notification {
            notification_type: "plan_rejected".to_string(),
            message: proposer_session.clone(),
        },
    )
    .await;

    let _ = client_event_tx.send(ServerEvent::Done { id });
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
