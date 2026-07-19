mod await_members_state;
mod background_tasks;
mod client_actions;
mod client_api;
mod client_comm;
mod client_comm_channels;
mod client_comm_context;
mod client_comm_message;
mod client_disconnect_cleanup;
mod client_lifecycle;
mod client_lifecycle_logging;
mod client_lightweight_control;
mod client_session;
mod client_state;
mod client_writer;
mod comm_await;
mod comm_control;
mod comm_graph;
mod comm_plan;
mod comm_session;
mod comm_sync;
mod debug;
mod debug_ambient;
mod debug_command_exec;
mod debug_events;
mod debug_help;
mod debug_jobs;
mod debug_server_state;
mod debug_session_admin;
mod debug_swarm_read;
mod debug_swarm_write;
mod debug_testers;
mod durable_state;
mod headless;
mod jade_relay;
mod lifecycle;
mod live_turn;
mod provider_control;
mod reload;
mod reload_recovery;
mod reload_state;
mod reload_trace;
mod runtime;
mod socket;
mod swarm;
mod swarm_channels;
mod swarm_mutation_state;
mod swarm_persistence;
mod util;

pub(super) use self::await_members_state::AwaitMembersRuntime;
use self::background_tasks::{
    dispatch_background_task_completion, dispatch_background_task_progress,
    dispatch_swarm_await_completion, dispatch_swarm_batch_progress, dispatch_swarm_output_tail,
    dispatch_swarm_runtime_status, dispatch_swarm_todo_progress, dispatch_swarm_tool_activity,
    dispatch_ui_activity,
};
use self::debug::{ClientConnectionInfo, ClientDebugState};
use self::debug_jobs::DebugJob;
use self::headless::create_headless_session;
use self::reload::await_reload_signal;
use self::runtime::ServerRuntime;
use self::swarm::{
    MAX_SWARM_MEMBERS, broadcast_swarm_plan, broadcast_swarm_plan_with_previous,
    broadcast_swarm_status, expired_terminal_member_ids, member_consumes_swarm_capacity,
    record_swarm_event, record_swarm_event_for_session, refresh_swarm_task_staleness,
    remove_plan_participant, remove_session_from_swarm, rename_plan_participant, run_swarm_message,
    send_swarm_plan_to_session, set_member_task_label, swarm_is_self_or_ancestor,
    update_member_status, update_member_status_with_report, update_member_status_with_report_tldr,
};
use self::swarm_channels::{
    remove_session_channel_subscriptions, subscribe_session_to_channel,
    unsubscribe_session_from_channel,
};
pub(super) use self::swarm_mutation_state::SwarmMutationRuntime;
use self::swarm_persistence::{
    LoadedSwarmRuntimeState, capture_swarm_state_version,
    load_runtime_state as load_persisted_swarm_runtime_state,
    persist_swarm_state as persist_swarm_state_snapshot, remove_swarm_state_if_version,
    swarm_operation_lock,
};
use self::util::get_shared_mcp_pool;
use crate::agent::Agent;
use crate::ambient_runner::AmbientRunnerHandle;
use crate::bus::{Bus, BusEvent};
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::runtime_memory_log::{
    RuntimeMemoryLogController, RuntimeMemoryLogSampling, RuntimeMemoryLogTrigger,
    ServerRuntimeMemoryBackground, ServerRuntimeMemoryClients, ServerRuntimeMemoryEmbeddings,
    ServerRuntimeMemorySample, ServerRuntimeMemoryServer, ServerRuntimeMemorySessions,
    ServerRuntimeMemoryTopSession,
};
use crate::tool::selfdev::ReloadContext;
use crate::transport::Listener;
use anyhow::Result;
use jcode_agent_runtime::{InterruptSignal, SoftInterruptSource};
use jcode_swarm_core::{
    append_swarm_completion_report_instructions, format_structured_completion_report,
    summarize_plan_items, truncate_detail,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast, mpsc};

pub(super) type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
pub(super) type ChannelSubscriptions =
    Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

/// Remove a live server session and its process-presence marker as one
/// lifecycle operation. Server-owned sessions all share the long-running
/// server PID, so leaving the marker behind makes presence UIs count the
/// removed session forever.
pub(super) async fn remove_session_entry<T>(
    sessions: &Arc<RwLock<HashMap<String, T>>>,
    session_id: &str,
) -> Option<T> {
    let removed = sessions.write().await.remove(session_id);
    if removed.is_some() {
        crate::storage::unregister_active_pid(session_id);
    }
    removed
}

const SERVER_NAME_ENV: &str = "JCODE_SERVER_NAME";
const SERVER_DISPLAY_NAME_ENV: &str = "JCODE_SERVER_DISPLAY_NAME";
const MAX_CONFIGURED_SERVER_NAME_LEN: usize = 64;
const SWARM_TERMINAL_MEMBER_GC_BATCH_SIZE: usize = 64;

async fn prune_expired_terminal_swarm_members(
    sessions: &SessionAgents,
    swarm_state: &SwarmState,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
) -> usize {
    let retention = swarm::swarm_terminal_member_retention();
    let mut candidates = {
        let members = swarm_state.members.read().await;
        expired_terminal_member_ids(&members, retention)
    };
    candidates.truncate(SWARM_TERMINAL_MEMBER_GC_BATCH_SIZE);
    if candidates.is_empty() {
        return 0;
    }

    let live_sessions: HashSet<String> = sessions.read().await.keys().cloned().collect();
    let mut pruned = 0usize;
    for session_id in candidates {
        // A session can be resumed between candidate collection and removal.
        // Never collect a member that currently has a live agent runtime.
        if live_sessions.contains(&session_id) || sessions.read().await.contains_key(&session_id) {
            continue;
        }
        let removed_swarm_id = {
            let mut members = swarm_state.members.write().await;
            let still_expired = members.get(&session_id).is_some_and(|member| {
                swarm::member_status_is_terminal(&member.status)
                    && member.last_status_change.elapsed() >= retention
            });
            if still_expired {
                members
                    .remove(&session_id)
                    .and_then(|member| member.swarm_id)
            } else {
                None
            }
        };
        let Some(swarm_id) = removed_swarm_id else {
            continue;
        };

        remove_session_from_swarm(
            &session_id,
            &swarm_id,
            &swarm_state.members,
            &swarm_state.swarms_by_id,
            &swarm_state.coordinators,
            &swarm_state.plans,
        )
        .await;
        remove_session_channel_subscriptions(
            &session_id,
            channel_subscriptions,
            channel_subscriptions_by_session,
        )
        .await;
        pruned += 1;
    }

    if pruned > 0 {
        crate::logging::info(&format!(
            "Garbage-collected {pruned} expired terminal swarm member(s)"
        ));
    }
    pruned
}

/// Reap spawned swarm workers that finished their work and have sat idle past
/// the reap window: ask any attached client window to close, shut down the
/// server-side agent, and remove the member from swarm state.
///
/// This is the server-side backstop for coordinator `cleanup`: coordinators
/// that get interrupted, replaced, or never call cleanup used to leave every
/// spawned worker running forever (one ~80-150 MB client process each). Only
/// agent-spawned members (`report_back_to_session_id` set) are eligible;
/// user-created sessions are never touched. See
/// [`swarm::idle_spawned_worker_reap_candidates`] for the exact policy.
async fn reap_idle_spawned_workers(
    sessions: &SessionAgents,
    swarm_state: &SwarmState,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    soft_interrupt_queues: &SessionInterruptQueues,
) -> usize {
    let Some(idle_after) = swarm::swarm_idle_worker_reap_after() else {
        return 0;
    };
    let candidates = {
        let members = swarm_state.members.read().await;
        swarm::idle_spawned_worker_reap_candidates(&members, idle_after)
    };
    if candidates.is_empty() {
        return 0;
    }

    let mut reaped = 0usize;
    for session_id in candidates {
        // Re-validate under the current map: status may have changed between
        // candidate collection and removal (a resumed/reassigned worker).
        let still_reapable = {
            let members = swarm_state.members.read().await;
            members.get(&session_id).is_some_and(|member| {
                member.report_back_to_session_id.is_some()
                    && member.role != "coordinator"
                    && (member.status == "ready"
                        || swarm::member_status_is_terminal(&member.status))
                    && member.last_status_change.elapsed() >= idle_after
            })
        };
        if !still_reapable {
            continue;
        }

        // Ask any attached client (visible spawned window) to close itself.
        let _ = fanout_session_event(
            &swarm_state.members,
            &session_id,
            ServerEvent::SessionCloseRequested {
                reason: format!(
                    "Idle spawned worker reaped after {}s of inactivity",
                    idle_after.as_secs()
                ),
            },
        )
        .await;

        if let Some(agent_arc) = remove_session_entry(sessions, &session_id).await {
            remove_session_interrupt_queue(soft_interrupt_queues, &session_id).await;
            remove_background_tool_signal(&session_id);
            if let Ok(mut agent) = agent_arc.try_lock() {
                agent.mark_closed();
            }
        }

        let removed_swarm_id = {
            let mut members = swarm_state.members.write().await;
            members
                .remove(&session_id)
                .and_then(|member| member.swarm_id)
        };
        if let Some(ref swarm_id) = removed_swarm_id {
            remove_session_from_swarm(
                &session_id,
                swarm_id,
                &swarm_state.members,
                &swarm_state.swarms_by_id,
                &swarm_state.coordinators,
                &swarm_state.plans,
            )
            .await;
        }
        remove_session_channel_subscriptions(
            &session_id,
            channel_subscriptions,
            channel_subscriptions_by_session,
        )
        .await;
        crate::logging::info(&format!(
            "Reaped idle spawned swarm worker {session_id} (idle > {}s)",
            idle_after.as_secs()
        ));
        reaped += 1;
    }
    reaped
}

