use super::{
    ClientConnectionInfo, ClientDebugState, DebugJob, FileAccess, FileTouchService, ServerIdentity,
    SessionInterruptQueues, SharedContext, SwarmEvent, SwarmMember, VersionedPlan,
};
use crate::agent::Agent;
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

const MEMORY_INCIDENT_WINDOW_MS: u128 = 15 * 60 * 1_000;
const MEMORY_WARNING_PSS_BYTES: u64 = 1024 * 1024 * 1024;
const MEMORY_CRITICAL_PSS_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MEMORY_WARNING_GROWTH_BYTES: u64 = 256 * 1024 * 1024;
const MEMORY_CRITICAL_GROWTH_BYTES: u64 = 1024 * 1024 * 1024;
const MEMORY_WARNING_LIVE_SESSIONS: usize = 128;
const MEMORY_CRITICAL_LIVE_SESSIONS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemoryIncidentDecision {
    severity: &'static str,
    primary_cause: &'static str,
    confidence: &'static str,
}

#[derive(Debug, Default, Serialize)]
struct LiveSwarmPopulation {
    swarm_id: String,
    live_sessions: usize,
    headless_live_sessions: usize,
    status_counts: HashMap<String, usize>,
}

#[derive(Debug, Clone, Copy)]
struct MemoryIncidentMetrics {
    pss_bytes: u64,
    pss_growth_bytes: u64,
    allocator_live_bytes: u64,
    allocator_retained_resident_bytes: u64,
    live_sessions: usize,
    headless_live_sessions: usize,
    connected_clients: usize,
}

fn count_live_spawned_swarm_agents<'a>(
    live_session_ids: &HashSet<String>,
    spawned_session_ids: impl IntoIterator<Item = &'a String>,
) -> usize {
    spawned_session_ids
        .into_iter()
        .filter(|session_id| live_session_ids.contains(*session_id))
        .count()
}

