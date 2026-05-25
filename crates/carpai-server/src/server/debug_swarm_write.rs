use super::{SharedContext, SwarmMember, SwarmState, VersionedPlan, persist_swarm_state_for};
use crate::plan::PlanItem;
use crate::protocol::{NotificationType, ServerEvent};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

pub(super) struct DebugSwarmWriteContext<'a> {
    pub(super) session_id: &'a Arc<RwLock<String>>,
    pub(super) swarm_members: &'a Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub(super) swarms_by_id: &'a Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub(super) shared_context: &'a Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    pub(super) swarm_plans: &'a Arc<RwLock<HashMap<String, VersionedPlan>>>,
    pub(super) swarm_coordinators: &'a Arc<RwLock<HashMap<String, String>>>,
}

pub(super) async fn maybe_handle_swarm_write_command(
    cmd: &str,
    ctx: &DebugSwarmWriteContext<'_>,
) -> Result<Option<String>> {
    if cmd.starts_with("swarm:clear_coordinator:") {
        let swarm_id = cmd
            .strip_prefix("swarm:clear_coordinator:")
            .unwrap_or("")
            .trim();
        let mut coordinators = ctx.swarm_coordinators.write().await;
        if coordinators.remove(swarm_id).is_some() {
            let mut members = ctx.swarm_members.write().await;
            for member in members.values_mut() {
                if member.swarm_id.as_deref() == Some(swarm_id) && member.role == "coordinator" {
                    member.role = "agent".to_string();
                }
            }
            drop(members);
            let swarm_state = SwarmState {
                members: Arc::clone(ctx.swarm_members),
                swarms_by_id: Arc::clone(ctx.swarms_by_id),
                plans: Arc::clone(ctx.swarm_plans),
                coordinators: Arc::clone(ctx.swarm_coordinators),
            };
            persist_swarm_state_for(swarm_id, &swarm_state).await;
            return Ok(Some(format!(
                "Coordinator cleared for swarm '{}'. Any session can now self-promote.",
                swarm_id
            )));
        }
        return Err(anyhow::anyhow!(
            "No coordinator set for swarm '{}'",
            swarm_id
        ));
    }

    if cmd.starts_with("swarm:broadcast:") {
        let rest = cmd.strip_prefix("swarm:broadcast:").unwrap_or("").trim();
        let (target_swarm_id, message) = if let Some(space_idx) = rest.find(' ') {
            let potential_id = &rest[..space_idx];
            let msg = rest[space_idx + 1..].trim();
            if potential_id.contains('/') {
                (Some(potential_id.to_string()), msg.to_string())
            } else {
                (None, rest.to_string())
            }
        } else {
            (None, rest.to_string())
        };

        if message.is_empty() {
            return Err(anyhow::anyhow!("swarm:broadcast requires a message"));
        }

        let swarm_id = if let Some(id) = target_swarm_id {
            Some(id)
        } else {
            let members = ctx.swarm_members.read().await;
            let current_session = ctx.session_id.read().await;
            members
                .get(&*current_session)
                .and_then(|member| member.swarm_id.clone())
        };

        if let Some(swarm_id) = swarm_id {
            let swarms = ctx.swarms_by_id.read().await;
            let members = ctx.swarm_members.read().await;
            let current_session = ctx.session_id.read().await;
            let from_name = members
                .get(&*current_session)
                .and_then(|member| member.friendly_name.clone());

            if let Some(member_ids) = swarms.get(&swarm_id) {
                let mut sent_count = 0;
                for member_id in member_ids {
                    if let Some(member) = members.get(member_id) {
                        let notification = ServerEvent::Notification {
                            from_session: current_session.clone(),
                            from_name: from_name.clone(),
                            notification_type: NotificationType::Message {
                                scope: Some("broadcast".to_string()),
                                channel: None,
                            },
                            message: message.clone(),
                        };
                        if member.event_tx.send(notification).is_ok() {
                            sent_count += 1;
                        }
                    }
                }
                return Ok(Some(
                    serde_json::json!({
                        "swarm_id": swarm_id,
                        "message": message,
                        "sent_to": sent_count,
                    })
                    .to_string(),
                ));
            }

            return Err(anyhow::anyhow!("No members in swarm '{}'", swarm_id));
        }

        return Err(anyhow::anyhow!(
            "No swarm found. Specify swarm_id: swarm:broadcast:<swarm_id> <message>"
        ));
    }

    if cmd.starts_with("swarm:notify:") {
        let rest = cmd.strip_prefix("swarm:notify:").unwrap_or("").trim();
        if let Some(space_idx) = rest.find(' ') {
            let target_session = &rest[..space_idx];
            let message = rest[space_idx + 1..].trim();

            if message.is_empty() {
                return Err(anyhow::anyhow!("swarm:notify requires a message"));
            }

            let members = ctx.swarm_members.read().await;
            let current_session = ctx.session_id.read().await;
            let from_name = members
                .get(&*current_session)
                .and_then(|member| member.friendly_name.clone());

            if let Some(target) = members.get(target_session) {
                let notification = ServerEvent::Notification {
                    from_session: current_session.clone(),
                    from_name: from_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("dm".to_string()),
                        channel: None,
                    },
                    message: message.to_string(),
                };
                if target.event_tx.send(notification).is_ok() {
                    return Ok(Some(
                        serde_json::json!({
                            "sent_to": target_session,
                            "sent_to_name": target.friendly_name.clone(),
                            "message": message,
                        })
                        .to_string(),
                    ));
                }
                return Err(anyhow::anyhow!("Failed to send notification"));
            }

            return Err(anyhow::anyhow!("Unknown session '{}'", target_session));
        }

        return Err(anyhow::anyhow!(
            "Usage: swarm:notify:<session_id> <message>"
        ));
    }

    if cmd.starts_with("swarm:set_context:") {
        let rest = cmd.strip_prefix("swarm:set_context:").unwrap_or("").trim();
        let parts: Vec<&str> = rest.splitn(3, ' ').collect();
        if parts.len() < 3 {
            return Err(anyhow::anyhow!(
                "Usage: swarm:set_context:<session_id> <key> <value>"
            ));
        }

        let acting_session = parts[0];
        let key = parts[1].to_string();
        let value = parts[2].to_string();

        let (swarm_id, friendly_name) = {
            let members = ctx.swarm_members.read().await;
            let swarm_id = members
                .get(acting_session)
                .and_then(|member| member.swarm_id.clone());
            let name = members
                .get(acting_session)
                .and_then(|member| member.friendly_name.clone());
            (swarm_id, name)
        };

        if let Some(swarm_id) = swarm_id {
            {
                let mut shared_ctx = ctx.shared_context.write().await;
                let swarm_ctx = shared_ctx
                    .entry(swarm_id.clone())
                    .or_insert_with(HashMap::new);
                let now = Instant::now();
                let created_at = swarm_ctx
                    .get(&key)
                    .map(|context| context.created_at)
                    .unwrap_or(now);
                swarm_ctx.insert(
                    key.clone(),
                    SharedContext {
                        key: key.clone(),
                        value: value.clone(),
                        from_session: acting_session.to_string(),
                        from_name: friendly_name.clone(),
                        created_at,
                        updated_at: now,
                    },
                );
            }

            let swarm_session_ids: Vec<String> = {
                let swarms = ctx.swarms_by_id.read().await;
                swarms
                    .get(&swarm_id)
                    .map(|sessions| sessions.iter().cloned().collect())
                    .unwrap_or_default()
            };
            let members = ctx.swarm_members.read().await;
            for sid in &swarm_session_ids {
                if sid != acting_session
                    && let Some(member) = members.get(sid)
                {
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: acting_session.to_string(),
                        from_name: friendly_name.clone(),
                        notification_type: NotificationType::SharedContext {
                            key: key.clone(),
                            value: value.clone(),
                        },
                        message: format!("Shared context: {} = {}", key, value),
                    });
                }
            }

            return Ok(Some(
                serde_json::json!({
                    "swarm_id": swarm_id,
                    "key": key,
                    "value": value,
                    "from_session": acting_session,
                })
                .to_string(),
            ));
        }

        return Err(anyhow::anyhow!(
            "Session '{}' is not in a swarm",
            acting_session
        ));
    }

    if cmd.starts_with("swarm:approve_plan:") {
        let rest = cmd.strip_prefix("swarm:approve_plan:").unwrap_or("").trim();
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        if parts.len() < 2 {
            return Err(anyhow::anyhow!(
                "Usage: swarm:approve_plan:<coordinator_session> <proposer_session>"
            ));
        }

        let coord_session = parts[0];
        let proposer_session = parts[1];

        let (swarm_id, is_coordinator) = {
            let members = ctx.swarm_members.read().await;
            let swarm_id = members
                .get(coord_session)
                .and_then(|member| member.swarm_id.clone());
            let is_coord = if let Some(ref swarm_id) = swarm_id {
                let coordinators = ctx.swarm_coordinators.read().await;
                coordinators
                    .get(swarm_id)
                    .map(|coordinator| coordinator == coord_session)
                    .unwrap_or(false)
            } else {
                false
            };
            (swarm_id, is_coord)
        };

        if !is_coordinator {
            return Err(anyhow::anyhow!(
                "Only the coordinator can approve plan proposals."
            ));
        }

        if let Some(swarm_id) = swarm_id {
            let proposal_key = format!("plan_proposal:{}", proposer_session);
            let proposal_value = {
                let shared_ctx = ctx.shared_context.read().await;
                shared_ctx
                    .get(&swarm_id)
                    .and_then(|swarm_ctx| swarm_ctx.get(&proposal_key))
                    .map(|context| context.value.clone())
            };

            return match proposal_value {
                None => Err(anyhow::anyhow!(
                    "No pending plan proposal from session '{}'",
                    proposer_session
                )),
                Some(proposal) => {
                    if let Ok(items) = serde_json::from_str::<Vec<PlanItem>>(&proposal) {
                        let version = {
                            let mut plans = ctx.swarm_plans.write().await;
                            let versioned_plan = plans
                                .entry(swarm_id.clone())
                                .or_insert_with(VersionedPlan::new);
                            versioned_plan.items.extend(items.clone());
                            versioned_plan.version += 1;
                            versioned_plan
                                .participants
                                .insert(coord_session.to_string());
                            versioned_plan
                                .participants
                                .insert(proposer_session.to_string());
                            versioned_plan.version
                        };
                        {
                            let mut shared_ctx = ctx.shared_context.write().await;
                            if let Some(swarm_ctx) = shared_ctx.get_mut(&swarm_id) {
                                swarm_ctx.remove(&proposal_key);
                            }
                        }
                        Ok(Some(
                            serde_json::json!({
                                "approved": true,
                                "items_added": items.len(),
                                "plan_version": version,
                                "swarm_id": swarm_id,
                            })
                            .to_string(),
                        ))
                    } else {
                        Err(anyhow::anyhow!(
                            "Failed to parse plan proposal as Vec<PlanItem>"
                        ))
                    }
                }
            };
        }

        return Err(anyhow::anyhow!("Not in a swarm."));
    }

    if cmd.starts_with("swarm:reject_plan:") {
        let rest = cmd.strip_prefix("swarm:reject_plan:").unwrap_or("").trim();
        let parts: Vec<&str> = rest.splitn(3, ' ').collect();
        if parts.len() < 2 {
            return Err(anyhow::anyhow!(
                "Usage: swarm:reject_plan:<coordinator_session> <proposer_session> [reason]"
            ));
        }

        let coord_session = parts[0];
        let proposer_session = parts[1];
        let reason = if parts.len() >= 3 {
            Some(parts[2].to_string())
        } else {
            None
        };

        let (swarm_id, is_coordinator) = {
            let members = ctx.swarm_members.read().await;
            let swarm_id = members
                .get(coord_session)
                .and_then(|member| member.swarm_id.clone());
            let is_coord = if let Some(ref swarm_id) = swarm_id {
                let coordinators = ctx.swarm_coordinators.read().await;
                coordinators
                    .get(swarm_id)
                    .map(|coordinator| coordinator == coord_session)
                    .unwrap_or(false)
            } else {
                false
            };
            (swarm_id, is_coord)
        };

        if !is_coordinator {
            return Err(anyhow::anyhow!(
                "Only the coordinator can reject plan proposals."
            ));
        }

        if let Some(swarm_id) = swarm_id {
            let proposal_key = format!("plan_proposal:{}", proposer_session);
            let proposal_exists = {
                let shared_ctx = ctx.shared_context.read().await;
                shared_ctx
                    .get(&swarm_id)
                    .and_then(|swarm_ctx| swarm_ctx.get(&proposal_key))
                    .is_some()
            };

            if !proposal_exists {
                return Err(anyhow::anyhow!(
                    "No pending plan proposal from session '{}'",
                    proposer_session
                ));
            }

            {
                let mut shared_ctx = ctx.shared_context.write().await;
                if let Some(swarm_ctx) = shared_ctx.get_mut(&swarm_id) {
                    swarm_ctx.remove(&proposal_key);
                }
            }
            let reason_msg = reason
                .as_ref()
                .map(|reason| format!(": {}", reason))
                .unwrap_or_default();
            return Ok(Some(
                serde_json::json!({
                    "rejected": true,
                    "proposer_session": proposer_session,
                    "reason": reason_msg,
                    "swarm_id": swarm_id,
                })
                .to_string(),
            ));
        }

        return Err(anyhow::anyhow!("Not in a swarm."));
    }

    Ok(None)
}