pub(super) async fn persist_swarm_state_for(swarm_id: &str, swarm_state: &SwarmState) {
    // Never call this while holding any SwarmState map guard. The operation
    // lock deliberately spans the independent map reads and atomic file write.
    let operation_lock = swarm_operation_lock(swarm_id);
    let _operation_guard = operation_lock.lock().await;
    let runtime = swarm_state.load_runtime(swarm_id).await;
    persist_swarm_state_snapshot(
        swarm_id,
        runtime.plan.as_ref(),
        runtime.coordinator_session_id.as_deref(),
        &runtime.members,
    );
}

pub(super) async fn remove_persisted_swarm_state_for(swarm_id: &str, swarm_state: &SwarmState) {
    // Persist and remove share one per-swarm ordering domain. The file version
    // is an extra CAS guard against direct/recovery writers outside this path.
    let operation_lock = swarm_operation_lock(swarm_id);
    let _operation_guard = operation_lock.lock().await;
    let file_version = capture_swarm_state_version(swarm_id);
    let runtime = swarm_state.load_runtime(swarm_id).await;
    if runtime.has_any_state() {
        return;
    }
    let _ = remove_swarm_state_if_version(swarm_id, &file_version);
}

fn headless_member_should_restore(status: &str, is_headless: bool) -> bool {
    is_headless
        && !matches!(
            status,
            "ready" | "completed" | "done" | "failed" | "stopped"
        )
}

fn headless_reload_continuation_message(reload_ctx: Option<ReloadContext>) -> Option<String> {
    ReloadContext::recovery_directive(reload_ctx.as_ref(), true, "", None)
        .map(|directive| directive.continuation_message)
}

fn configured_server_name(cli_name: Option<String>) -> Option<String> {
    cli_name
        .as_deref()
        .and_then(normalize_configured_server_name)
        .or_else(configured_server_name_from_env)
}

fn configured_server_name_from_env() -> Option<String> {
    [SERVER_NAME_ENV, SERVER_DISPLAY_NAME_ENV]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .and_then(|value| normalize_configured_server_name(&value))
        })
}

fn normalize_configured_server_name(raw: &str) -> Option<String> {
    let mut normalized = String::new();
    let mut previous_dash = false;

    for ch in raw.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if ch == '.' || ch == '-' {
            ch
        } else {
            '-'
        };

        if mapped == '-' {
            if previous_dash {
                continue;
            }
            previous_dash = true;
        } else {
            previous_dash = false;
        }
        normalized.push(mapped);
        if normalized.len() >= MAX_CONFIGURED_SERVER_NAME_LEN {
            break;
        }
    }

    let trimmed = normalized.trim_matches(|ch| matches!(ch, '-' | '.'));
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[derive(Default)]
struct HeadlessRecoveryStats {
    candidates: usize,
    resumed: usize,
    skipped: usize,
    failed_to_load: usize,
}

async fn capture_runtime_memory_common_sample(
    identity: &ServerIdentity,
    client_count: &Arc<RwLock<usize>>,
    server_start_time: Instant,
    kind: &str,
    source: &str,
    trigger: RuntimeMemoryLogTrigger,
    sampling: RuntimeMemoryLogSampling,
) -> ServerRuntimeMemorySample {
    let now = chrono::Utc::now();
    let process =
        crate::process_memory::snapshot_with_source(format!("server:runtime-log:{source}"));
    let connected_count = *client_count.read().await;
    let background_task_count = crate::background::global().list().await.len();
    let embedder_stats = crate::embedding::stats();
    let embedding_model_available = crate::embedding::is_model_available();

    ServerRuntimeMemorySample {
        schema_version: 2,
        kind: kind.to_string(),
        timestamp: now.to_rfc3339(),
        timestamp_ms: now.timestamp_millis(),
        source: source.to_string(),
        trigger,
        sampling,
        server: ServerRuntimeMemoryServer {
            id: identity.id.clone(),
            name: identity.name.clone(),
            icon: identity.icon.clone(),
            version: identity.version.clone(),
            git_hash: identity.git_hash.clone(),
            uptime_secs: server_start_time.elapsed().as_secs(),
        },
        process_diagnostics: crate::runtime_memory_log::build_process_diagnostics(&process),
        process,
        clients: ServerRuntimeMemoryClients { connected_count },
        sessions: None,
        background: ServerRuntimeMemoryBackground {
            task_count: background_task_count,
        },
        embeddings: ServerRuntimeMemoryEmbeddings {
            model_available: embedding_model_available,
            stats: embedder_stats,
        },
    }
}

async fn capture_runtime_memory_process_sample(
    identity: &ServerIdentity,
    client_count: &Arc<RwLock<usize>>,
    server_start_time: Instant,
    source: &str,
    trigger: RuntimeMemoryLogTrigger,
    sampling: RuntimeMemoryLogSampling,
) -> ServerRuntimeMemorySample {
    capture_runtime_memory_common_sample(
        identity,
        client_count,
        server_start_time,
        "process",
        source,
        trigger,
        sampling,
    )
    .await
}

async fn capture_runtime_memory_attribution_sample(
    identity: &ServerIdentity,
    sessions: &SessionAgents,
    client_count: &Arc<RwLock<usize>>,
    server_start_time: Instant,
    source: &str,
    trigger: RuntimeMemoryLogTrigger,
    sampling: RuntimeMemoryLogSampling,
) -> ServerRuntimeMemorySample {
    let mut sample = capture_runtime_memory_common_sample(
        identity,
        client_count,
        server_start_time,
        "attribution",
        source,
        trigger,
        sampling,
    )
    .await;

    let sessions_guard = sessions.read().await;
    let live_count = sessions_guard.len();
    let mut sampled_count = 0usize;
    let mut contended_count = 0usize;
    let mut memory_enabled_session_count = 0usize;
    let mut total_message_count = 0u64;
    let mut total_provider_cache_message_count = 0u64;
    let mut total_json_bytes = 0u64;
    let mut total_payload_text_bytes = 0u64;
    let mut total_provider_cache_json_bytes = 0u64;
    let mut total_tool_result_bytes = 0u64;
    let mut total_provider_cache_tool_result_bytes = 0u64;
    let mut total_large_blob_bytes = 0u64;
    let mut total_provider_cache_large_blob_bytes = 0u64;
    let mut top_sessions: Vec<ServerRuntimeMemoryTopSession> = Vec::new();

    for (session_id, agent_arc) in sessions_guard.iter() {
        let Ok(mut agent) = agent_arc.try_lock() else {
            contended_count += 1;
            continue;
        };

        sampled_count += 1;
        let profile = agent.session_memory_profile_snapshot();
        let memory_enabled = agent.memory_enabled();
        if memory_enabled {
            memory_enabled_session_count += 1;
        }

        let message_count = profile.message_count as u64;
        let provider_cache_message_count = profile.provider_cache_message_count as u64;
        let json_bytes = profile.total_json_bytes as u64;
        let payload_text_bytes = profile.payload_text_bytes as u64;
        let provider_cache_json_bytes = profile.provider_cache_json_bytes as u64;
        let tool_result_bytes = profile.canonical_tool_result_bytes as u64;
        let provider_cache_tool_result_bytes = profile.provider_cache_tool_result_bytes as u64;
        let large_blob_bytes = profile.canonical_large_blob_bytes as u64;
        let provider_cache_large_blob_bytes = profile.provider_cache_large_blob_bytes as u64;

        total_message_count += message_count;
        total_provider_cache_message_count += provider_cache_message_count;
        total_json_bytes += json_bytes;
        total_payload_text_bytes += payload_text_bytes;
        total_provider_cache_json_bytes += provider_cache_json_bytes;
        total_tool_result_bytes += tool_result_bytes;
        total_provider_cache_tool_result_bytes += provider_cache_tool_result_bytes;
        total_large_blob_bytes += large_blob_bytes;
        total_provider_cache_large_blob_bytes += provider_cache_large_blob_bytes;

        top_sessions.push(ServerRuntimeMemoryTopSession {
            session_id: session_id.clone(),
            provider: agent.provider_name(),
            model: agent.provider_model(),
            memory_enabled,
            message_count,
            provider_cache_message_count,
            json_bytes,
            payload_text_bytes,
            provider_cache_json_bytes,
            tool_result_bytes,
            provider_cache_tool_result_bytes,
            large_blob_bytes,
            provider_cache_large_blob_bytes,
        });
    }
    drop(sessions_guard);

    top_sessions.sort_by(|left, right| right.json_bytes.cmp(&left.json_bytes));
    top_sessions.truncate(5);

    sample.sessions = Some(ServerRuntimeMemorySessions {
        live_count,
        sampled_count,
        contended_count,
        memory_enabled_session_count,
        total_message_count,
        total_provider_cache_message_count,
        total_json_bytes,
        total_payload_text_bytes,
        total_provider_cache_json_bytes,
        total_tool_result_bytes,
        total_provider_cache_tool_result_bytes,
        total_large_blob_bytes,
        total_provider_cache_large_blob_bytes,
        top_by_json_bytes: top_sessions,
    });
    sample
}

mod state;

use self::state::latest_peer_touches;
pub use self::state::{
    FileAccess, SessionControlHandle, SharedContext, SwarmEvent, SwarmEventType, SwarmMember,
    SwarmState,
};
use self::state::{
    SessionInterruptQueues, fanout_live_client_event, fanout_session_event,
    queue_soft_interrupt_for_session, register_background_tool_signal,
    register_session_event_sender, register_session_interrupt_queue, remove_background_tool_signal,
    remove_session_interrupt_queue, rename_background_tool_signal, rename_session_interrupt_queue,
    session_event_fanout_sender, unregister_session_event_sender,
};
pub use crate::plan::{SwarmTaskProgress, VersionedPlan};