async fn connected_session_snapshot(
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> (
    Vec<(String, Arc<Mutex<Agent>>)>,
    HashMap<String, SwarmMember>,
) {
    // Never hold more than one shared-state lock at a time. Resume transitions
    // use client_connections -> sessions atomically, so a debug snapshot that
    // retained sessions while awaiting client_connections could deadlock them.
    let connected_sessions: HashSet<String> = {
        let connections = client_connections.read().await;
        connections.values().map(|c| c.session_id.clone()).collect()
    };
    let connected_agents = {
        let sessions_guard = sessions.read().await;
        sessions_guard
            .iter()
            .filter(|(session_id, _)| connected_sessions.contains(*session_id))
            .map(|(session_id, agent)| (session_id.clone(), Arc::clone(agent)))
            .collect()
    };
    let members = swarm_members.read().await.clone();
    (connected_agents, members)
}

#[expect(
    clippy::too_many_arguments,
    reason = "server-state debug command inspects many shared server structures in one snapshot"
)]
pub(super) async fn maybe_handle_server_state_command(
    cmd: &str,
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    server_identity: &ServerIdentity,
    server_start_time: Instant,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    debug_jobs: &Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, jcode_agent_runtime::InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
) -> Result<Option<String>> {
    if cmd == "sessions" {
        let (connected_agents, members) =
            connected_session_snapshot(sessions, client_connections, swarm_members).await;
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (sid, agent_arc) in &connected_agents {
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

    if cmd == "memory-incident" || cmd == "server:memory-incident" {
        let payload = build_server_memory_incident_payload(
            sessions,
            client_connections,
            swarm_members,
            server_identity,
            server_start_time,
        )
        .await;
        return Ok(Some(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()),
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
            file_touch,
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

    if cmd == "memory-judge" || cmd == "memory:judge" || cmd == "server:memory-judge" {
        // Attribution of no-LLM memory-mode conversions: how often a surfacing
        // turn ran the LLM judge vs converted (intended opt-out/cadence vs the
        // degradations we drive to zero). See `memory_judge_metrics`.
        return Ok(Some(
            serde_json::to_string_pretty(&jcode_base::memory_judge_metrics::snapshot())
                .unwrap_or_else(|_| "{}".to_string()),
        ));
    }

    if cmd == "embeddings" || cmd == "embeddings:stats" {
        return Ok(Some(
            serde_json::to_string_pretty(&crate::embedding::stats())
                .unwrap_or_else(|_| "{}".to_string()),
        ));
    }

    if cmd == "embeddings:load" {
        let result = crate::embedding::get_embedder();
        let payload = match result {
            Ok(_) => serde_json::json!({
                "status": "loaded",
                "embeddings": crate::embedding::stats(),
            }),
            Err(err) => serde_json::json!({
                "status": "error",
                "error": err.to_string(),
                "embeddings": crate::embedding::stats(),
            }),
        };
        return Ok(Some(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()),
        ));
    }

    if cmd == "embeddings:unload" {
        let unloaded = crate::embedding::unload_now();
        let payload = serde_json::json!({
            "status": if unloaded { "unloaded" } else { "noop" },
            "embeddings": crate::embedding::stats(),
        });
        return Ok(Some(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()),
        ));
    }

    if cmd == "info" || cmd == "server:info" {
        let uptime_secs = server_start_time.elapsed().as_secs();
        let live_session_ids: HashSet<String> = sessions.read().await.keys().cloned().collect();
        let session_count = live_session_ids.len();
        let spawned_swarm_agent_count = {
            let members = swarm_members.read().await;
            count_live_spawned_swarm_agents(
                &live_session_ids,
                members
                    .values()
                    .filter(|member| member.report_back_to_session_id.is_some())
                    .map(|member| &member.session_id),
            )
        };
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
                "spawned_swarm_agent_count": spawned_swarm_agent_count,
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

fn classify_memory_incident(metrics: MemoryIncidentMetrics) -> MemoryIncidentDecision {
    let detached_or_headless = metrics
        .live_sessions
        .saturating_sub(metrics.connected_clients);
    let session_population_dominates = metrics.live_sessions >= MEMORY_WARNING_LIVE_SESSIONS
        && (metrics.headless_live_sessions >= MEMORY_WARNING_LIVE_SESSIONS / 2
            || detached_or_headless >= MEMORY_WARNING_LIVE_SESSIONS);
    let retained_share_is_high = metrics.allocator_retained_resident_bytes >= 256 * 1024 * 1024
        && metrics.allocator_retained_resident_bytes.saturating_mul(4) >= metrics.pss_bytes;

    let (primary_cause, confidence) = if session_population_dominates {
        ("runaway_live_session_population", "high")
    } else if retained_share_is_high {
        ("allocator_retention", "high")
    } else if metrics.allocator_live_bytes >= MEMORY_WARNING_PSS_BYTES {
        ("unattributed_live_heap", "medium")
    } else if metrics.pss_bytes >= MEMORY_WARNING_PSS_BYTES {
        ("non_heap_or_mapping_growth", "medium")
    } else {
        ("within_normal_operating_range", "high")
    };

    let critical = metrics.pss_bytes >= MEMORY_CRITICAL_PSS_BYTES
        || metrics.pss_growth_bytes >= MEMORY_CRITICAL_GROWTH_BYTES
        || metrics.live_sessions >= MEMORY_CRITICAL_LIVE_SESSIONS;
    let warning = metrics.pss_bytes >= MEMORY_WARNING_PSS_BYTES
        || metrics.pss_growth_bytes >= MEMORY_WARNING_GROWTH_BYTES
        || metrics.live_sessions >= MEMORY_WARNING_LIVE_SESSIONS;
    let severity = if critical {
        "critical"
    } else if warning {
        "warning"
    } else {
        "healthy"
    };

    MemoryIncidentDecision {
        severity,
        primary_cause,
        confidence,
    }
}

async fn build_server_memory_incident_payload(
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    server_identity: &ServerIdentity,
    server_start_time: Instant,
) -> serde_json::Value {
    // This command intentionally avoids locking any Agent. It must remain usable
    // when thousands of sessions are resident and the full server:memory walk is
    // slow or contended.
    let process = crate::process_memory::snapshot_with_source("server:memory-incident");
    let diagnostics = crate::runtime_memory_log::build_process_diagnostics(&process);
    let history = crate::process_memory::history(512);
    let latest_timestamp_ms = history
        .last()
        .map(|entry| entry.timestamp_ms)
        .unwrap_or_default();
    let window_start_ms = latest_timestamp_ms.saturating_sub(MEMORY_INCIDENT_WINDOW_MS);
    let baseline = history
        .iter()
        .find(|entry| entry.timestamp_ms >= window_start_ms)
        .or_else(|| history.first());
    let current_pss_bytes = process
        .os
        .as_ref()
        .and_then(|os| os.pss_bytes)
        .or(process.rss_bytes)
        .unwrap_or(0);
    let baseline_pss_bytes = baseline
        .and_then(|entry| {
            entry
                .snapshot
                .os
                .as_ref()
                .and_then(|os| os.pss_bytes)
                .or(entry.snapshot.rss_bytes)
        })
        .unwrap_or(current_pss_bytes);
    let pss_growth_bytes = current_pss_bytes.saturating_sub(baseline_pss_bytes);
    let allocator_live_bytes = process
        .allocator
        .stats
        .as_ref()
        .and_then(|stats| stats.allocated_bytes)
        .unwrap_or(0);
    let allocator_retained_resident_bytes = diagnostics
        .allocator_retained_resident_estimate_bytes
        .unwrap_or(0);

    let live_session_ids: HashSet<String> = sessions.read().await.keys().cloned().collect();
    let connected_clients = client_connections.read().await.len();
    let members = swarm_members.read().await;
    let total_member_count = members.len();
    let total_headless_members = members.values().filter(|member| member.is_headless).count();
    let total_status_counts =
        summarize_status_counts(members.values().map(|member| member.status.as_str()));
    let mut live_status_counts = HashMap::new();
    let mut headless_live_sessions = 0usize;
    let mut untracked_live_sessions = 0usize;
    let mut populations: HashMap<String, LiveSwarmPopulation> = HashMap::new();

    for session_id in &live_session_ids {
        let Some(member) = members.get(session_id) else {
            untracked_live_sessions += 1;
            continue;
        };
        *live_status_counts.entry(member.status.clone()).or_insert(0) += 1;
        if member.is_headless {
            headless_live_sessions += 1;
        }
        let swarm_id = member
            .swarm_id
            .clone()
            .unwrap_or_else(|| "<no-swarm>".to_string());
        let population =
            populations
                .entry(swarm_id.clone())
                .or_insert_with(|| LiveSwarmPopulation {
                    swarm_id,
                    ..LiveSwarmPopulation::default()
                });
        population.live_sessions += 1;
        if member.is_headless {
            population.headless_live_sessions += 1;
        }
        *population
            .status_counts
            .entry(member.status.clone())
            .or_insert(0) += 1;
    }
    drop(members);

    let mut top_live_swarms: Vec<LiveSwarmPopulation> = populations.into_values().collect();
    top_live_swarms.sort_by(|left, right| {
        right
            .live_sessions
            .cmp(&left.live_sessions)
            .then_with(|| left.swarm_id.cmp(&right.swarm_id))
    });
    top_live_swarms.truncate(12);

    let metrics = MemoryIncidentMetrics {
        pss_bytes: current_pss_bytes,
        pss_growth_bytes,
        allocator_live_bytes,
        allocator_retained_resident_bytes,
        live_sessions: live_session_ids.len(),
        headless_live_sessions,
        connected_clients,
    };
    let decision = classify_memory_incident(metrics);
    let detached_live_sessions = live_session_ids.len().saturating_sub(connected_clients);
    let allocated_per_live_session_bytes = (!live_session_ids.is_empty())
        .then(|| allocator_live_bytes / live_session_ids.len() as u64);

    let (summary, actions) = match decision.primary_cause {
        "runaway_live_session_population" => (
            format!(
                "{} live Agent runtimes are resident for {} attached clients; live session population is the dominant operational cause.",
                live_session_ids.len(), connected_clients
            ),
            vec![
                serde_json::json!({
                    "priority": 1,
                    "action": "Pause or cap the producer that is creating headless sessions before doing allocator work.",
                    "why": "The heap is live, so allocator purge cannot release the Agent runtimes.",
                    "commands": ["jcode debug 'swarm:list'", "jcode debug 'server:memory-incident'"]
                }),
                serde_json::json!({
                    "priority": 2,
                    "action": "Inspect top_live_swarms and stop or clean up workers that the owning coordinator no longer needs.",
                    "why": "Target the largest live swarm first; do not destroy active work blindly.",
                    "commands": ["Use `swarm list` and `swarm cleanup` from the owning coordinator", "Use `destroy_session:<id>` only for a confirmed disposable headless session"]
                }),
                serde_json::json!({
                    "priority": 3,
                    "action": "Re-run this report after cleanup and verify both live_sessions and allocator_live_bytes fall.",
                    "why": "A falling live heap confirms session retention was causal.",
                    "commands": ["jcode debug 'server:memory-incident'", "python scripts/analyze_runtime_memory_log.py --days 1"]
                }),
                serde_json::json!({
                    "priority": 4,
                    "action": "Only purge the allocator if retained_resident_bytes remains high after live sessions are reduced.",
                    "why": "Purge is a second-stage response for freed-but-held pages, not live Agents.",
                    "commands": ["jcode debug 'allocator:purge'"]
                }),
            ],
        ),
        "allocator_retention" => (
            "Freed-but-held allocator pages are a material share of resident memory.".to_string(),
            vec![
                serde_json::json!({
                    "priority": 1,
                    "action": "Capture this report, purge the allocator, and compare PSS immediately.",
                    "why": "A large drop proves allocator retention rather than live application state.",
                    "commands": ["jcode debug 'allocator:purge'", "jcode debug 'server:memory-incident'"]
                }),
                serde_json::json!({
                    "priority": 2,
                    "action": "If retention repeatedly regrows, inspect allocation churn and allocator decay settings.",
                    "commands": ["jcode debug 'allocator'", "jcode debug 'allocator:decay:1000'"]
                }),
            ],
        ),
        "unattributed_live_heap" => (
            "Allocator live bytes are high without a large session-population or retention signal.".to_string(),
            vec![
                serde_json::json!({
                    "priority": 1,
                    "action": "Capture full attribution and identify which tracked subsystem is missing from the live heap.",
                    "commands": ["jcode debug 'server:memory'", "python scripts/analyze_runtime_memory_log.py --days 1"]
                }),
                serde_json::json!({
                    "priority": 2,
                    "action": "Escalate to a jemalloc-prof build and heap dump if attribution remains below 50% of live heap.",
                    "commands": ["jcode debug 'allocator:profile:on'", "jcode debug 'allocator:profile:dump /tmp/jcode-server.heap'"]
                }),
            ],
        ),
        "non_heap_or_mapping_growth" => (
            "PSS is high but allocator live bytes do not explain it; inspect mappings, thread stacks, and shared memory.".to_string(),
            vec![serde_json::json!({
                "priority": 1,
                "action": "Inspect OS mappings and thread growth before changing application retention.",
                "commands": ["cat /proc/<server-pid>/smaps_rollup", "pmap -x <server-pid> | sort -k3 -nr | head"]
            })],
        ),
        _ => (
            "No memory incident threshold is currently exceeded.".to_string(),
            vec![serde_json::json!({
                "priority": 1,
                "action": "Continue normal monitoring; compare this report if memory begins to grow.",
                "commands": ["jcode debug 'server:memory-incident'"]
            })],
        ),
    };

    serde_json::json!({
        "schema_version": 1,
        "server": {
            "id": server_identity.id,
            "name": server_identity.name,
            "version": server_identity.version,
            "git_hash": server_identity.git_hash,
            "uptime_secs": server_start_time.elapsed().as_secs(),
        },
        "assessment": {
            "severity": decision.severity,
            "primary_cause": decision.primary_cause,
            "confidence": decision.confidence,
            "summary": summary,
        },
        "process": {
            "rss_bytes": process.rss_bytes,
            "pss_bytes": current_pss_bytes,
            "pss_anon_bytes": process.os.as_ref().and_then(|os| os.pss_anon_bytes),
            "pss_file_bytes": process.os.as_ref().and_then(|os| os.pss_file_bytes),
            "allocator_live_bytes": allocator_live_bytes,
            "allocator_retained_bytes": process.allocator.stats.as_ref().and_then(|stats| stats.retained_bytes),
            "allocator_retained_resident_bytes": allocator_retained_resident_bytes,
            "thread_count": process.thread_count,
        },
        "trend_15m": {
            "baseline_timestamp_ms": baseline.map(|entry| entry.timestamp_ms),
            "baseline_pss_bytes": baseline_pss_bytes,
            "current_timestamp_ms": latest_timestamp_ms,
            "current_pss_bytes": current_pss_bytes,
            "pss_growth_bytes": pss_growth_bytes,
            "sample_count": history.iter().filter(|entry| entry.timestamp_ms >= window_start_ms).count(),
        },
        "population": {
            "live_sessions": live_session_ids.len(),
            "headless_live_sessions": headless_live_sessions,
            "detached_live_sessions": detached_live_sessions,
            "untracked_live_sessions": untracked_live_sessions,
            "connected_clients": connected_clients,
            "allocated_per_live_session_bytes": allocated_per_live_session_bytes,
            "live_status_counts": live_status_counts,
            "top_live_swarms": top_live_swarms,
            "total_swarm_members": total_member_count,
            "total_headless_members": total_headless_members,
            "total_member_status_counts": total_status_counts,
        },
        "thresholds": {
            "warning_pss_bytes": MEMORY_WARNING_PSS_BYTES,
            "critical_pss_bytes": MEMORY_CRITICAL_PSS_BYTES,
            "warning_pss_growth_bytes": MEMORY_WARNING_GROWTH_BYTES,
            "critical_pss_growth_bytes": MEMORY_CRITICAL_GROWTH_BYTES,
            "warning_live_sessions": MEMORY_WARNING_LIVE_SESSIONS,
            "critical_live_sessions": MEMORY_CRITICAL_LIVE_SESSIONS,
        },
        "next_actions": actions,
        "safety": "Preserve active work. Stop the producer first, then clean only sessions confirmed disposable by their owning coordinator.",
        "runbook": "docs/MEMORY_INCIDENT_RUNBOOK.md",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn connected_session_snapshot_releases_connections_before_waiting_for_sessions() {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let client_connections = Arc::new(RwLock::new(HashMap::new()));
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));

        let sessions_gate = sessions.write().await;
        let snapshot = connected_session_snapshot(&sessions, &client_connections, &swarm_members);
        tokio::pin!(snapshot);
        tokio::select! {
            _ = &mut snapshot => panic!("snapshot unexpectedly completed"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }

        let connections_guard =
            tokio::time::timeout(Duration::from_millis(100), client_connections.write())
                .await
                .expect("debug snapshot retained connections while waiting for sessions");
        drop(connections_guard);

        drop(sessions_gate);
        let (connected_agents, members) =
            tokio::time::timeout(Duration::from_secs(1), &mut snapshot)
                .await
                .expect("debug snapshot deadlocked");
        assert!(connected_agents.is_empty());
        assert!(members.is_empty());
    }

    #[test]
    fn spawned_swarm_agent_count_only_includes_live_owned_sessions() {
        let live_session_ids = HashSet::from([
            "root".to_string(),
            "worker-running".to_string(),
            "worker-ready".to_string(),
        ]);
        let spawned_session_ids = [
            "worker-running".to_string(),
            "worker-ready".to_string(),
            "worker-stale".to_string(),
        ];

        assert_eq!(
            count_live_spawned_swarm_agents(&live_session_ids, spawned_session_ids.iter()),
            2
        );
    }

    #[test]
    fn memory_incident_classifies_runaway_live_sessions_before_allocator_retention() {
        let decision = classify_memory_incident(MemoryIncidentMetrics {
            pss_bytes: 4 * 1024 * 1024 * 1024,
            pss_growth_bytes: 3 * 1024 * 1024 * 1024,
            allocator_live_bytes: 3_800 * 1024 * 1024,
            allocator_retained_resident_bytes: 300 * 1024 * 1024,
            live_sessions: 1_145,
            headless_live_sessions: 1_140,
            connected_clients: 5,
        });

        assert_eq!(decision.severity, "critical");
        assert_eq!(decision.primary_cause, "runaway_live_session_population");
        assert_eq!(decision.confidence, "high");
    }

    #[test]
    fn memory_incident_classifies_allocator_retention_when_live_heap_is_small() {
        let decision = classify_memory_incident(MemoryIncidentMetrics {
            pss_bytes: 1_500 * 1024 * 1024,
            pss_growth_bytes: 400 * 1024 * 1024,
            allocator_live_bytes: 500 * 1024 * 1024,
            allocator_retained_resident_bytes: 600 * 1024 * 1024,
            live_sessions: 8,
            headless_live_sessions: 3,
            connected_clients: 5,
        });

        assert_eq!(decision.severity, "warning");
        assert_eq!(decision.primary_cause, "allocator_retention");
        assert_eq!(decision.confidence, "high");
    }

    #[test]
    fn memory_incident_reports_healthy_baseline() {
        let decision = classify_memory_incident(MemoryIncidentMetrics {
            pss_bytes: 220 * 1024 * 1024,
            pss_growth_bytes: 12 * 1024 * 1024,
            allocator_live_bytes: 150 * 1024 * 1024,
            allocator_retained_resident_bytes: 20 * 1024 * 1024,
            live_sessions: 5,
            headless_live_sessions: 1,
            connected_clients: 4,
        });

        assert_eq!(decision.severity, "healthy");
        assert_eq!(decision.primary_cause, "within_normal_operating_range");
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "server memory payload aggregates many live server structures into one debug snapshot"
)]
async fn build_server_memory_payload(
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    server_identity: &ServerIdentity,
    server_start_time: Instant,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    debug_jobs: &Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, jcode_agent_runtime::InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
) -> serde_json::Value {
    let process = crate::process_memory::snapshot_with_source("server:memory");
    let background_tasks = crate::background::global().list().await;
    let embedder_stats = crate::embedding::stats();
    let (search_index_count, search_index_entries, search_index_bytes) =
        crate::tool::session_search_index::cache_memory_stats();
    let embedding_model_available = crate::embedding::is_model_available();

    let sessions_guard = sessions.read().await;
    let mut locked_session_profiles: Vec<serde_json::Value> = Vec::new();
    let mut session_json_bytes = 0u64;
    let mut session_payload_text_bytes = 0u64;
    let mut session_message_count = 0u64;
    let mut session_provider_cache_json_bytes = 0u64;
    let mut session_tool_result_bytes = 0u64;
    let mut session_provider_cache_tool_result_bytes = 0u64;
    let mut session_large_blob_bytes = 0u64;
    let mut session_provider_cache_large_blob_bytes = 0u64;
    let mut locked_session_count = 0usize;
    let mut contended_session_count = 0usize;
    for (session_id, agent_arc) in sessions_guard.iter() {
        if let Ok(agent) = agent_arc.try_lock() {
            locked_session_count += 1;
            let profile = agent.debug_memory_profile();
            let session_profile = profile.get("session").cloned().unwrap_or_default();
            let totals = session_profile.get("totals").cloned().unwrap_or_default();
            let messages = session_profile.get("messages").cloned().unwrap_or_default();
            let provider_cache = session_profile
                .get("provider_messages_cache")
                .cloned()
                .unwrap_or_default();
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
            let provider_cache_json_bytes = totals
                .get("provider_cache_json_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or_else(|| {
                    provider_cache
                        .get("json_bytes")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0)
                });
            let tool_result_bytes = totals
                .get("canonical_tool_result_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or_else(|| {
                    messages
                        .get("memory")
                        .and_then(|value| value.get("tool_result_bytes"))
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0)
                });
            let provider_cache_tool_result_bytes = totals
                .get("provider_cache_tool_result_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or_else(|| {
                    provider_cache
                        .get("memory")
                        .and_then(|value| value.get("tool_result_bytes"))
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0)
                });
            let large_blob_bytes = totals
                .get("canonical_large_blob_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or_else(|| {
                    messages
                        .get("memory")
                        .and_then(|value| value.get("large_block_bytes"))
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0)
                });
            let provider_cache_large_blob_bytes = totals
                .get("provider_cache_large_blob_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or_else(|| {
                    provider_cache
                        .get("memory")
                        .and_then(|value| value.get("large_block_bytes"))
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0)
                });
            session_json_bytes += json_bytes;
            session_payload_text_bytes += payload_bytes;
            session_message_count += message_count;
            session_provider_cache_json_bytes += provider_cache_json_bytes;
            session_tool_result_bytes += tool_result_bytes;
            session_provider_cache_tool_result_bytes += provider_cache_tool_result_bytes;
            session_large_blob_bytes += large_blob_bytes;
            session_provider_cache_large_blob_bytes += provider_cache_large_blob_bytes;
            locked_session_profiles.push(serde_json::json!({
                "session_id": session_id,
                "provider": agent.provider_name(),
                "model": agent.provider_model(),
                "messages": message_count,
                "json_bytes": json_bytes,
                "payload_text_bytes": payload_bytes,
                "provider_cache_json_bytes": provider_cache_json_bytes,
                "tool_result_bytes": tool_result_bytes,
                "provider_cache_tool_result_bytes": provider_cache_tool_result_bytes,
                "large_blob_bytes": large_blob_bytes,
                "provider_cache_large_blob_bytes": provider_cache_large_blob_bytes,
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

    let touches = file_touch.snapshot().await;
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

    let touched_by_session = file_touch.reverse_snapshot().await;
    let touched_session_count = touched_by_session.len();
    let touched_session_estimate_bytes: usize = touched_by_session
        .iter()
        .map(|(session_id, paths)| {
            session_id.len() + paths.iter().map(|path| path_len(path)).sum::<usize>()
        })
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
            "total_provider_cache_json_bytes": session_provider_cache_json_bytes,
            "total_tool_result_bytes": session_tool_result_bytes,
            "total_provider_cache_tool_result_bytes": session_provider_cache_tool_result_bytes,
            "total_large_blob_bytes": session_large_blob_bytes,
            "total_provider_cache_large_blob_bytes": session_provider_cache_large_blob_bytes,
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
        },
        "session_search_index": {
            "index_count": search_index_count,
            "entry_count": search_index_entries,
            "approx_resident_bytes": search_index_bytes,
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
        + member
            .working_dir
            .as_ref()
            .map(|path| path_len(path))
            .unwrap_or(0)
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

fn path_len(path: &std::path::Path) -> usize {
    path.to_string_lossy().len()
}
