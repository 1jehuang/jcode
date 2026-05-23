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
mod client_session;
mod client_state;
mod comm_await;
mod comm_control;
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
mod lifecycle;
mod provider_control;
mod reload;
mod reload_recovery;
mod reload_state;
mod runtime;
mod socket;
mod swarm;
mod swarm_channels;
mod swarm_mutation_state;
mod swarm_persistence;
pub mod lsp_event_bridge;
pub mod conflict_detector;
pub mod collab;
pub use lsp_event_bridge::{LspEventBridge, LspDiagnosticEvent, DiagnosticSummary};
pub use conflict_detector::{SymbolConflictDetector, ConflictReport, ConflictType};
mod util;

pub(super) use self::await_members_state::AwaitMembersRuntime;
use self::background_tasks::{
    dispatch_background_task_completion, dispatch_background_task_progress,
};
use self::debug::{ClientConnectionInfo, ClientDebugState};
use self::debug_jobs::DebugJob;
use self::headless::create_headless_session;
use self::reload::await_reload_signal;
use self::runtime::ServerRuntime;
use self::swarm::{
    broadcast_swarm_plan, broadcast_swarm_plan_with_previous, broadcast_swarm_status,
    record_swarm_event, record_swarm_event_for_session, refresh_swarm_task_staleness,
    remove_plan_participant, remove_session_file_touches, remove_session_from_swarm,
    rename_plan_participant, run_swarm_message, update_member_status,
    update_member_status_with_report,
};
use self::swarm_channels::{
    remove_session_channel_subscriptions, subscribe_session_to_channel,
    unsubscribe_session_from_channel,
};
pub(super) use self::swarm_mutation_state::SwarmMutationRuntime;
use self::swarm_persistence::{
    LoadedSwarmRuntimeState, load_runtime_state as load_persisted_swarm_runtime_state,
    persist_swarm_state as persist_swarm_state_snapshot,
    remove_swarm_state as remove_persisted_swarm_state,
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

pub(super) async fn persist_swarm_state_for(swarm_id: &str, swarm_state: &SwarmState) {
    let runtime = swarm_state.load_runtime(swarm_id).await;
    persist_swarm_state_snapshot(
        swarm_id,
        runtime.plan.as_ref(),
        runtime.coordinator_session_id.as_deref(),
        &runtime.members,
    );
}

pub(super) async fn remove_persisted_swarm_state_for(swarm_id: &str, swarm_state: &SwarmState) {
    let runtime = swarm_state.load_runtime(swarm_id).await;
    if runtime.has_any_state() {
        return;
    }
    remove_persisted_swarm_state(swarm_id);
}

fn headless_member_should_restore(status: &str, is_headless: bool) -> bool {
    is_headless && !matches!(status, "completed" | "done" | "failed" | "stopped")
}

fn headless_reload_continuation_message(reload_ctx: Option<ReloadContext>) -> Option<String> {
    ReloadContext::recovery_directive(reload_ctx.as_ref(), true, "", None)
        .map(|directive| directive.continuation_message)
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
    queue_soft_interrupt_for_session, register_session_event_sender,
    register_session_interrupt_queue, remove_session_interrupt_queue,
    rename_session_interrupt_queue, session_event_fanout_sender, unregister_session_event_sender,
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

pub(crate) use self::lifecycle::configure_temporary_server;
#[cfg(unix)]
pub use self::socket::spawn_server_notify;
#[cfg(unix)]
use self::socket::{acquire_daemon_lock, mark_close_on_exec};
pub use self::socket::{
    cleanup_socket_pair, connect_socket, debug_socket_path, has_live_listener, is_server_ready,
    set_socket_path, socket_path, wait_for_server_ready,
};
use self::socket::{signal_ready_fd, socket_has_live_listener};

pub use self::util::ServerIdentity;
use self::util::{
    debug_control_allowed, embedding_idle_unload_secs, git_common_dir_for, server_has_newer_binary,
    server_update_candidate, startup_headless_recovery_test_delay, swarm_id_for_dir,
};

mod file_activity;
use self::file_activity::file_activity_scope_label;

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
    /// Track file touches: path -> list of accesses
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    /// Reverse index for file touches: session_id -> touched paths
    files_touched_by_session: Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
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
    /// Real-time collaboration server for multi-user editing sessions.
    collab_server: Arc<collab::CollaborationServer>,
    /// Optional LSP server manager for language intelligence features.
    lsp_manager: Option<Arc<jcode_lsp::LspServerManager>>,
    /// Optional LSP event bridge that forwards diagnostics to Swarm channels.
    lsp_event_bridge: Option<Arc<LspEventBridge>>,
    /// Optional symbol conflict detector for Swarm task scheduling.
    conflict_detector: Option<Arc<SymbolConflictDetector>>,
    /// Backpressure controller to prevent overload cascading failures
    backpressure_controller: Arc<crate::backpressure::BackpressureController>,
    /// GPU load balancer scheduler (optional, activated when GPUs available)
    gpu_scheduler: Option<Arc<jcode_unified_scheduler::UnifiedScheduler>>,
}

mod server_impl;

pub use server_impl::Client;