pub use self::await_members_state::pending_await_members_for_session;
use self::reload_state::clear_reload_marker_if_stale_for_pid;
#[cfg(test)]
pub(crate) use self::reload_state::subscribe_reload_signal_for_tests;
pub use self::reload_state::{
    ReloadAck, ReloadPhase, ReloadSignal, ReloadState, ReloadWaitStatus, acknowledge_reload_signal,
    await_reload_handoff, clear_reload_marker, inspect_reload_wait_status,
    publish_reload_socket_ready, recent_reload_state, reload_marker_active, reload_marker_exists,
    reload_marker_path, reload_process_alive, reload_state_summary, send_reload_signal,
    wait_for_reload_ack, wait_for_reload_handoff_event, write_reload_marker, write_reload_state,
};

pub use self::lifecycle::configure_temporary_server;
#[cfg(unix)]
pub use self::socket::spawn_server_notify;
#[cfg(unix)]
use self::socket::{acquire_daemon_lock, mark_close_on_exec};
pub use self::socket::{
    cleanup_socket_pair, connect_socket, debug_socket_path, has_live_listener, is_server_ready,
    reap_stale_socket_if_dead, set_socket_path, socket_path, wait_for_server_ready,
};
use self::socket::{signal_ready_fd, socket_has_live_listener};

pub use self::util::ServerIdentity;
pub(crate) use self::util::server_has_newer_binary;
use self::util::{
    debug_control_allowed, embedding_idle_unload_secs, git_common_dir_for, reload_exec_target,
    startup_headless_recovery_test_delay, swarm_id_for_dir,
};

mod file_activity;
use self::file_activity::file_activity_scope_label;

mod file_touch_service;
pub(crate) use self::file_touch_service::FileTouchService;

#[cfg(test)]
mod socket_tests;

#[cfg(test)]
mod startup_tests;

#[cfg(test)]
mod queue_tests;

#[cfg(test)]
mod file_activity_tests;

/// Idle timeout for the shared server when no clients are connected (5 minutes)
const IDLE_TIMEOUT_SECS: u64 = 300;

/// How often to check whether the embedding model can be unloaded.
const EMBEDDING_IDLE_CHECK_SECS: u64 = 30;

/// How often the retained-heap watchdog samples allocator retention.
const HEAP_RETENTION_CHECK_SECS: u64 = 120;

/// Exit code when server shuts down due to idle timeout
pub const EXIT_IDLE_TIMEOUT: i32 = 44;

/// Server state
pub struct Server {
    provider: Arc<dyn Provider>,
    socket_path: PathBuf,
    debug_socket_path: PathBuf,
    gateway_config_override: Option<crate::gateway::GatewayConfig>,
    /// Server identity for multi-server support
    identity: ServerIdentity,
    /// Broadcast channel for streaming events to all subscribers
    event_tx: broadcast::Sender<ServerEvent>,
    /// Active sessions (session_id -> Agent)
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    /// Current processing state
    is_processing: Arc<RwLock<bool>>,
    /// Session ID for the default session
    session_id: Arc<RwLock<String>>,
    /// Number of connected clients
    client_count: Arc<RwLock<usize>>,
    /// Connected client mapping (client_id -> session_id)
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    /// File-touch tracking service (forward path index + reverse session index)
    file_touch: FileTouchService,
    /// Shared ownership of core swarm coordination state.
    swarm_state: SwarmState,
    /// Shared context by swarm (swarm_id -> key -> SharedContext)
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    /// Active and available TUI debug channels (request_id, command)
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    /// Channel to receive client debug responses from TUI (request_id, response)
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    /// Background debug jobs (async debug commands)
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    /// Channel subscriptions (swarm_id -> channel -> session_ids)
    channel_subscriptions: ChannelSubscriptions,
    /// Reverse index for channel subscriptions: session_id -> swarm_id -> channels
    channel_subscriptions_by_session: ChannelSubscriptions,
    /// Event history for real-time event subscription (ring buffer)
    event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    /// Counter for event IDs
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    /// Broadcast channel for swarm event subscriptions (debug socket subscribers)
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    /// Ambient mode runner handle (None if ambient is disabled)
    ambient_runner: Option<AmbientRunnerHandle>,
    /// Shared MCP server pool (processes shared across sessions), initialized lazily.
    mcp_pool: Arc<OnceCell<Arc<crate::mcp::SharedMcpPool>>>,
    /// Graceful shutdown signals by session_id (stored outside agent mutex so they
    /// can be signaled without locking the agent during active tool execution)
    shutdown_signals: Arc<RwLock<HashMap<String, InterruptSignal>>>,
    /// Soft interrupt queues by session_id (stored outside agent mutex so swarm/debug
    /// notifications can be enqueued while an agent is actively processing)
    soft_interrupt_queues: SessionInterruptQueues,
    /// Persisted communicate await_members wait registry.
    await_members_runtime: AwaitMembersRuntime,
    /// Persisted dedupe registry for mutating swarm coordinator operations.
    swarm_mutation_runtime: SwarmMutationRuntime,
}

