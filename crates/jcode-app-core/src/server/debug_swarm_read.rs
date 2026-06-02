use super::swarm_channels::list_channels_for_swarm;
use super::{
    FileAccess, ServerIdentity, SharedContext, SwarmMember, SwarmState, VersionedPlan,
    git_common_dir_for, swarm_id_for_dir,
};
use crate::agent::Agent;
use crate::plan::{next_runnable_item_ids, summarize_plan_graph};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

#[expect(
    clippy::too_many_arguments,
    reason = "swarm read debug commands inspect sessions, swarm state, shared context, plans, channels, and file touches together"
)]
pub(super) async fn maybe_handle_swarm_read_command(
    cmd: &str,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    channel_subscriptions: &ChannelSubscriptions,
    server_identity: &ServerIdentity,
) -> Result<Option<String>> {
    let swarm_state = SwarmState {
        members: Arc::clone(swarm_members),
        swarms_by_id: Arc::clone(swarms_by_id),
        plans: Arc::clone(swarm_plans),
        coordinators: Arc::clone(swarm_coordinators),
    };

    if cmd == "swarm" || cmd == "swarm_status" || cmd == "swarm:members" {
        let members = swarm_members.read().await;
        let sessions_guard = sessions.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for member in members.values() {
            let (provider, model) = if let Some(agent_arc) = sessions_guard.get(&member.session_id)
            {
                if let Ok(agent) = agent_arc.try_lock() {
                    (Some(agent.provider_name()), Some(agent.provider_model()))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };
            out.push(serde_json::json!({
                "session_id": member.session_id,
                "friendly_name": member.friendly_name,
                "swarm_id": member.swarm_id,
                "working_dir": member.working_dir,
                "status": member.status,
                "detail": member.detail,
                "role": member.role,
                "is_headless": member.is_headless,
                "live_attachments": member.event_txs.len(),
                "joined_secs_ago": member.joined_at.elapsed().as_secs(),
                "status_changed_secs_ago": member.last_status_change.elapsed().as_secs(),
                "provider": provider,
                "model": model,
                "server_name": server_identity.name,
                "server_icon": server_identity.icon,
            }));
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd == "swarm:list" {
        let swarms = swarms_by_id.read().await;
        let coordinators = swarm_coordinators.read().await;
        let members = swarm_members.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (swarm_id, session_ids) in swarms.iter() {
            let coordinator = coordinators.get(swarm_id);
            let coordinator_name =
                coordinator.and_then(|cid| members.get(cid).and_then(|m| m.friendly_name.clone()));
            let mut status_counts: HashMap<String, usize> = HashMap::new();
            let mut headless_count = 0usize;
            let mut attached_member_count = 0usize;
            let mut live_attachment_count = 0usize;
            let member_details: Vec<serde_json::Value> = session_ids
                .iter()
                .filter_map(|session_id| members.get(session_id))
                .map(|member| {
                    *status_counts.entry(member.status.clone()).or_default() += 1;
                    if member.is_headless {
                        headless_count += 1;
                    }
                    if !member.event_txs.is_empty() {
                        attached_member_count += 1;
                    }
                    live_attachment_count += member.event_txs.len();
                    serde_json::json!({
                        "session_id": member.session_id,
                        "friendly_name": member.friendly_name,
                        "status": member.status,
                        "detail": member.detail,
                        "role": member.role,
                        "is_headless": member.is_headless,
                        "live_attachments": member.event_txs.len(),
                    })
                })
                .collect();
            out.push(serde_json::json!({
                "swarm_id": swarm_id,
                "member_count": session_ids.len(),
                "members": session_ids.iter().collect::<Vec<_>>(),
                "coordinator": coordinator,
                "coordinator_name": coordinator_name,
                "headless_count": headless_count,
                "attached_member_count": attached_member_count,
                "live_attachment_count": live_attachment_count,
                "status_counts": status_counts,
                "member_details": member_details,
            }));
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd == "swarm:coordinators" {
        let coordinators = swarm_coordinators.read().await;
        let members = swarm_members.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (swarm_id, session_id) in coordinators.iter() {
            let name = members
                .get(session_id)
                .and_then(|m| m.friendly_name.clone());
            out.push(serde_json::json!({
                "swarm_id": swarm_id,
                "coordinator_session": session_id,
                "coordinator_name": name,
            }));
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("swarm:coordinator:") {
        let swarm_id = cmd.strip_prefix("swarm:coordinator:").unwrap_or("").trim();
        let coordinators = swarm_coordinators.read().await;
        let members = swarm_members.read().await;
        let output = if let Some(session_id) = coordinators.get(swarm_id) {
            let name = members
                .get(session_id)
                .and_then(|m| m.friendly_name.clone());
            serde_json::json!({
                "swarm_id": swarm_id,
                "coordinator_session": session_id,
                "coordinator_name": name,
            })
            .to_string()
        } else {
            return Err(anyhow::anyhow!("No coordinator for swarm '{}'", swarm_id));
        };
        return Ok(Some(output));
    }

    if cmd == "swarm:roles" {
        let members = swarm_members.read().await;
        let coordinators = swarm_coordinators.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (sid, member) in members.iter() {
            let is_coordinator = member
                .swarm_id
                .as_ref()
                .map(|swid| coordinators.get(swid).map(|c| c == sid).unwrap_or(false))
                .unwrap_or(false);
            out.push(serde_json::json!({
                "session_id": sid,
                "friendly_name": member.friendly_name,
                "role": member.role,
                "swarm_id": member.swarm_id,
                "status": member.status,
                "is_coordinator": is_coordinator,
            }));
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd == "swarm:channels" {
        let subs = channel_subscriptions.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for swarm_id in subs.keys() {
            let channels = list_channels_for_swarm(swarm_id, channel_subscriptions).await;
            let mut channel_data: Vec<serde_json::Value> = Vec::new();
            for (channel, member_count) in channels {
                channel_data.push(serde_json::json!({
                    "channel": channel,
                    "count": member_count,
                }));
            }
            out.push(serde_json::json!({
                "swarm_id": swarm_id,
                "channels": channel_data,
            }));
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("swarm:plan_version:") {
        let swarm_id = cmd.strip_prefix("swarm:plan_version:").unwrap_or("").trim();
        let runtime = swarm_state.load_runtime(swarm_id).await;
        let output = if let Some(vp) = runtime.plan.as_ref() {
            let summary = summarize_plan_graph(&vp.items);
            let next_ready_ids = next_runnable_item_ids(&vp.items, Some(8));
            serde_json::json!({
                "swarm_id": runtime.swarm_id,
                "version": vp.version,
                "item_count": vp.items.len(),
                "member_count": runtime.members.len(),
                "coordinator": runtime.coordinator_session_id,
                "stale_item_count": vp.items.iter().filter(|item| item.status == "running_stale").count(),
                "ready_item_count": summary.ready_ids.len(),
                "blocked_item_count": summary.blocked_ids.len(),
                "active_item_count": summary.active_ids.len(),
                "completed_item_count": summary.completed_ids.len(),
                "next_ready_ids": next_ready_ids,
                "cycle_ids": summary.cycle_ids,
                "unresolved_dependency_ids": summary.unresolved_dependency_ids,
            })
            .to_string()
        } else {
            serde_json::json!({
                "swarm_id": swarm_id,
                "version": 0,
                "item_count": 0,
                "stale_item_count": 0,
                "ready_item_count": 0,
                "blocked_item_count": 0,
                "active_item_count": 0,
                "completed_item_count": 0,
                "next_ready_ids": Vec::<String>::new(),
                "cycle_ids": Vec::<String>::new(),
                "unresolved_dependency_ids": Vec::<String>::new(),
            })
            .to_string()
        };
        return Ok(Some(output));
    }

    if cmd == "swarm:plans" {
        let plans = swarm_plans.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for swarm_id in plans.keys() {
            let runtime = swarm_state.load_runtime(swarm_id).await;
            let Some(vp) = runtime.plan.as_ref() else {
                continue;
            };
            let summary = summarize_plan_graph(&vp.items);
            let next_ready_ids = next_runnable_item_ids(&vp.items, Some(8));
            out.push(serde_json::json!({
                "swarm_id": runtime.swarm_id,
                "item_count": vp.items.len(),
                "version": vp.version,
                "member_count": runtime.members.len(),
                "coordinator": runtime.coordinator_session_id,
                "plan_definition": vp.plan_definition(),
                "execution_state": vp.execution_state(),
                "participants": &vp.participants,
                "items": &vp.items,
                "task_progress": &vp.task_progress,
                "ready_ids": summary.ready_ids,
                "blocked_ids": summary.blocked_ids,
                "active_ids": summary.active_ids,
                "completed_ids": summary.completed_ids,
                "next_ready_ids": next_ready_ids,
                "cycle_ids": summary.cycle_ids,
                "unresolved_dependency_ids": summary.unresolved_dependency_ids,
            }));
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("swarm:plan:") {
        let swarm_id = cmd.strip_prefix("swarm:plan:").unwrap_or("").trim();
        let runtime = swarm_state.load_runtime(swarm_id).await;
        let output = if let Some(vp) = runtime.plan.as_ref() {
            let summary = summarize_plan_graph(&vp.items);
            let next_ready_ids = next_runnable_item_ids(&vp.items, Some(8));
            serde_json::json!({
                "swarm_id": runtime.swarm_id,
                "version": vp.version,
                "member_count": runtime.members.len(),
                "coordinator": runtime.coordinator_session_id,
                "plan_definition": vp.plan_definition(),
                "execution_state": vp.execution_state(),
                "participants": &vp.participants,
                "items": &vp.items,
                "task_progress": &vp.task_progress,
                "ready_ids": summary.ready_ids,
                "blocked_ids": summary.blocked_ids,
                "active_ids": summary.active_ids,
                "completed_ids": summary.completed_ids,
                "next_ready_ids": next_ready_ids,
                "cycle_ids": summary.cycle_ids,
                "unresolved_dependency_ids": summary.unresolved_dependency_ids,
            })
            .to_string()
        } else {
            "[]".to_string()
        };
        return Ok(Some(output));
    }

    if cmd == "swarm:context" {
        let ctx = shared_context.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (swarm_id, entries) in ctx.iter() {
            for (key, context) in entries.iter() {
                out.push(serde_json::json!({
                    "swarm_id": swarm_id,
                    "key": key,
                    "value": context.value,
                    "from_session": context.from_session,
                    "from_name": context.from_name,
                    "created_secs_ago": context.created_at.elapsed().as_secs(),
                    "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                }));
            }
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("swarm:context:") {
        let arg = cmd.strip_prefix("swarm:context:").unwrap_or("").trim();
        let ctx = shared_context.read().await;
        let output = if let Some((swarm_id, key)) = arg.split_once(':') {
            if let Some(entries) = ctx.get(swarm_id) {
                if let Some(context) = entries.get(key) {
                    serde_json::json!({
                        "swarm_id": swarm_id,
                        "key": key,
                        "value": context.value,
                        "from_session": context.from_session,
                        "from_name": context.from_name,
                        "created_secs_ago": context.created_at.elapsed().as_secs(),
                        "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                    })
                    .to_string()
                } else {
                    return Err(anyhow::anyhow!(
                        "No context key '{}' in swarm '{}'",
                        key,
                        swarm_id
                    ));
                }
            } else {
                return Err(anyhow::anyhow!("No context for swarm '{}'", swarm_id));
            }
        } else if let Some(entries) = ctx.get(arg) {
            let mut out: Vec<serde_json::Value> = Vec::new();
            for (key, context) in entries.iter() {
                out.push(serde_json::json!({
                    "key": key,
                    "value": context.value,
                    "from_session": context.from_session,
                    "from_name": context.from_name,
                    "created_secs_ago": context.created_at.elapsed().as_secs(),
                    "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                }));
            }
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string())
        } else {
            "[]".to_string()
        };
        return Ok(Some(output));
    }

    if cmd == "swarm:touches" {
        let touches = file_touches.read().await;
        let members = swarm_members.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (path, accesses) in touches.iter() {
            for access in accesses.iter() {
                let name = members
                    .get(&access.session_id)
                    .and_then(|m| m.friendly_name.clone());
                let timestamp_unix = access
                    .absolute_time
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                out.push(serde_json::json!({
                    "path": path.to_string_lossy(),
                    "session_id": access.session_id,
                    "session_name": name,
                    "op": access.op.as_str(),
                    "summary": access.summary,
                    "age_secs": access.timestamp.elapsed().as_secs(),
                    "timestamp_unix": timestamp_unix,
                }));
            }
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("swarm:touches:") {
        let arg = cmd.strip_prefix("swarm:touches:").unwrap_or("").trim();
        let touches = file_touches.read().await;
        let members = swarm_members.read().await;
        let output = if arg.starts_with("swarm:") {
            let swarm_id = arg.strip_prefix("swarm:").unwrap_or("");
            let swarm_sessions: HashSet<String> = members
                .iter()
                .filter(|(_, m)| m.swarm_id.as_deref() == Some(swarm_id))
                .map(|(id, _)| id.clone())
                .collect();

            let mut out: Vec<serde_json::Value> = Vec::new();
            for (path, accesses) in touches.iter() {
                for access in accesses.iter() {
                    if swarm_sessions.contains(&access.session_id) {
                        let name = members
                            .get(&access.session_id)
                            .and_then(|m| m.friendly_name.clone());
                        let timestamp_unix = access
                            .absolute_time
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        out.push(serde_json::json!({
                            "path": path.to_string_lossy(),
                            "session_id": access.session_id,
                            "session_name": name,
                            "op": access.op.as_str(),
                            "summary": access.summary,
                            "age_secs": access.timestamp.elapsed().as_secs(),
                            "timestamp_unix": timestamp_unix,
                        }));
                    }
                }
            }
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string())
        } else {
            let path = PathBuf::from(arg);
            if let Some(accesses) = touches.get(&path) {
                let mut out: Vec<serde_json::Value> = Vec::new();
                for access in accesses.iter() {
                    let name = members
                        .get(&access.session_id)
                        .and_then(|m| m.friendly_name.clone());
                    let timestamp_unix = access
                        .absolute_time
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    out.push(serde_json::json!({
                        "session_id": access.session_id,
                        "session_name": name,
                        "op": access.op.as_str(),
                        "summary": access.summary,
                        "age_secs": access.timestamp.elapsed().as_secs(),
                        "timestamp_unix": timestamp_unix,
                    }));
                }
                serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string())
            } else {
                "[]".to_string()
            }
        };
        return Ok(Some(output));
    }

    if cmd == "swarm:conflicts" {
        let touches = file_touches.read().await;
        let members = swarm_members.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (path, accesses) in touches.iter() {
            let unique_sessions: HashSet<_> = accesses.iter().map(|a| &a.session_id).collect();
            if unique_sessions.len() > 1 {
                let access_history: Vec<_> = accesses
                    .iter()
                    .map(|access| {
                        let name = members
                            .get(&access.session_id)
                            .and_then(|m| m.friendly_name.clone());
                        let timestamp_unix = access
                            .absolute_time
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        serde_json::json!({
                            "session_id": access.session_id,
                            "session_name": name,
                            "op": access.op.as_str(),
                            "summary": access.summary,
                            "age_secs": access.timestamp.elapsed().as_secs(),
                            "timestamp_unix": timestamp_unix,
                        })
                    })
                    .collect();
                out.push(serde_json::json!({
                    "path": path.to_string_lossy(),
                    "session_count": unique_sessions.len(),
                    "accesses": access_history,
                }));
            }
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd == "swarm:proposals" {
        let ctx = shared_context.read().await;
        let members = swarm_members.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (swarm_id, swarm_ctx) in ctx.iter() {
            for (key, context) in swarm_ctx.iter() {
                if key.starts_with("plan_proposal:") {
                    let proposer_id = key.strip_prefix("plan_proposal:").unwrap_or("");
                    let proposer_name = members
                        .get(proposer_id)
                        .and_then(|m| m.friendly_name.clone());
                    let item_count = serde_json::from_str::<Vec<serde_json::Value>>(&context.value)
                        .map(|v| v.len())
                        .unwrap_or(0);
                    out.push(serde_json::json!({
                        "swarm_id": swarm_id,
                        "proposer_session": proposer_id,
                        "proposer_name": proposer_name,
                        "item_count": item_count,
                        "age_secs": context.created_at.elapsed().as_secs(),
                        "status": "pending",
                    }));
                }
            }
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("swarm:proposals:") {
        let arg = cmd.strip_prefix("swarm:proposals:").unwrap_or("").trim();
        let ctx = shared_context.read().await;
        let members = swarm_members.read().await;
        let output = if arg.starts_with("session_") {
            let proposal_key = format!("plan_proposal:{}", arg);
            let mut found_proposal: Option<String> = None;
            for (swarm_id, swarm_ctx) in ctx.iter() {
                if let Some(context) = swarm_ctx.get(&proposal_key) {
                    let proposer_name = members.get(arg).and_then(|m| m.friendly_name.clone());
                    let items: Vec<serde_json::Value> =
                        serde_json::from_str(&context.value).unwrap_or_default();
                    found_proposal = Some(
                        serde_json::json!({
                            "swarm_id": swarm_id,
                            "proposer_session": arg,
                            "proposer_name": proposer_name,
                            "status": "pending",
                            "age_secs": context.created_at.elapsed().as_secs(),
                            "items": items,
                        })
                        .to_string(),
                    );
                    break;
                }
            }
            found_proposal
                .ok_or_else(|| anyhow::anyhow!("No proposal found from session '{}'", arg))?
        } else {
            let mut out: Vec<serde_json::Value> = Vec::new();
            if let Some(swarm_ctx) = ctx.get(arg) {
                for (key, context) in swarm_ctx.iter() {
                    if key.starts_with("plan_proposal:") {
                        let proposer_id = key.strip_prefix("plan_proposal:").unwrap_or("");
                        let proposer_name = members
                            .get(proposer_id)
                            .and_then(|m| m.friendly_name.clone());
                        let items: Vec<serde_json::Value> =
                            serde_json::from_str(&context.value).unwrap_or_default();
                        out.push(serde_json::json!({
                            "proposer_session": proposer_id,
                            "proposer_name": proposer_name,
                            "status": "pending",
                            "age_secs": context.created_at.elapsed().as_secs(),
                            "items": items,
                        }));
                    }
                }
            }
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string())
        };
        return Ok(Some(output));
    }

    if cmd.starts_with("swarm:info:") {
        let swarm_id = cmd.strip_prefix("swarm:info:").unwrap_or("").trim();
        let swarms = swarms_by_id.read().await;
        let coordinators = swarm_coordinators.read().await;
        let members = swarm_members.read().await;
        let plans = swarm_plans.read().await;
        let ctx = shared_context.read().await;
        let touches = file_touches.read().await;

        let output = if let Some(session_ids) = swarms.get(swarm_id) {
            let coordinator = coordinators.get(swarm_id);
            let coordinator_name =
                coordinator.and_then(|cid| members.get(cid).and_then(|m| m.friendly_name.clone()));

            let member_details: Vec<_> = session_ids
                .iter()
                .filter_map(|sid| {
                    members.get(sid).map(|m| {
                        serde_json::json!({
                            "session_id": m.session_id,
                            "friendly_name": m.friendly_name,
                            "status": m.status,
                            "detail": m.detail,
                            "working_dir": m.working_dir,
                        })
                    })
                })
                .collect();

            let plan = plans
                .get(swarm_id)
                .map(|vp| {
                    serde_json::json!({
                        "items": &vp.items,
                        "task_progress": &vp.task_progress,
                        "version": vp.version,
                    })
                })
                .unwrap_or_else(|| {
                    serde_json::json!({
                        "items": [],
                        "task_progress": {},
                        "version": 0,
                    })
                });

            let context_keys: Vec<_> = ctx
                .get(swarm_id)
                .map(|entries| entries.keys().cloned().collect())
                .unwrap_or_default();

            let conflicts: Vec<_> = touches
                .iter()
                .filter_map(|(path, accesses)| {
                    let swarm_accesses: Vec<_> = accesses
                        .iter()
                        .filter(|a| session_ids.contains(&a.session_id))
                        .collect();
                    let unique: HashSet<_> = swarm_accesses.iter().map(|a| &a.session_id).collect();
                    if unique.len() > 1 {
                        Some(path.to_string_lossy().to_string())
                    } else {
                        None
                    }
                })
                .collect();

            serde_json::json!({
                "swarm_id": swarm_id,
                "member_count": session_ids.len(),
                "members": member_details,
                "coordinator": coordinator,
                "coordinator_name": coordinator_name,
                "plan": plan,
                "context_keys": context_keys,
                "conflict_files": conflicts,
            })
            .to_string()
        } else {
            return Err(anyhow::anyhow!("No swarm with id '{}'", swarm_id));
        };
        return Ok(Some(output));
    }

    if cmd.starts_with("swarm:session:") {
        let target_session = cmd.strip_prefix("swarm:session:").unwrap_or("").trim();
        if target_session.is_empty() {
            return Err(anyhow::anyhow!("swarm:session requires a session_id"));
        }
        let sessions_guard = sessions.read().await;
        let members = swarm_members.read().await;

        let output = if let Some(agent_arc) = sessions_guard.get(target_session) {
            let member_info = members.get(target_session);
            let agent_state = if let Ok(agent) = agent_arc.try_lock() {
                Some(serde_json::json!({
                    "provider": agent.provider_name(),
                    "model": agent.provider_model(),
                    "message_count": agent.message_count(),
                    "pending_alert_count": agent.pending_alert_count(),
                    "pending_alerts": agent.pending_alerts_preview(),
                    "soft_interrupt_count": agent.soft_interrupt_count(),
                    "soft_interrupts": agent.soft_interrupts_preview(),
                    "has_urgent_interrupt": agent.has_urgent_interrupt(),
                    "last_usage": agent.last_usage(),
                }))
            } else {
                None
            };

            let is_processing = member_info
                .map(|m| m.status == "running")
                .unwrap_or(agent_state.is_none());

            serde_json::json!({
                "session_id": target_session,
                "friendly_name": member_info.and_then(|m| m.friendly_name.clone()),
                "swarm_id": member_info.and_then(|m| m.swarm_id.clone()),
                "status": member_info.map(|m| m.status.clone()),
                "detail": member_info.and_then(|m| m.detail.clone()),
                "joined_secs_ago": member_info.map(|m| m.joined_at.elapsed().as_secs()),
                "status_changed_secs_ago": member_info.map(|m| m.last_status_change.elapsed().as_secs()),
                "is_processing": is_processing,
                "agent_state": agent_state,
            })
            .to_string()
        } else {
            return Err(anyhow::anyhow!("Unknown session '{}'", target_session));
        };
        return Ok(Some(output));
    }

    if cmd == "swarm:interrupts" {
        let sessions_guard = sessions.read().await;
        let members = swarm_members.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();

        for (session_id, agent_arc) in sessions_guard.iter() {
            if let Ok(agent) = agent_arc.try_lock() {
                let alert_count = agent.pending_alert_count();
                let interrupt_count = agent.soft_interrupt_count();

                if alert_count > 0 || interrupt_count > 0 {
                    let name = members
                        .get(session_id)
                        .and_then(|m| m.friendly_name.clone());
                    out.push(serde_json::json!({
                        "session_id": session_id,
                        "session_name": name,
                        "pending_alert_count": alert_count,
                        "pending_alerts": agent.pending_alerts_preview(),
                        "soft_interrupt_count": interrupt_count,
                        "soft_interrupts": agent.soft_interrupts_preview(),
                        "has_urgent": agent.has_urgent_interrupt(),
                    }));
                }
            }
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd.starts_with("swarm:id:") {
        let path_str = cmd.strip_prefix("swarm:id:").unwrap_or("").trim();
        if path_str.is_empty() {
            return Err(anyhow::anyhow!("swarm:id requires a path"));
        }
        let path = PathBuf::from(path_str);
        let env_override = std::env::var("JCODE_SWARM_ID")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let git_common = git_common_dir_for(&path);
        let swarm_id = swarm_id_for_dir(Some(path.clone()));
        let is_git_repo = git_common.is_some();
        return Ok(Some(
            serde_json::json!({
                "path": path_str,
                "swarm_id": swarm_id,
                "source": if env_override.is_some() { "env:JCODE_SWARM_ID" }
                          else if is_git_repo { "git_common_dir" }
                          else { "none" },
                "env_override": env_override,
                "git_common_dir": git_common.clone(),
                "git_root": git_common,
                "is_git_repo": is_git_repo,
            })
            .to_string(),
        ));
    }

    Ok(None)
}
