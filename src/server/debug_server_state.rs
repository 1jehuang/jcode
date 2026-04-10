use super::{
    ClientConnectionInfo, ClientDebugState, DebugJob, FileAccess, ServerIdentity,
    SessionInterruptQueues, SharedContext, SwarmEvent, SwarmMember, VersionedPlan,
};
use crate::agent::Agent;
use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

#[allow(clippy::too_many_arguments)]
pub(super) async fn maybe_handle_server_state_command(
    cmd: &str,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    server_identity: &ServerIdentity,
    server_start_time: Instant,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
    debug_jobs: &Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, jcode_agent_runtime::InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
) -> Result<Option<String>> {
    if cmd == "sessions" {
        let sessions_guard = sessions.read().await;
        let members = swarm_members.read().await;
        let connections = client_connections.read().await;
        let connected_sessions: HashSet<String> =
            connections.values().map(|c| c.session_id.clone()).collect();
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (sid, agent_arc) in sessions_guard.iter() {
            if !connected_sessions.contains(sid) {
                continue;
            }
            let member_info = members.get(sid);
            let member_status = member_info.map(|m| m.status.as_str());
            let (provider, model, is_processing, working_dir_str, token_usage): (
                Option<String>,
                Option<String>,
                bool,
                Option<String>,
                Option<serde_json::Value>,
            ) = if let Ok(agent) = agent_arc.try_lock() {
                let usage = agent.last_usage();
                (
                    Some(agent.provider_name()),
                    Some(agent.provider_model()),
                    member_status == Some("running"),
                    agent.working_dir().map(|p| p.to_string()),
                    Some(serde_json::json!({
                        "input": usage.input_tokens,
                        "output": usage.output_tokens,
                        "cache_read": usage.cache_read_input_tokens,
                        "cache_write": usage.cache_creation_input_tokens,
                    })),
                )
            } else {
                (None, None, member_status == Some("running"), None, None)
            };
            let final_working_dir: Option<String> = working_dir_str.or_else(|| {
                member_info.and_then(|m| {
                    m.working_dir
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string())
                })
            });
            out.push(serde_json::json!({
                "session_id": sid,
                "friendly_name": member_info.and_then(|m| m.friendly_name.clone()),
                "provider": provider,
                "model": model,
                "is_processing": is_processing,
                "working_dir": final_working_dir,
                "swarm_id": member_info.and_then(|m| m.swarm_id.clone()),
                "status": member_info.map(|m| m.status.clone()),
                "detail": member_info.and_then(|m| m.detail.clone()),
                "token_usage": token_usage,
                "server_name": server_identity.name,
                "server_icon": server_identity.icon,
            }));
        }
        return Ok(Some(
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd == "background" || cmd == "background:tasks" {
        let tasks = crate::background::global().list().await;
        return Ok(Some(
            serde_json::json!({
                "count": tasks.len(),
                "tasks": tasks,
            })
            .to_string(),
        ));
    }

    if cmd == "memory" || cmd == "server:memory" {
        let payload = build_server_memory_payload(
            sessions,
            client_connections,
            swarm_members,
            client_debug_state,
            server_identity,
            server_start_time,
            swarms_by_id,
            shared_context,
            swarm_plans,
            swarm_coordinators,
            file_touches,
            files_touched_by_session,
            channel_subscriptions,
            channel_subscriptions_by_session,
            debug_jobs,
            event_history,
            shutdown_signals,
            soft_interrupt_queues,
        )
        .await;
        return Ok(Some(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()),
        ));
    }

    if cmd == "memory-history" || cmd == "server:memory-history" {
        return Ok(Some(
            serde_json::to_string_pretty(&crate::process_memory::history(256))
                .unwrap_or_else(|_| "[]".to_string()),
        ));
    }

    if cmd == "info" || cmd == "server:info" {
        let uptime_secs = server_start_time.elapsed().as_secs();
        let session_count = sessions.read().await.len();
        let member_count = swarm_members.read().await.len();
        let has_update = super::server_has_newer_binary();
        return Ok(Some(
            serde_json::json!({
                "id": server_identity.id,
                "name": server_identity.name,
                "icon": server_identity.icon,
                "version": server_identity.version,
                "git_hash": server_identity.git_hash,
                "uptime_secs": uptime_secs,
                "session_count": session_count,
                "swarm_member_count": member_count,
                "has_update": has_update,
                "debug_control_enabled": super::debug_control_allowed(),
            })
            .to_string(),
        ));
    }

    if cmd == "clients:map" || cmd == "clients:mapping" {
        let connections = client_connections.read().await;
        let members = swarm_members.read().await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for info in connections.values() {
            let member = members.get(&info.session_id);
            out.push(serde_json::json!({
                "client_id": info.client_id,
                "session_id": info.session_id,
                "friendly_name": member.and_then(|m| m.friendly_name.clone()),
                "working_dir": member.and_then(|m| m.working_dir.clone()),
                "swarm_id": member.and_then(|m| m.swarm_id.clone()),
                "status": member.map(|m| m.status.clone()),
                "detail": member.and_then(|m| m.detail.clone()),
                "connected_secs_ago": info.connected_at.elapsed().as_secs(),
                "last_seen_secs_ago": info.last_seen.elapsed().as_secs(),
            }));
        }
        return Ok(Some(
            serde_json::json!({
                "count": out.len(),
                "clients": out,
            })
            .to_string(),
        ));
    }

    if cmd == "clients" {
        let debug_state = client_debug_state.read().await;
        let client_ids: Vec<&String> = debug_state.clients.keys().collect();
        return Ok(Some(
            serde_json::json!({
                "count": debug_state.clients.len(),
                "active_id": debug_state.active_id,
                "client_ids": client_ids,
            })
            .to_string(),
        ));
    }

    Ok(None)
}

#[allow(clippy::too_many_arguments)]
async fn build_server_memory_payload(
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    server_identity: &ServerIdentity,
    server_start_time: Instant,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
    debug_jobs: &Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, jcode_agent_runtime::InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
) -> serde_json::Value {
    let process = crate::process_memory::snapshot_with_source("server:memory");
    let background_tasks = crate::background::global().list().await;
    let embedder_stats = crate::embedding::stats();
    let embedding_model_available = crate::embedding::is_model_available();

    let sessions_guard = sessions.read().await;
    let mut locked_session_profiles: Vec<serde_json::Value> = Vec::new();
    let mut session_json_bytes = 0u64;
    let mut session_payload_text_bytes = 0u64;
    let mut session_message_count = 0u64;
    let mut locked_session_count = 0usize;
    let mut contended_session_count = 0usize;
    for (session_id, agent_arc) in sessions_guard.iter() {
        if let Ok(agent) = agent_arc.try_lock() {
            locked_session_count += 1;
            let profile = agent.debug_memory_profile();
            let session_profile = profile.get("session").cloned().unwrap_or_default();
            let totals = session_profile.get("totals").cloned().unwrap_or_default();
            let messages = session_profile.get("messages").cloned().unwrap_or_default();
            let json_bytes = totals
                .get("json_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let payload_bytes = totals
                .get("payload_text_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let message_count = messages
                .get("count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            session_json_bytes += json_bytes;
            session_payload_text_bytes += payload_bytes;
            session_message_count += message_count;
            locked_session_profiles.push(serde_json::json!({
                "session_id": session_id,
                "provider": agent.provider_name(),
                "model": agent.provider_model(),
                "messages": message_count,
                "json_bytes": json_bytes,
                "payload_text_bytes": payload_bytes,
                "working_dir": agent.working_dir(),
            }));
        } else {
            contended_session_count += 1;
        }
    }
    drop(sessions_guard);

    locked_session_profiles.sort_by(|left, right| {
        right["json_bytes"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&left["json_bytes"].as_u64().unwrap_or(0))
    });
    let top_sessions: Vec<serde_json::Value> =
        locked_session_profiles.into_iter().take(12).collect();

    let connections = client_connections.read().await;
    let client_connection_estimate_bytes: usize = connections
        .values()
        .map(estimate_client_connection_bytes)
        .sum();
    let connected_client_count = connections.len();
    drop(connections);

    let debug_state = client_debug_state.read().await;
    let debug_clients_count = debug_state.clients.len();
    let debug_client_id_bytes: usize = debug_state.clients.keys().map(|id| id.len()).sum();
    drop(debug_state);

    let members = swarm_members.read().await;
    let swarm_member_estimate_bytes: usize =
        members.values().map(estimate_swarm_member_bytes).sum();
    let swarm_status_counts =
        summarize_status_counts(members.values().map(|member| member.status.as_str()));
    let swarm_member_count = members.len();
    drop(members);

    let swarms = swarms_by_id.read().await;
    let swarm_membership_count: usize = swarms.values().map(|set| set.len()).sum();
    let swarms_estimate_bytes: usize = swarms
        .iter()
        .map(|(swarm_id, members)| {
            swarm_id.len() + members.iter().map(|sid| sid.len()).sum::<usize>()
        })
        .sum();
    let swarm_count = swarms.len();
    drop(swarms);

    let context = shared_context.read().await;
    let shared_context_entry_count: usize = context.values().map(|entries| entries.len()).sum();
    let shared_context_estimate_bytes: usize = context
        .values()
        .flat_map(|entries| entries.values())
        .map(estimate_shared_context_bytes)
        .sum();
    let shared_context_swarm_count = context.len();
    drop(context);

    let plans = swarm_plans.read().await;
    let swarm_plan_count = plans.len();
    let swarm_plan_item_count: usize = plans.values().map(|plan| plan.items.len()).sum();
    let swarm_plan_estimate_bytes: usize = plans
        .iter()
        .map(|(swarm_id, plan)| {
            swarm_id.len()
                + crate::process_memory::estimate_json_bytes(&plan.items)
                + plan.participants.iter().map(|sid| sid.len()).sum::<usize>()
        })
        .sum();
    drop(plans);

    let coordinators = swarm_coordinators.read().await;
    let swarm_coordinator_count = coordinators.len();
    let swarm_coordinator_bytes: usize = coordinators
        .iter()
        .map(|(swarm_id, session_id)| swarm_id.len() + session_id.len())
        .sum();
    drop(coordinators);

    let touches = file_touches.read().await;
    let file_touch_path_count = touches.len();
    let file_touch_entry_count: usize = touches.values().map(|entries| entries.len()).sum();
    let file_touch_estimate_bytes: usize = touches
        .iter()
        .map(|(path, entries)| {
            path_len(path)
                + entries
                    .iter()
                    .map(estimate_file_access_bytes)
                    .sum::<usize>()
        })
        .sum();
    drop(touches);

    let touched_by_session = files_touched_by_session.read().await;
    let touched_session_count = touched_by_session.len();
    let touched_session_estimate_bytes: usize = touched_by_session
        .iter()
        .map(|(session_id, paths)| session_id.len() + paths.iter().map(path_len).sum::<usize>())
        .sum();
    drop(touched_by_session);

    let subscriptions = channel_subscriptions.read().await;
    let subscription_swarm_count = subscriptions.len();
    let subscription_channel_count: usize = subscriptions.values().map(|map| map.len()).sum();
    let subscription_member_count: usize = subscriptions
        .values()
        .flat_map(|channels| channels.values())
        .map(|members| members.len())
        .sum();
    let subscription_estimate_bytes: usize = subscriptions
        .iter()
        .map(|(swarm_id, channels)| {
            swarm_id.len()
                + channels
                    .iter()
                    .map(|(channel, members)| {
                        channel.len() + members.iter().map(|sid| sid.len()).sum::<usize>()
                    })
                    .sum::<usize>()
        })
        .sum();
    drop(subscriptions);

    let subscriptions_by_session = channel_subscriptions_by_session.read().await;
    let subscriptions_by_session_count = subscriptions_by_session.len();
    let subscriptions_by_session_estimate_bytes: usize = subscriptions_by_session
        .iter()
        .map(|(session_id, swarms)| {
            session_id.len()
                + swarms
                    .iter()
                    .map(|(swarm_id, channels)| {
                        swarm_id.len() + channels.iter().map(|channel| channel.len()).sum::<usize>()
                    })
                    .sum::<usize>()
        })
        .sum();
    drop(subscriptions_by_session);

    let jobs = debug_jobs.read().await;
    let debug_job_count = jobs.len();
    let debug_job_estimate_bytes: usize = jobs.values().map(estimate_debug_job_bytes).sum();
    let debug_job_output_bytes: usize = jobs
        .values()
        .map(|job| job.output.as_ref().map(|value| value.len()).unwrap_or(0))
        .sum();
    drop(jobs);

    let events = event_history.read().await;
    let event_history_count = events.len();
    let event_history_estimate_bytes: usize = events.iter().map(estimate_swarm_event_bytes).sum();
    drop(events);

    let shutdown = shutdown_signals.read().await;
    let shutdown_signal_count = shutdown.len();
    let shutdown_signal_bytes: usize = shutdown.keys().map(|sid| sid.len()).sum();
    drop(shutdown);

    let soft_queues = soft_interrupt_queues.read().await;
    let mut soft_interrupt_session_count = soft_queues.len();
    let mut soft_interrupt_count = 0usize;
    let mut soft_interrupt_text_bytes = 0usize;
    for queue in soft_queues.values() {
        if let Ok(queue) = queue.lock() {
            soft_interrupt_count += queue.len();
            soft_interrupt_text_bytes += queue.iter().map(|item| item.content.len()).sum::<usize>();
        }
    }
    if soft_interrupt_session_count == 0 && soft_interrupt_count > 0 {
        soft_interrupt_session_count = 1;
    }
    drop(soft_queues);

    let background_task_count = background_tasks.len();
    let background_task_json_bytes: usize = background_tasks
        .iter()
        .map(crate::process_memory::estimate_json_bytes)
        .sum();

    serde_json::json!({
        "server": {
            "id": server_identity.id,
            "name": server_identity.name,
            "icon": server_identity.icon,
            "version": server_identity.version,
            "git_hash": server_identity.git_hash,
            "uptime_secs": server_start_time.elapsed().as_secs(),
        },
        "process": process,
        "history": crate::process_memory::history(128),
        "clients": {
            "connected_clients": connected_client_count,
            "debug_clients": debug_clients_count,
            "connection_estimate_bytes": client_connection_estimate_bytes,
            "debug_client_id_bytes": debug_client_id_bytes,
        },
        "sessions": {
            "live_count": locked_session_count + contended_session_count,
            "locked_count": locked_session_count,
            "contended_count": contended_session_count,
            "total_message_count": session_message_count,
            "total_json_bytes": session_json_bytes,
            "total_payload_text_bytes": session_payload_text_bytes,
            "top_by_json_bytes": top_sessions,
        },
        "swarm": {
            "member_count": swarm_member_count,
            "status_counts": swarm_status_counts,
            "member_estimate_bytes": swarm_member_estimate_bytes,
            "swarm_count": swarm_count,
            "swarm_membership_count": swarm_membership_count,
            "swarms_estimate_bytes": swarms_estimate_bytes,
            "shared_context_swarm_count": shared_context_swarm_count,
            "shared_context_entry_count": shared_context_entry_count,
            "shared_context_estimate_bytes": shared_context_estimate_bytes,
            "plan_count": swarm_plan_count,
            "plan_item_count": swarm_plan_item_count,
            "plan_estimate_bytes": swarm_plan_estimate_bytes,
            "coordinator_count": swarm_coordinator_count,
            "coordinator_estimate_bytes": swarm_coordinator_bytes,
        },
        "file_tracking": {
            "paths_with_touches": file_touch_path_count,
            "touch_entries": file_touch_entry_count,
            "touch_estimate_bytes": file_touch_estimate_bytes,
            "files_touched_by_session_count": touched_session_count,
            "files_touched_by_session_estimate_bytes": touched_session_estimate_bytes,
        },
        "channels": {
            "subscription_swarms": subscription_swarm_count,
            "subscription_channels": subscription_channel_count,
            "subscription_memberships": subscription_member_count,
            "subscription_estimate_bytes": subscription_estimate_bytes,
            "subscriptions_by_session_count": subscriptions_by_session_count,
            "subscriptions_by_session_estimate_bytes": subscriptions_by_session_estimate_bytes,
        },
        "debug": {
            "job_count": debug_job_count,
            "job_estimate_bytes": debug_job_estimate_bytes,
            "job_output_bytes": debug_job_output_bytes,
            "event_history_count": event_history_count,
            "event_history_estimate_bytes": event_history_estimate_bytes,
            "shutdown_signal_count": shutdown_signal_count,
            "shutdown_signal_bytes": shutdown_signal_bytes,
        },
        "interrupts": {
            "queue_sessions": soft_interrupt_session_count,
            "pending_interrupts": soft_interrupt_count,
            "pending_interrupt_text_bytes": soft_interrupt_text_bytes,
        },
        "background": {
            "task_count": background_task_count,
            "tasks_json_bytes": background_task_json_bytes,
        },
        "embeddings": {
            "model_available": embedding_model_available,
            "loaded": embedder_stats.loaded,
            "load_count": embedder_stats.load_count,
            "unload_count": embedder_stats.unload_count,
            "embed_calls": embedder_stats.embed_calls,
            "embed_failures": embedder_stats.embed_failures,
            "total_embed_ms": embedder_stats.total_embed_ms,
            "avg_embed_ms": embedder_stats.avg_embed_ms,
            "idle_secs": embedder_stats.idle_secs,
            "loaded_secs": embedder_stats.loaded_secs,
            "cache_hits": embedder_stats.cache_hits,
            "cache_size": embedder_stats.cache_size,
        }
    })
}

fn summarize_status_counts<'a>(statuses: impl Iterator<Item = &'a str>) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for status in statuses {
        *counts.entry(status.to_string()).or_insert(0) += 1;
    }
    counts
}

fn estimate_client_connection_bytes(info: &ClientConnectionInfo) -> usize {
    info.client_id.len()
        + info.session_id.len()
        + info
            .client_instance_id
            .as_ref()
            .map(|value| value.len())
            .unwrap_or(0)
        + info
            .debug_client_id
            .as_ref()
            .map(|value| value.len())
            .unwrap_or(0)
}

fn estimate_swarm_member_bytes(member: &SwarmMember) -> usize {
    member.session_id.len()
        + member.status.len()
        + member.detail.as_ref().map(|value| value.len()).unwrap_or(0)
        + member
            .friendly_name
            .as_ref()
            .map(|value| value.len())
            .unwrap_or(0)
        + member.role.len()
        + member.working_dir.as_ref().map(path_len).unwrap_or(0)
        + member
            .swarm_id
            .as_ref()
            .map(|value| value.len())
            .unwrap_or(0)
}

fn estimate_shared_context_bytes(context: &SharedContext) -> usize {
    context.key.len()
        + context.value.len()
        + context.from_session.len()
        + context
            .from_name
            .as_ref()
            .map(|value| value.len())
            .unwrap_or(0)
}

fn estimate_file_access_bytes(access: &FileAccess) -> usize {
    access.session_id.len()
        + format!("{:?}", access.op).len()
        + access
            .summary
            .as_ref()
            .map(|value| value.len())
            .unwrap_or(0)
        + access.detail.as_ref().map(|value| value.len()).unwrap_or(0)
}

fn estimate_debug_job_bytes(job: &DebugJob) -> usize {
    job.id.len()
        + job.command.len()
        + job
            .session_id
            .as_ref()
            .map(|value| value.len())
            .unwrap_or(0)
        + job.output.as_ref().map(|value| value.len()).unwrap_or(0)
        + job.error.as_ref().map(|value| value.len()).unwrap_or(0)
}

fn estimate_swarm_event_bytes(event: &SwarmEvent) -> usize {
    format!("{:?}", event).len()
}

fn path_len(path: &PathBuf) -> usize {
    path.to_string_lossy().len()
}