impl Server {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self::new_with_name(provider, None)
    }

    pub fn new_with_name(provider: Arc<dyn Provider>, server_name: Option<String>) -> Self {
        use crate::id::{new_id, new_memorable_server_id, server_icon};

        // Register the live provider so background helpers (the memory sidecar)
        // can make cheap model calls on whatever provider the user is running.
        // Without this, the sidecar only works on OpenAI/Claude OAuth and
        // silently degrades (rerank -> hybrid order, no relevance/extraction) on
        // Copilot, Antigravity, Gemini, Cursor, Bedrock, and OpenRouter.
        crate::provider::set_active_provider(Arc::clone(&provider));

        let (event_tx, _) = broadcast::channel(1024);
        let (client_debug_response_tx, _) = broadcast::channel(64);

        // Generate a memorable server name unless the operator configured a
        // stable one for long-lived remote runtimes.
        let (id, name) = match configured_server_name(server_name) {
            Some(name) => (new_id(&format!("server_{name}")), name),
            None => new_memorable_server_id(),
        };
        let icon = server_icon(&name).to_string();
        let identity = ServerIdentity {
            id,
            name,
            icon,
            git_hash: jcode_build_meta::git_hash().to_string(),
            version: jcode_build_meta::version().to_string(),
        };
        crate::process_title::set_server_title(&identity.name);

        // Initialize the background runner even when ambient mode is disabled so
        // session-targeted scheduled tasks still have a live delivery loop.
        let ambient_runner = {
            let safety = Arc::new(crate::safety::SafetySystem::new());
            let handle = AmbientRunnerHandle::new(safety);
            crate::tool::ambient::init_schedule_runner(handle.clone());
            Some(handle)
        };

        let LoadedSwarmRuntimeState {
            plans: restored_swarm_plans,
            coordinators: restored_swarm_coordinators,
            members: restored_swarm_members,
            swarms_by_id: restored_swarms_by_id,
        } = load_persisted_swarm_runtime_state();

        Self {
            provider,
            socket_path: socket_path(),
            debug_socket_path: debug_socket_path(),
            gateway_config_override: None,
            identity,
            event_tx,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            is_processing: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(String::new())),
            client_count: Arc::new(RwLock::new(0)),
            client_connections: Arc::new(RwLock::new(HashMap::new())),
            file_touch: FileTouchService::new(),
            swarm_state: SwarmState::new(
                restored_swarm_members,
                restored_swarms_by_id,
                restored_swarm_plans,
                restored_swarm_coordinators,
            ),
            shared_context: Arc::new(RwLock::new(HashMap::new())),
            client_debug_state: Arc::new(RwLock::new(ClientDebugState::default())),
            client_debug_response_tx,
            debug_jobs: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions_by_session: Arc::new(RwLock::new(HashMap::new())),
            event_history: Arc::new(RwLock::new(std::collections::VecDeque::new())),
            event_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            swarm_event_tx: broadcast::channel(256).0,
            ambient_runner,
            mcp_pool: Arc::new(OnceCell::new()),
            shutdown_signals: Arc::new(RwLock::new(HashMap::new())),
            soft_interrupt_queues: Arc::new(RwLock::new(HashMap::new())),
            await_members_runtime: AwaitMembersRuntime::default(),
            swarm_mutation_runtime: SwarmMutationRuntime::default(),
        }
    }

    pub fn new_with_paths(
        provider: Arc<dyn Provider>,
        socket_path: PathBuf,
        debug_socket_path: PathBuf,
    ) -> Self {
        let mut server = Self::new(provider);
        server.socket_path = socket_path;
        server.debug_socket_path = debug_socket_path;
        server
    }

    pub fn with_gateway_config(mut self, gateway_config: crate::gateway::GatewayConfig) -> Self {
        self.gateway_config_override = Some(gateway_config);
        self
    }

    /// Get the server identity
    pub fn identity(&self) -> &ServerIdentity {
        &self.identity
    }

    fn runtime(&self) -> ServerRuntime {
        ServerRuntime::from_server(self)
    }

    fn build_registry_info(&self) -> crate::registry::ServerInfo {
        crate::registry::ServerInfo {
            id: self.identity.id.clone(),
            name: self.identity.name.clone(),
            icon: self.identity.icon.clone(),
            socket: self.socket_path.clone(),
            debug_socket: self.debug_socket_path.clone(),
            git_hash: self.identity.git_hash.clone(),
            version: self.identity.version.clone(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            sessions: Vec::new(),
        }
    }

    fn spawn_registry_prewarm(&self) {
        let registry_warm_provider = Arc::clone(&self.provider);
        tokio::spawn(async move {
            let start = Instant::now();
            let provider = registry_warm_provider.fork();
            let _ = crate::tool::Registry::new(provider).await;
            crate::logging::info(&format!(
                "Registry prewarm completed in {}ms",
                start.elapsed().as_millis()
            ));
        });
    }

    async fn recover_headless_sessions_on_startup(&self) {
        let sessions_to_restore = {
            let members = self.swarm_state.members.read().await;
            members
                .values()
                .filter(|member| headless_member_should_restore(&member.status, member.is_headless))
                .map(|member| member.session_id.clone())
                .collect::<Vec<_>>()
        };

        if sessions_to_restore.is_empty() {
            return;
        }

        crate::logging::info(&format!(
            "Recovering {} headless session(s) after startup: {:?}",
            sessions_to_restore.len(),
            sessions_to_restore
        ));

        if let Some(delay) = startup_headless_recovery_test_delay() {
            crate::logging::info(&format!(
                "Applying test-only headless startup recovery delay of {}ms",
                delay.as_millis()
            ));
            tokio::time::sleep(delay).await;
        }

        let mcp_pool = get_shared_mcp_pool(&self.mcp_pool).await;
        let recovery_started = Instant::now();
        let mut stats = HeadlessRecoveryStats::default();
        let mut swarms_to_persist = HashSet::new();

        for session_id in sessions_to_restore {
            stats.candidates += 1;
            let session = match crate::session::Session::load(&session_id) {
                Ok(session) => session,
                Err(error) => {
                    stats.failed_to_load += 1;
                    crate::logging::warn(&format!(
                        "Failed to load headless session {} during startup recovery: {}",
                        session_id, error
                    ));
                    update_member_status(
                        &session_id,
                        "failed",
                        Some(truncate_detail(&error.to_string(), 120)),
                        &self.swarm_state.members,
                        &self.swarm_state.swarms_by_id,
                        Some(&self.event_history),
                        Some(&self.event_counter),
                        Some(&self.swarm_event_tx),
                    )
                    .await;
                    if let Some(swarm_id) = {
                        let members = self.swarm_state.members.read().await;
                        members
                            .get(&session_id)
                            .and_then(|member| member.swarm_id.clone())
                    } {
                        persist_swarm_state_for(&swarm_id, &self.swarm_state).await;
                    }
                    continue;
                }
            };

            let previous_status = session.status.clone();
            let provider = self.provider.fork();
            let registry = crate::tool::Registry::new(provider.clone()).await;
            if session.is_canary {
                registry.register_selfdev_tools().await;
            }
            registry
                .register_mcp_tools_for_dir(
                    None,
                    Some(Arc::clone(&mcp_pool)),
                    Some("headless".to_string()),
                    session.working_dir.as_ref().map(std::path::PathBuf::from),
                )
                .await;

            let agent = Arc::new(Mutex::new(Agent::new_with_session(
                provider, registry, session, None,
            )));

            {
                let mut sessions = self.sessions.write().await;
                if sessions.contains_key(&session_id) {
                    continue;
                }
                sessions.insert(session_id.clone(), Arc::clone(&agent));
            }

            {
                let agent_guard = agent.lock().await;
                register_session_interrupt_queue(
                    &self.soft_interrupt_queues,
                    &session_id,
                    agent_guard.soft_interrupt_queue(),
                )
                .await;
                let mut shutdown_signals = self.shutdown_signals.write().await;
                shutdown_signals.insert(session_id.clone(), agent_guard.graceful_shutdown_signal());
                register_background_tool_signal(&session_id, agent_guard.background_tool_signal());
            }

            let stored_recovery_record = reload_recovery::peek_for_session(&session_id)
                .ok()
                .flatten();
            let has_stored_recovery_intent = stored_recovery_record
                .as_ref()
                .map(|record| record.status == reload_recovery::ReloadRecoveryStatus::Pending)
                .unwrap_or(false);
            let should_resume = has_stored_recovery_intent || {
                let agent_guard = agent.lock().await;
                self::client_session::restored_session_was_interrupted(
                    &session_id,
                    &previous_status,
                    &agent_guard,
                )
            };
            if let Some(record) = stored_recovery_record.as_ref() {
                reload_trace::record_value(
                    &record.reload_id,
                    "startup_recovery_decision",
                    serde_json::json!({
                        "session_id": session_id,
                        "has_stored_recovery_intent": has_stored_recovery_intent,
                        "should_resume": should_resume,
                        "previous_status": previous_status,
                        "is_headless": true,
                    }),
                );
            }

            if !should_resume {
                ReloadContext::log_recovery_outcome(
                    "server_startup_headless",
                    &session_id,
                    "skipped",
                    "restored session was not interrupted by reload",
                );
                stats.skipped += 1;
                update_member_status(
                    &session_id,
                    "ready",
                    None,
                    &self.swarm_state.members,
                    &self.swarm_state.swarms_by_id,
                    Some(&self.event_history),
                    Some(&self.event_counter),
                    Some(&self.swarm_event_tx),
                )
                .await;
                if let Some(swarm_id) = {
                    let members = self.swarm_state.members.read().await;
                    members
                        .get(&session_id)
                        .and_then(|member| member.swarm_id.clone())
                } {
                    swarms_to_persist.insert(swarm_id);
                }
                continue;
            }

            let stored_directive = reload_recovery::pending_directive_for_session(&session_id)
                .ok()
                .flatten();
            let reload_ctx = if stored_directive.is_none() {
                ReloadContext::load_for_session(&session_id).ok().flatten()
            } else {
                None
            };
            let reminder = stored_directive
                .map(|directive| directive.continuation_message)
                .or_else(|| headless_reload_continuation_message(reload_ctx));
            let Some(reminder) = reminder else {
                ReloadContext::log_recovery_outcome(
                    "server_startup_headless",
                    &session_id,
                    "failed",
                    "recovery directive missing for interrupted headless session",
                );
                continue;
            };
            stats.resumed += 1;
            ReloadContext::log_recovery_outcome(
                "server_startup_headless",
                &session_id,
                "resuming",
                "restored interrupted headless session after reload",
            );
            let recover_swarm_members = Arc::clone(&self.swarm_state.members);
            let recover_swarms_by_id = Arc::clone(&self.swarm_state.swarms_by_id);
            let recover_event_history = Arc::clone(&self.event_history);
            let recover_event_counter = Arc::clone(&self.event_counter);
            let recover_swarm_event_tx = self.swarm_event_tx.clone();
            let recover_swarm_state = self.swarm_state.clone();
            let recovery_reload_id = stored_recovery_record.map(|record| record.reload_id);

            tokio::spawn(async move {
                if let Some(reload_id) = recovery_reload_id.as_deref() {
                    reload_trace::record_value(
                        reload_id,
                        "continuation_started",
                        serde_json::json!({
                            "session_id": session_id,
                            "source": "server_startup_headless",
                        }),
                    );
                }
                update_member_status(
                    &session_id,
                    "running",
                    Some("resuming after reload".to_string()),
                    &recover_swarm_members,
                    &recover_swarms_by_id,
                    Some(&recover_event_history),
                    Some(&recover_event_counter),
                    Some(&recover_swarm_event_tx),
                )
                .await;
                if let Some(swarm_id) = {
                    let members = recover_swarm_members.read().await;
                    members
                        .get(&session_id)
                        .and_then(|member| member.swarm_id.clone())
                } {
                    persist_swarm_state_for(&swarm_id, &recover_swarm_state).await;
                }

                match reload_recovery::mark_delivered_if_matching_continuation(
                    &session_id,
                    &reminder,
                    "server_startup_headless",
                ) {
                    Ok(true) => {}
                    Ok(false) => {}
                    Err(error) => crate::logging::warn(&format!(
                        "Failed to mark headless reload recovery intent delivered for {}: {}",
                        session_id, error
                    )),
                }

                let event_tx = self::state::session_event_fanout_sender(
                    session_id.clone(),
                    Arc::clone(&recover_swarm_members),
                );
                let result = self::client_lifecycle::process_message_streaming_mpsc(
                    Arc::clone(&agent),
                    "",
                    vec![],
                    Some(reminder),
                    event_tx,
                )
                .await;

                let (status, detail) = match result {
                    Ok(()) => {
                        if let Some(reload_id) = recovery_reload_id.as_deref() {
                            reload_trace::record_value(
                                reload_id,
                                "continuation_finished",
                                serde_json::json!({
                                    "session_id": session_id,
                                    "source": "server_startup_headless",
                                    "status": "ready",
                                }),
                            );
                        }
                        ReloadContext::log_recovery_outcome(
                            "server_startup_headless",
                            &session_id,
                            "resumed",
                            "continuation dispatched successfully",
                        );
                        ("ready", None)
                    }
                    Err(error) => {
                        if let Some(reload_id) = recovery_reload_id.as_deref() {
                            reload_trace::record_value(
                                reload_id,
                                "continuation_failed",
                                serde_json::json!({
                                    "session_id": session_id,
                                    "source": "server_startup_headless",
                                    "error": error.to_string(),
                                }),
                            );
                        }
                        ReloadContext::log_recovery_outcome(
                            "server_startup_headless",
                            &session_id,
                            "failed",
                            &error.to_string(),
                        );
                        ("failed", Some(truncate_detail(&error.to_string(), 120)))
                    }
                };
                update_member_status(
                    &session_id,
                    status,
                    detail,
                    &recover_swarm_members,
                    &recover_swarms_by_id,
                    Some(&recover_event_history),
                    Some(&recover_event_counter),
                    Some(&recover_swarm_event_tx),
                )
                .await;
                if let Some(swarm_id) = {
                    let members = recover_swarm_members.read().await;
                    members
                        .get(&session_id)
                        .and_then(|member| member.swarm_id.clone())
                } {
                    persist_swarm_state_for(&swarm_id, &recover_swarm_state).await;
                }
            });
        }

        for swarm_id in swarms_to_persist {
            persist_swarm_state_for(&swarm_id, &self.swarm_state).await;
        }

        crate::logging::info(&format!(
            "[TIMING] headless reload startup recovery: candidates={}, resumed={}, skipped={}, failed_to_load={}, total={}ms",
            stats.candidates,
            stats.resumed,
            stats.skipped,
            stats.failed_to_load,
            recovery_started.elapsed().as_millis()
        ));
    }

    async fn finish_startup_after_bind(
        &self,
        main_listener: Listener,
        debug_listener: Listener,
        server_start_time: Instant,
    ) -> (
        ServerRuntime,
        tokio::task::JoinHandle<()>,
        tokio::task::JoinHandle<()>,
    ) {
        self.spawn_registry_prewarm();
        let registry_info = self.build_registry_info();

        let runtime = self.runtime();
        let main_handle = runtime.spawn_main_accept_loop(main_listener);
        let debug_handle = runtime.spawn_debug_accept_loop(debug_listener, server_start_time);

        crate::logging::info("Accept loop tasks spawned");

        // Signal readiness to the spawning client only after the accept loops
        // are live, so a "ready" server can immediately handle requests.
        publish_reload_socket_ready();
        signal_ready_fd();

        // Persist auxiliary discovery metadata after the server is already live.
        self.spawn_registry_metadata_publisher(registry_info);

        // Spawn WebSocket gateway for iOS/web clients (if enabled)
        self.spawn_gateway(runtime.clone()).await;

        // Startup recovery can be expensive in multi-session reloads. Run it
        // only after the replacement daemon is already accepting reconnects.
        self.recover_headless_sessions_on_startup().await;

        (runtime, main_handle, debug_handle)
    }

    fn spawn_background_tasks(
        &self,
        server_start_time: Instant,
        temporary_server_policy: Option<lifecycle::TemporaryServerPolicy>,
    ) {
        // Preload the embedding model in background so warm startups get fast
        // memory recall. On a cold install, skip eager preload because the
        // first-time model download can make the first spawned client look hung
        // while the daemon finishes bootstrapping.
        if crate::embedding::is_model_available() {
            tokio::task::spawn_blocking(|| {
                let start = std::time::Instant::now();
                match crate::embedding::get_embedder() {
                    Ok(_) => {
                        crate::logging::info(&format!(
                            "Embedding model preloaded in {}ms",
                            start.elapsed().as_millis()
                        ));
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Embedding model preload failed (non-fatal): {}",
                            e
                        ));
                    }
                }
            });
        } else {
            crate::logging::info(
                "Embedding model not installed yet; skipping eager preload during server startup",
            );
        }

        // Warm the lightweight session-search index after daemon startup. This
        // keeps the first agent `session_search` call from paying the cold
        // indexing cost while leaving exhaustive searches available on demand.
        crate::tool::spawn_recent_index_warmup();

        // Reconcile background-task status files orphaned by a previous
        // process image (crash or exec-based reload). Non-detached tasks die
        // with their owning process but their status files still say Running,
        // which leaves phantom entries in `bg list` and blocks `bg wait`
        // until timeout. Detached tasks are untouched (they survive reloads
        // and reconcile via their real pid).
        tokio::spawn(async move {
            let reconciled = crate::background::global().reconcile_orphaned_tasks().await;
            if reconciled > 0 {
                crate::logging::info(&format!(
                    "Marked {} orphaned background task(s) from a previous server process as failed",
                    reconciled
                ));
            }
        });

        // Spawn reload monitor (event-driven via in-process channel).
        // In the unified server design, self-dev sessions share the main server,
        // so the shared server must always listen for reload signals.
        let signal_sessions = Arc::clone(&self.sessions);
        let signal_swarm_members = Arc::clone(&self.swarm_state.members);
        let signal_shutdown_signals = Arc::clone(&self.shutdown_signals);
        let signal_swarm_event_tx = self.swarm_event_tx.clone();
        tokio::spawn(async move {
            await_reload_signal(
                signal_sessions,
                signal_swarm_members,
                signal_shutdown_signals,
                signal_swarm_event_tx,
            )
            .await;
        });

        // Log when we receive SIGTERM for debugging
        #[cfg(unix)]
        {
            let sigterm_server_name = self.identity.name.clone();
            tokio::spawn(async move {
                use tokio::signal::unix::{SignalKind, signal};
                if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                    sigterm.recv().await;
                    crate::logging::info("Server received SIGTERM, shutting down gracefully");
                    let _ = crate::registry::unregister_server(&sigterm_server_name).await;
                    std::process::exit(0);
                }
            });
        }

        // Spawn the bus monitor for swarm coordination
        let monitor_file_touch = self.file_touch.clone();
        let monitor_swarm_members = Arc::clone(&self.swarm_state.members);
        let monitor_swarms_by_id = Arc::clone(&self.swarm_state.swarms_by_id);
        let monitor_swarm_plans = Arc::clone(&self.swarm_state.plans);
        let monitor_swarm_coordinators = Arc::clone(&self.swarm_state.coordinators);
        let monitor_shared_context = Arc::clone(&self.shared_context);
        let monitor_sessions = Arc::clone(&self.sessions);
        let monitor_soft_interrupt_queues = Arc::clone(&self.soft_interrupt_queues);
        let monitor_event_history = Arc::clone(&self.event_history);
        let monitor_event_counter = Arc::clone(&self.event_counter);
        let monitor_swarm_event_tx = self.swarm_event_tx.clone();
        tokio::spawn(async move {
            Self::monitor_bus(
                monitor_file_touch,
                monitor_swarm_members,
                monitor_swarms_by_id,
                monitor_swarm_plans,
                monitor_swarm_coordinators,
                monitor_shared_context,
                monitor_sessions,
                monitor_soft_interrupt_queues,
                monitor_event_history,
                monitor_event_counter,
                monitor_swarm_event_tx,
            )
            .await;
        });

        // Resume any background `swarm await_members` watchers that were active
        // before this (re)start. Their results are delivered via notify/wake, so
        // they can pick up transparently without the agent rerunning the wait.
        {
            let resume_swarm_members = Arc::clone(&self.swarm_state.members);
            let resume_swarms_by_id = Arc::clone(&self.swarm_state.swarms_by_id);
            let resume_swarm_event_tx = self.swarm_event_tx.clone();
            let resume_await_runtime = self.await_members_runtime.clone();
            tokio::spawn(async move {
                comm_await::resume_background_awaits(
                    &resume_swarm_members,
                    &resume_swarms_by_id,
                    &resume_swarm_event_tx,
                    &resume_await_runtime,
                )
                .await;
            });
        }

        let stale_swarm_members = Arc::clone(&self.swarm_state.members);
        let stale_swarms_by_id = Arc::clone(&self.swarm_state.swarms_by_id);
        let stale_swarm_plans = Arc::clone(&self.swarm_state.plans);
        let stale_swarm_coordinators = Arc::clone(&self.swarm_state.coordinators);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(crate::server::swarm::swarm_task_sweep_interval());
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                refresh_swarm_task_staleness(
                    &stale_swarm_members,
                    &stale_swarms_by_id,
                    &stale_swarm_plans,
                    &stale_swarm_coordinators,
                )
                .await;
            }
        });

        let gc_sessions = Arc::clone(&self.sessions);
        let gc_swarm_state = self.swarm_state.clone();
        let gc_channel_subscriptions = Arc::clone(&self.channel_subscriptions);
        let gc_channel_subscriptions_by_session =
            Arc::clone(&self.channel_subscriptions_by_session);
        let gc_soft_interrupt_queues = Arc::clone(&self.soft_interrupt_queues);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(swarm::swarm_terminal_member_gc_interval());
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                prune_expired_terminal_swarm_members(
                    &gc_sessions,
                    &gc_swarm_state,
                    &gc_channel_subscriptions,
                    &gc_channel_subscriptions_by_session,
                )
                .await;
                // Backstop for coordinator cleanup: close finished spawned
                // workers that have been idle past the reap window so they do
                // not accumulate one leaked client process each.
                reap_idle_spawned_workers(
                    &gc_sessions,
                    &gc_swarm_state,
                    &gc_channel_subscriptions,
                    &gc_channel_subscriptions_by_session,
                    &gc_soft_interrupt_queues,
                )
                .await;
            }
        });

        // Keep the machine awake while any session is actively streaming/processing.
        // This watches the same "running" member signal Waybar surfaces as
        // "N streaming" and toggles a best-effort OS power inhibitor accordingly.
        Self::spawn_power_inhibitor(Arc::clone(&self.swarm_state.members));

        // Initialize the memory agent early so it's ready for all sessions
        if crate::config::config().features.memory {
            tokio::spawn(async {
                let _ = crate::memory_agent::init().await;
            });
        }

        // Spawn the background ambient/schedule loop.
        if let Some(ref runner) = self.ambient_runner {
            let ambient_handle = runner.clone();
            let ambient_provider = Arc::clone(&self.provider);
            crate::logging::info("Starting ambient/schedule background loop");
            tokio::spawn(async move {
                ambient_handle.run_loop(ambient_provider).await;
            });
        }

        // Spawn the Jade cloud relay listener independently of ambient mode. The
        // worker is strictly opt-in and requires an explicit API base, token,
        // session id, and reply-enabled flag before it makes any outbound calls.
        jade_relay::spawn_if_configured(
            &crate::config::config().safety,
            Arc::clone(&self.sessions),
            Arc::clone(&self.soft_interrupt_queues),
            Arc::clone(&self.shutdown_signals),
            Arc::clone(&self.swarm_state.members),
        );

        // Spawn embedding idle monitor so the model can be unloaded when this
        // server has been quiet for a while.
        let embedding_idle_secs = embedding_idle_unload_secs();
        tokio::spawn(async move {
            let idle_for = std::time::Duration::from_secs(embedding_idle_secs);
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(EMBEDDING_IDLE_CHECK_SECS));
            loop {
                interval.tick().await;
                let unloaded = crate::embedding::maybe_unload_if_idle(idle_for);
                if unloaded {
                    let stats = crate::embedding::stats();
                    crate::logging::info(&format!(
                        "Embedding idle monitor: model unloaded (loads={}, unloads={}, calls={}, avg_ms={})",
                        stats.load_count,
                        stats.unload_count,
                        stats.embed_calls,
                        stats
                            .avg_embed_ms
                            .map(|v| format!("{:.1}", v))
                            .unwrap_or_else(|| "n/a".to_string())
                    ));
                }
            }
        });

        // Spawn the retained-heap watchdog: glibc/jemalloc keep freed pages
        // inside arenas, and the event-driven trim hooks (turn completion,
        // history load) rarely fire on a server hosting mostly-idle sessions.
        // Periodically check the allocator's freed-but-retained byte count and
        // trim when it crosses the threshold, returning the pages to the OS.
        let retention_threshold = crate::process_memory::retention_trim_threshold_bytes();
        if retention_threshold != u64::MAX {
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                    HEAP_RETENTION_CHECK_SECS,
                ));
                loop {
                    interval.tick().await;
                    crate::process_memory::release_retained_heap_if_excessive(
                        "server_retention_watchdog",
                        retention_threshold,
                        std::time::Duration::from_secs(60),
                    );
                }
            });
        }

        if crate::runtime_memory_log::server_logging_enabled() {
            let log_identity = self.identity.clone();
            let log_sessions = Arc::clone(&self.sessions);
            let log_client_count = Arc::clone(&self.client_count);
            let (memory_event_tx, mut memory_event_rx) = mpsc::unbounded_channel();
            crate::runtime_memory_log::install_event_sink(memory_event_tx);
            tokio::spawn(async move {
                match crate::runtime_memory_log::prune_old_server_logs() {
                    Ok(removed) if removed > 0 => {
                        crate::logging::info(&format!(
                            "Runtime memory logging pruned {} old log files",
                            removed
                        ));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        crate::logging::info(&format!(
                            "Runtime memory logging could not prune old logs: {}",
                            err
                        ));
                    }
                }

                let log_config = crate::runtime_memory_log::server_logging_config();
                match crate::runtime_memory_log::current_server_log_path() {
                    Ok(path) => crate::logging::info(&format!(
                        "Runtime memory logging enabled: process={}s attribution={}s -> {}",
                        log_config.process_interval.as_secs(),
                        log_config.attribution_interval.as_secs(),
                        path.display()
                    )),
                    Err(err) => crate::logging::info(&format!(
                        "Runtime memory logging enabled: process={}s attribution={}s (path unavailable: {})",
                        log_config.process_interval.as_secs(),
                        log_config.attribution_interval.as_secs(),
                        err
                    )),
                }

                let mut controller = RuntimeMemoryLogController::new(log_config);
                let startup_now = Instant::now();
                let mut startup_sample = capture_runtime_memory_attribution_sample(
                    &log_identity,
                    &log_sessions,
                    &log_client_count,
                    server_start_time,
                    "attribution:startup",
                    RuntimeMemoryLogTrigger {
                        category: "startup".to_string(),
                        reason: "server_start".to_string(),
                        session_id: None,
                        detail: None,
                    },
                    RuntimeMemoryLogSampling {
                        forced: true,
                        threshold_reasons: vec!["initial_attribution".to_string()],
                        pending_event_count: 0,
                        pending_categories: Vec::new(),
                    },
                )
                .await;
                controller.record_process_sample(startup_now);
                controller.finalize_attribution_sample(startup_now, &mut startup_sample);
                if let Err(err) = crate::runtime_memory_log::append_server_sample(&startup_sample) {
                    crate::logging::info(&format!(
                        "Runtime memory logging startup sample failed: {}",
                        err
                    ));
                }

                let mut process_interval =
                    tokio::time::interval(controller.config().process_interval);
                let mut attribution_interval =
                    tokio::time::interval(controller.config().attribution_interval);
                process_interval.tick().await;
                attribution_interval.tick().await;
                loop {
                    tokio::select! {
                        _ = process_interval.tick() => {
                            let now = Instant::now();
                            let process_sample = capture_runtime_memory_process_sample(
                                &log_identity,
                                &log_client_count,
                                server_start_time,
                                "process:heartbeat",
                                RuntimeMemoryLogTrigger {
                                    category: "process_heartbeat".to_string(),
                                    reason: "periodic".to_string(),
                                    session_id: None,
                                    detail: None,
                                },
                                controller.build_sampling_for_process(None),
                            )
                            .await;
                            controller.record_process_sample(now);
                            if let Err(err) = crate::runtime_memory_log::append_server_sample(&process_sample) {
                                crate::logging::info(&format!(
                                    "Runtime memory logging process heartbeat sample failed: {}",
                                    err
                                ));
                            }

                            if let Some(sampling) = controller.build_sampling_for_attribution(
                                now,
                                &process_sample.process,
                                None,
                                None,
                            ) {
                                let mut attribution_sample = capture_runtime_memory_attribution_sample(
                                    &log_identity,
                                    &log_sessions,
                                    &log_client_count,
                                    server_start_time,
                                    "attribution:process-heartbeat",
                                    RuntimeMemoryLogTrigger {
                                        category: "process_heartbeat".to_string(),
                                        reason: "threshold_flush".to_string(),
                                        session_id: None,
                                        detail: None,
                                    },
                                    sampling,
                                )
                                .await;
                                controller.finalize_attribution_sample(now, &mut attribution_sample);
                                if let Err(err) = crate::runtime_memory_log::append_server_sample(&attribution_sample) {
                                    crate::logging::info(&format!(
                                        "Runtime memory logging attribution flush failed: {}",
                                        err
                                    ));
                                }
                            }
                        }
                        _ = attribution_interval.tick() => {
                            let now = Instant::now();
                            let preflight = capture_runtime_memory_process_sample(
                                &log_identity,
                                &log_client_count,
                                server_start_time,
                                "process:attribution-preflight",
                                RuntimeMemoryLogTrigger {
                                    category: "attribution_heartbeat".to_string(),
                                    reason: "preflight".to_string(),
                                    session_id: None,
                                    detail: None,
                                },
                                RuntimeMemoryLogSampling::default(),
                            )
                            .await;
                            if let Some(sampling) = controller.build_sampling_for_attribution(
                                now,
                                &preflight.process,
                                None,
                                Some("attribution_heartbeat"),
                            ) {
                                let mut attribution_sample = capture_runtime_memory_attribution_sample(
                                    &log_identity,
                                    &log_sessions,
                                    &log_client_count,
                                    server_start_time,
                                    "attribution:heartbeat",
                                    RuntimeMemoryLogTrigger {
                                        category: "attribution_heartbeat".to_string(),
                                        reason: "periodic".to_string(),
                                        session_id: None,
                                        detail: None,
                                    },
                                    sampling,
                                )
                                .await;
                                controller.finalize_attribution_sample(now, &mut attribution_sample);
                                if let Err(err) = crate::runtime_memory_log::append_server_sample(&attribution_sample) {
                                    crate::logging::info(&format!(
                                        "Runtime memory logging attribution heartbeat failed: {}",
                                        err
                                    ));
                                }
                            } else {
                                controller.mark_attribution_heartbeat_pending();
                            }
                        }
                        maybe_event = memory_event_rx.recv() => {
                            let Some(event) = maybe_event else {
                                break;
                            };
                            let now = Instant::now();
                            let should_write_process = controller.should_write_process_for_event(now, &event);
                            let process_sample = if should_write_process {
                                Some(
                                    capture_runtime_memory_process_sample(
                                        &log_identity,
                                        &log_client_count,
                                        server_start_time,
                                        &format!("process:event:{}", event.category),
                                        RuntimeMemoryLogTrigger {
                                            category: event.category.clone(),
                                            reason: event.reason.clone(),
                                            session_id: event.session_id.clone(),
                                            detail: event.detail.clone(),
                                        },
                                        controller.build_sampling_for_process(Some(&event)),
                                    )
                                    .await,
                                )
                            } else {
                                None
                            };

                            if let Some(process_sample) = process_sample.as_ref() {
                                controller.record_process_sample(now);
                                if let Err(err) = crate::runtime_memory_log::append_server_sample(process_sample) {
                                    crate::logging::info(&format!(
                                        "Runtime memory logging event process sample failed: {}",
                                        err
                                    ));
                                }
                            }

                            let mut wrote_attribution = false;
                            let preflight_sample = if process_sample.is_none() && controller.can_write_attribution(now) {
                                Some(
                                    capture_runtime_memory_process_sample(
                                        &log_identity,
                                        &log_client_count,
                                        server_start_time,
                                        &format!("process:event-preflight:{}", event.category),
                                        RuntimeMemoryLogTrigger {
                                            category: event.category.clone(),
                                            reason: "preflight".to_string(),
                                            session_id: event.session_id.clone(),
                                            detail: event.detail.clone(),
                                        },
                                        RuntimeMemoryLogSampling::default(),
                                    )
                                    .await,
                                )
                            } else {
                                None
                            };
                            let preflight = process_sample.as_ref().or(preflight_sample.as_ref());
                            if let Some(preflight) = preflight
                                && let Some(sampling) = controller.build_sampling_for_attribution(
                                    now,
                                    &preflight.process,
                                    Some(&event),
                                    None,
                                )
                            {
                                    let mut attribution_sample = capture_runtime_memory_attribution_sample(
                                        &log_identity,
                                        &log_sessions,
                                        &log_client_count,
                                        server_start_time,
                                        &format!("attribution:event:{}", event.category),
                                        RuntimeMemoryLogTrigger {
                                            category: event.category.clone(),
                                            reason: event.reason.clone(),
                                            session_id: event.session_id.clone(),
                                            detail: event.detail.clone(),
                                        },
                                        sampling,
                                    )
                                    .await;
                                    controller.finalize_attribution_sample(now, &mut attribution_sample);
                                    wrote_attribution = true;
                                    if let Err(err) = crate::runtime_memory_log::append_server_sample(&attribution_sample) {
                                        crate::logging::info(&format!(
                                            "Runtime memory logging event attribution sample failed: {}",
                                            err
                                        ));
                                    }
                                }

                            if !wrote_attribution {
                                controller.defer_event(event);
                            }
                        }
                    }
                }
            });
        }

        if let Some(policy) = temporary_server_policy {
            lifecycle::spawn_temporary_lifecycle_monitor(
                Arc::clone(&self.client_count),
                self.socket_path.clone(),
                self.debug_socket_path.clone(),
                self.identity.name.clone(),
                policy,
            );
        } else if debug_control_allowed() {
            crate::logging::info("Debug control enabled; idle timeout monitor disabled.");
        } else {
            let idle_client_count = Arc::clone(&self.client_count);
            let idle_server_name = self.identity.name.clone();
            tokio::spawn(async move {
                let mut idle_since: Option<std::time::Instant> = None;
                let mut check_interval = tokio::time::interval(std::time::Duration::from_secs(10));

                loop {
                    check_interval.tick().await;

                    let count = *idle_client_count.read().await;

                    if count == 0 {
                        // No clients connected
                        if idle_since.is_none() {
                            idle_since = Some(std::time::Instant::now());
                            crate::logging::info(&format!(
                                "No clients connected. Server will exit after {} minutes of idle.",
                                IDLE_TIMEOUT_SECS / 60
                            ));
                        }

                        if let Some(since) = idle_since {
                            let idle_duration = since.elapsed().as_secs();
                            if idle_duration >= IDLE_TIMEOUT_SECS {
                                crate::logging::info(&format!(
                                    "Server idle for {} minutes with no clients. Shutting down.",
                                    idle_duration / 60
                                ));
                                let _ = crate::registry::unregister_server(&idle_server_name).await;
                                std::process::exit(EXIT_IDLE_TIMEOUT);
                            }
                        }
                    } else {
                        // Clients connected - reset idle timer
                        if idle_since.is_some() {
                            crate::logging::info("Client connected. Idle timer cancelled.");
                        }
                        idle_since = None;
                    }
                }
            });
        }
    }

    fn spawn_registry_metadata_publisher(&self, registry_info: crate::registry::ServerInfo) {
        let registry_identity = self.identity.display_name();
        tokio::spawn(async move {
            let hash_path = format!("{}.hash", registry_info.socket.display());
            let _ = std::fs::write(&hash_path, jcode_build_meta::git_hash());

            let mut registry = crate::registry::ServerRegistry::load()
                .await
                .unwrap_or_default();
            registry.register(registry_info);
            let _ = registry.save().await;
            crate::logging::info(&format!(
                "Registered as {} in server registry",
                registry_identity,
            ));

            if let Ok(mut registry) = crate::registry::ServerRegistry::load().await {
                let _ = registry.cleanup_stale().await;
                let _ = registry.save().await;
            }
        });
    }

    /// Spawn the background loop that keeps the machine awake while any session
    /// is actively streaming/processing.
    ///
    /// The shared daemon owns every session, so a single inhibitor here covers
    /// all of them. We poll the swarm-member map (the authoritative "running"
    /// signal that also drives Waybar's "N streaming" indicator) on a short
    /// interval and reconcile a best-effort OS power inhibitor against it. The
    /// inhibitor blocks automatic system sleep; Linux also blocks lid-switch
    /// handling. Windows still honors explicit lid/power-button actions from the
    /// active power plan. The display can turn off. When no session is running,
    /// the guard is released so normal power management resumes immediately.
    fn spawn_power_inhibitor(swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>) {
        // Reconcile interval. Short enough that the inhibitor engages promptly
        // when a turn starts and releases promptly when work finishes, but cheap
        // (a read lock + a scan) so it adds no meaningful load.
        const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

        let mut inhibitor = crate::power_inhibit::PowerInhibitor::new();
        if !inhibitor.is_available() {
            // Disabled via the legacy env escape hatch, or unsupported platform.
            crate::logging::info(
                "power_inhibit: unavailable (unsupported platform or JCODE_DISABLE_POWER_INHIBIT set); not monitoring",
            );
            return;
        }

        crate::logging::info(
            "power_inhibit: monitoring active sessions to prevent sleep while streaming",
        );

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_active: Option<bool> = None;
            loop {
                interval.tick().await;

                // Re-evaluate the config each tick so toggling it at runtime
                // takes effect without restarting the daemon.
                let enabled = crate::config::config().power.prevent_sleep_while_streaming;

                let active = enabled && Self::any_session_streaming(&swarm_members).await;
                if last_active != Some(active) {
                    crate::logging::info(&format!(
                        "power_inhibit: {} (streaming sessions {})",
                        if active { "engaging" } else { "releasing" },
                        if active { "present" } else { "absent" },
                    ));
                    last_active = Some(active);
                }
                inhibitor.set_active(active);
            }
        });
    }

    /// Whether at least one session is currently in the "running" state, i.e.
    /// actively streaming/processing a turn. This is the same signal that drives
    /// the Waybar "N streaming" indicator.
    async fn any_session_streaming(
        swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    ) -> bool {
        let members = swarm_members.read().await;
        members.values().any(|member| member.status == "running")
    }

    /// Monitor the global Bus for FileTouch events and detect conflicts
    #[expect(
        clippy::too_many_arguments,
        reason = "bus monitor needs file state, swarm state, sessions, queues, and event history sinks"
    )]
    async fn monitor_bus(
        file_touch: FileTouchService,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
        swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
        _swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
        _swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
        _shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
        sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
        soft_interrupt_queues: SessionInterruptQueues,
        event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
        event_counter: Arc<std::sync::atomic::AtomicU64>,
        swarm_event_tx: broadcast::Sender<SwarmEvent>,
    ) {
        let mut receiver = Bus::global().subscribe();
        let mut last_cleanup = Instant::now();
        const TOUCH_EXPIRY: Duration = Duration::from_secs(30 * 60); // 30 min
        const CLEANUP_INTERVAL: Duration = Duration::from_secs(5 * 60); // 5 min

        loop {
            // Periodic cleanup of expired file touches
            if last_cleanup.elapsed() > CLEANUP_INTERVAL {
                file_touch.expire_older_than(TOUCH_EXPIRY).await;
                last_cleanup = Instant::now();
            }

            match receiver.recv().await {
                Ok(BusEvent::FileTouch(touch)) => {
                    let path = touch.path.clone();
                    let session_id = touch.session_id.clone();

                    // Record this touch
                    file_touch
                        .record_touch(
                            path.clone(),
                            FileAccess {
                                session_id: session_id.clone(),
                                op: touch.op.clone(),
                                timestamp: Instant::now(),
                                absolute_time: std::time::SystemTime::now(),
                                intent: touch.intent.clone(),
                                summary: touch.summary.clone(),
                                detail: touch.detail.clone(),
                            },
                        )
                        .await;

                    // Record event for subscription
                    {
                        let members = swarm_members.read().await;
                        let member = members.get(&session_id);
                        let session_name = member.and_then(|m| m.friendly_name.clone());
                        let swarm_id = member.and_then(|m| m.swarm_id.clone());

                        drop(members);
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            session_id.clone(),
                            session_name,
                            swarm_id,
                            SwarmEventType::FileTouch {
                                path: path.to_string_lossy().to_string(),
                                op: touch.op.as_str().to_string(),
                                intent: touch.intent.clone(),
                                summary: touch.summary.clone(),
                                detail: touch.detail.clone(),
                            },
                        )
                        .await;
                    }

                    // Find the swarm this session belongs to
                    let swarm_session_ids: Vec<String> = {
                        let members = swarm_members.read().await;
                        if let Some(member) = members.get(&session_id) {
                            if let Some(ref swarm_id) = member.swarm_id {
                                let swarms = swarms_by_id.read().await;
                                if let Some(swarm) = swarms.get(swarm_id) {
                                    swarm.iter().cloned().collect()
                                } else {
                                    vec![]
                                }
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        }
                    };

                    // Only notify on modifications, and only about prior peer modifications.
                    // Plain reads are still tracked for later context/listing but should not
                    // proactively alert the swarm.
                    let is_modification = touch.op.is_modification();
                    if is_modification {
                        crate::logging::info(&format!(
                            "[file-activity] modification by {} on {}, swarm_peers: {:?}",
                            &session_id[..8.min(session_id.len())],
                            path.display(),
                            swarm_session_ids
                                .iter()
                                .map(|s| &s[..8.min(s.len())])
                                .collect::<Vec<_>>()
                        ));
                    }
                    let previous_touches: Vec<FileAccess> = if is_modification {
                        if let Some(accesses) = file_touch.accesses_for_path(&path).await {
                            let swarm_session_ids_set: HashSet<String> =
                                swarm_session_ids.iter().cloned().collect();
                            let result =
                                latest_peer_touches(&accesses, &session_id, &swarm_session_ids_set);
                            crate::logging::info(&format!(
                                "[file-activity] {} prior peer touches ({} total accesses)",
                                result.len(),
                                accesses.len()
                            ));
                            result
                        } else {
                            crate::logging::info("[file-activity] no touches for this path yet");
                            vec![]
                        }
                    } else {
                        vec![]
                    };

                    // If swarm peers previously touched this file, notify both sides so they
                    // can coordinate before the work diverges further.
                    if !previous_touches.is_empty() {
                        crate::logging::info(&format!(
                            "[file-activity] {} touched by peers before modification — sending alerts",
                            path.display()
                        ));
                        let members = swarm_members.read().await;
                        let current_member = members.get(&session_id);
                        let current_name = current_member.and_then(|m| m.friendly_name.clone());

                        // Alert the current agent about previous peer touches (one per agent).
                        if let Some(member) = current_member {
                            for prev in &previous_touches {
                                let prev_member = members.get(&prev.session_id);
                                let prev_name = prev_member.and_then(|m| m.friendly_name.clone());
                                let scope = file_activity_scope_label(prev, &touch);
                                let intent_suffix = prev
                                    .intent
                                    .as_ref()
                                    .map(|intent| format!(" — intent: {}", intent))
                                    .unwrap_or_default();
                                let alert_msg = format!(
                                    "⚠ File activity: {} — {} — {} previously {} this file{}{}",
                                    path.display(),
                                    scope,
                                    prev_name.as_deref().unwrap_or(&prev.session_id[..8]),
                                    prev.op.as_str(),
                                    prev.summary
                                        .as_ref()
                                        .map(|s| format!(": {}", s))
                                        .unwrap_or_default(),
                                    intent_suffix
                                );
                                let notification = ServerEvent::Notification {
                                    from_session: prev.session_id.clone(),
                                    from_name: prev_name,
                                    notification_type: NotificationType::FileConflict {
                                        path: path.display().to_string(),
                                        operation: prev.op.as_str().to_string(),
                                        intent: prev.intent.clone(),
                                        summary: prev.summary.clone(),
                                        detail: prev.detail.clone(),
                                    },
                                    message: alert_msg.clone(),
                                };
                                let _ = member.event_tx.send(notification);

                                if !queue_soft_interrupt_for_session(
                                    &session_id,
                                    alert_msg.clone(),
                                    false,
                                    SoftInterruptSource::System,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-activity soft interrupt for session {}",
                                        session_id
                                    ));
                                }
                            }
                        }

                        // Alert previous agents about the current modification.
                        for prev in &previous_touches {
                            if let Some(prev_member) = members.get(&prev.session_id) {
                                let scope = file_activity_scope_label(prev, &touch);
                                let intent_suffix = touch
                                    .intent
                                    .as_ref()
                                    .map(|intent| format!(" — intent: {}", intent))
                                    .unwrap_or_default();
                                let alert_msg = format!(
                                    "⚠ File activity: {} — {} — {} just {} this file you previously worked with{}{}",
                                    path.display(),
                                    scope,
                                    current_name
                                        .as_deref()
                                        .unwrap_or(&session_id[..8.min(session_id.len())]),
                                    touch.op.as_str(),
                                    touch
                                        .summary
                                        .as_ref()
                                        .map(|s| format!(": {}", s))
                                        .unwrap_or_default(),
                                    intent_suffix
                                );
                                let notification = ServerEvent::Notification {
                                    from_session: session_id.clone(),
                                    from_name: current_name.clone(),
                                    notification_type: NotificationType::FileConflict {
                                        path: path.display().to_string(),
                                        operation: touch.op.as_str().to_string(),
                                        intent: touch.intent.clone(),
                                        summary: touch.summary.clone(),
                                        detail: touch.detail.clone(),
                                    },
                                    message: alert_msg.clone(),
                                };
                                let _ = prev_member.event_tx.send(notification);

                                if !queue_soft_interrupt_for_session(
                                    &prev.session_id,
                                    alert_msg.clone(),
                                    false,
                                    SoftInterruptSource::System,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-activity soft interrupt for session {}",
                                        prev.session_id
                                    ));
                                }
                            }
                        }
                    }
                }
                Ok(BusEvent::BackgroundTaskCompleted(task)) => {
                    dispatch_background_task_completion(
                        &task,
                        &sessions,
                        &soft_interrupt_queues,
                        &swarm_members,
                        &swarms_by_id,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                    )
                    .await;
                }
                Ok(BusEvent::BackgroundTaskProgress(task)) => {
                    dispatch_background_task_progress(&task, &swarm_members).await;
                }
                Ok(BusEvent::SwarmAwaitCompleted(event)) => {
                    dispatch_swarm_await_completion(
                        &event,
                        &sessions,
                        &soft_interrupt_queues,
                        &swarm_members,
                        &swarms_by_id,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                    )
                    .await;
                }
                Ok(BusEvent::UiActivity(activity)) => {
                    dispatch_ui_activity(&activity, &swarm_members).await;
                }
                Ok(BusEvent::ToolUpdated(event)) => {
                    dispatch_swarm_tool_activity(&event, &swarm_members, &swarms_by_id).await;
                }
                Ok(BusEvent::SubagentStatus(event)) => {
                    dispatch_swarm_runtime_status(&event, &swarm_members, &swarms_by_id).await;
                }
                Ok(BusEvent::BatchProgress(progress)) => {
                    dispatch_swarm_batch_progress(&progress, &swarm_members, &swarms_by_id).await;
                }
                // Session todos are private to the session's transcript, but the
                // Compact todo names and progress are surfaced on the inline
                // swarm strip so a coordinator can see each managed agent's work.
                Ok(BusEvent::TodoUpdated(event)) => {
                    dispatch_swarm_todo_progress(&event, &swarm_members, &swarms_by_id).await;
                }
                Ok(BusEvent::SwarmOutputTail(tail)) => {
                    dispatch_swarm_output_tail(&tail, &swarm_members, &swarms_by_id).await;
                }
                Ok(_) => {
                    // Ignore other events
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    crate::logging::info(&format!("Bus monitor lagged by {} events", n));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    }

    /// Start the server (both main and debug sockets)
    pub async fn run(&self) -> Result<()> {
        // Ensure socket directory exists (for named sockets like /run/user/1000/jcode/)
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        #[cfg(unix)]
        let _daemon_lock = acquire_daemon_lock()?;

        if socket_has_live_listener(&self.socket_path).await {
            anyhow::bail!(
                "Refusing to replace active server socket at {}",
                self.socket_path.display()
            );
        }

        // Remove existing sockets (uses transport abstraction for cross-platform cleanup)
        crate::transport::remove_socket(&self.socket_path);
        crate::transport::remove_socket(&self.debug_socket_path);

        let main_listener = Listener::bind(&self.socket_path)?;
        let debug_listener = Listener::bind(&self.debug_socket_path)?;

        #[cfg(unix)]
        {
            // Server reload uses exec. Force the published listener fds to close
            // across exec so the replacement daemon can safely rebind them.
            mark_close_on_exec(&main_listener);
            mark_close_on_exec(&debug_listener);
        }

        // Preserve an in-flight reload marker for exec-based reloads owned by this
        // process, but clear stale markers from unrelated/stale processes.
        clear_reload_marker_if_stale_for_pid(std::process::id());

        match reload_recovery::collect_garbage() {
            Ok(stats) if stats.removed > 0 || stats.errors > 0 => {
                crate::logging::info(&format!(
                    "Reload recovery GC: removed={}, retained={}, errors={}",
                    stats.removed, stats.retained, stats.errors
                ));
            }
            Ok(_) => {}
            Err(error) => crate::logging::warn(&format!(
                "Reload recovery GC failed during startup: {error}"
            )),
        }

        // Restrict socket files to owner-only so other local users cannot connect.
        let _ = crate::platform::set_permissions_owner_only(&self.socket_path);
        let _ = crate::platform::set_permissions_owner_only(&self.debug_socket_path);

        // Set logging context for this server
        crate::logging::set_server(&self.identity.name);

        // Log server identity
        crate::logging::info(&format!(
            "Server {} starting ({})",
            self.identity.display_name(),
            self.identity.version
        ));
        crate::logging::info(&format!("Server listening on {:?}", self.socket_path));
        crate::logging::info(&format!("Debug socket on {:?}", self.debug_socket_path));

        let temporary_server_policy = lifecycle::temporary_server_policy_from_env();
        if let Some(policy) = temporary_server_policy.as_ref() {
            crate::logging::info(&format!(
                "Temporary server lifecycle enabled: owner_pid={:?}, idle_timeout_secs={}",
                policy.owner_pid, policy.idle_timeout_secs
            ));
            let _ = lifecycle::write_temporary_metadata(
                &self.socket_path,
                &self.debug_socket_path,
                policy,
            );
        }

        let server_start_time = Instant::now();

        self.spawn_background_tasks(server_start_time, temporary_server_policy);
        let (runtime, main_handle, debug_handle) = self
            .finish_startup_after_bind(main_listener, debug_listener, server_start_time)
            .await;

        // If either listener exits unexpectedly, stop accepting work and wait
        // for every owned connection task before returning. The normal daemon
        // path runs until process shutdown or exec-based reload.
        let mut main_handle = main_handle;
        let mut debug_handle = debug_handle;
        tokio::select! {
            result = &mut main_handle => {
                if let Err(error) = result {
                    crate::logging::error(&format!("Main accept loop failed: {error}"));
                }
                runtime.shutdown().await;
                let _ = debug_handle.await;
            }
            result = &mut debug_handle => {
                if let Err(error) = result {
                    crate::logging::error(&format!("Debug accept loop failed: {error}"));
                }
                runtime.shutdown().await;
                let _ = main_handle.await;
            }
        }
        Ok(())
    }

    /// Spawn the WebSocket gateway if enabled in config.
    /// The runtime task scope owns both the listener and client accept loop so
    /// server shutdown can cancel and join them with the other connection work.
    async fn spawn_gateway(&self, runtime: ServerRuntime) {
        let config = if let Some(override_config) = &self.gateway_config_override {
            override_config.clone()
        } else {
            let gw_config = &crate::config::config().gateway;
            crate::gateway::GatewayConfig {
                port: gw_config.port,
                bind_addr: gw_config.bind_addr.clone(),
                enabled: gw_config.enabled,
            }
        };

        if !config.enabled {
            return;
        }

        let (client_tx, client_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::gateway::GatewayClient>();

        let listener_runtime = runtime.clone();
        let listener_spawned = runtime
            .spawn_background_task(async move {
                if let Err(e) = crate::gateway::run_gateway(config, client_tx).await {
                    crate::logging::error(&format!("Gateway error: {}", e));
                }
            })
            .await;
        if listener_spawned {
            let _ = listener_runtime.spawn_gateway_accept_loop(client_rx).await;
        }
    }
}

pub use self::client_api::Client;

#[cfg(test)]
mod tests;
