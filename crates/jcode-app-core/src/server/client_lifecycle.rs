use super::client_actions::{
    AgentTaskContext, NotifySessionContext, handle_agent_task, handle_compact, handle_input_shell,
    handle_notify_session, handle_rename_session, handle_run_subagent, handle_set_feature,
    handle_set_session_saved, handle_set_subagent_model, handle_split, handle_stdin_response,
    handle_transfer, handle_trigger_memory_extraction,
};
use super::client_comm::{
    handle_comm_channel_members, handle_comm_list, handle_comm_list_channels, handle_comm_message,
    handle_comm_read, handle_comm_share, handle_comm_subscribe_channel,
    handle_comm_unsubscribe_channel,
};
use super::client_comm_swarms::{handle_comm_list_swarms, handle_comm_set_swarm_label};
use super::client_disconnect_cleanup::{cleanup_client_connection, detach_client_attachment};
use super::client_lifecycle_logging::{
    ServerRequestLifecycleFields, interrupt_request_log_fields, request_payload_summary,
    request_type_from_line, request_type_is_read_only, server_request_lifecycle_fields,
};
use super::client_lightweight_control::{
    LightweightControlContext, handle_lightweight_control_request, parse_swarm_spawn_mode,
};
use super::client_session::{
    handle_clear_session, handle_reload, handle_resume_session, handle_subscribe,
};
use super::client_state::{
    handle_get_compacted_history, handle_get_history, handle_get_model_catalog, handle_get_state,
};
use super::client_writer::write_direct_event;
use super::comm_await::{CommAwaitMembersContext, handle_comm_await_members};
use super::comm_control::{
    handle_client_debug_command, handle_client_debug_response, handle_comm_assign_next,
    handle_comm_assign_role, handle_comm_assign_task, handle_comm_task_control,
};
use super::comm_plan::{
    handle_comm_approve_plan, handle_comm_propose_plan, handle_comm_reject_plan,
};
use super::comm_session::{handle_comm_spawn, handle_comm_stop};
use super::comm_sync::{
    CommResyncPlanContext, handle_comm_plan_status, handle_comm_read_context,
    handle_comm_resync_plan, handle_comm_status, handle_comm_summary,
};
use super::provider_control::{
    handle_cycle_model, handle_invalidate_anthropic_usage, handle_invalidate_openai_usage,
    handle_notify_auth_changed, handle_refresh_models, handle_set_compaction_mode,
    handle_set_context_cap, handle_set_model, handle_set_premium_mode, handle_set_reasoning_effort,
    handle_set_route, handle_set_service_tier, handle_set_transport,
    handle_switch_anthropic_account, handle_switch_openai_account,
    try_available_models_catalog_offloaded,
};
use super::{
    AwaitMembersRuntime, ClientConnectionInfo, ClientDebugState, FileTouchService,
    SessionControlHandle, SessionInterruptQueues, SharedContext, SwarmEvent, SwarmMember,
    SwarmMutationRuntime, VersionedPlan, format_structured_completion_report,
    register_session_interrupt_queue, send_swarm_plan_to_session, truncate_detail,
    update_member_status, update_member_status_with_report, update_member_status_with_report_tldr,
};
use crate::agent::Agent;
use crate::bus::{Bus, BusEvent};
use crate::id;
use crate::protocol::{Request, ServerEvent, decode_request, encode_event};
use crate::provider::Provider;
use crate::tool::Registry;
use crate::transport::Stream;
use anyhow::Result;
use futures::FutureExt;
use jcode_agent_runtime::{InterruptSignal, SoftInterruptSource, StreamError};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

use super::SessionAgents;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;
const RELOAD_STARTING_GUARD_MAX_AGE: Duration = Duration::from_secs(30);
const REQUEST_HANDLER_STALL_THRESHOLDS_MS: [u64; 3] = [2_000, 10_000, 60_000];

#[path = "client_subscribe_context.rs"]
mod subscribe_context;
use subscribe_context::*;

fn event_for_transport(
    mut event: ServerEvent,
    supports_turn_identity: bool,
) -> Option<ServerEvent> {
    if supports_turn_identity {
        return Some(event);
    }
    loop {
        event = match event {
            ServerEvent::TurnEvent { event, .. } => *event,
            ServerEvent::TurnStarted { .. } | ServerEvent::TurnState { .. } => return None,
            event => return Some(event),
        };
    }
}

struct ProcessingMessage {
    id: u64,
    content: String,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    active_skill: Option<String>,
    skill_arguments: Option<String>,
    /// Retry on the next pool account after a usage limit (API clients).
    server_failover: bool,
}

// A client request ID is not a turn identity: clients can reuse it after cancel.
type ProcessingCompletion = (
    u64,
    Result<()>,
    Option<String>,
    tokio::task::Id,
    Option<crate::agent::TurnPublicationPin>,
);

struct ProcessingState<'a> {
    client_is_processing: &'a mut bool,
    message_id: &'a mut Option<u64>,
    session_id: &'a mut Option<String>,
    task: &'a mut Option<tokio::task::JoinHandle<()>>,
}

struct SwarmStatusRefs<'a> {
    members: &'a Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &'a Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &'a Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &'a Arc<std::sync::atomic::AtomicU64>,
    event_tx: &'a broadcast::Sender<SwarmEvent>,
}

#[path = "request_watchdog.rs"]
mod request_watchdog;
use request_watchdog::{
    RequestHandlerWatchdog, RequestHandlerWatchdogContext, log_request_lifecycle_handled,
    reject_if_agent_busy_for_request, send_agent_busy_error, should_start_idle_soft_interrupt,
};
fn server_reload_starting() -> bool {
    matches!(
        crate::server::recent_reload_state(RELOAD_STARTING_GUARD_MAX_AGE),
        Some(state) if state.phase == crate::server::ReloadPhase::Starting
    )
}

fn compaction_server_event(event: crate::compaction::CompactionEvent) -> ServerEvent {
    ServerEvent::Compaction {
        trigger: event.trigger,
        pre_tokens: event.pre_tokens,
        post_tokens: event.post_tokens,
        tokens_saved: event.tokens_saved,
        duration_ms: event.duration_ms,
        messages_dropped: event.messages_dropped,
        messages_compacted: event.messages_compacted,
        summary_chars: event.summary_chars,
        active_messages: event.active_messages,
    }
}

async fn poll_agent_compaction_completion(agent: Arc<Mutex<Agent>>) -> Option<ServerEvent> {
    let mut agent_guard = agent.lock().await;
    agent_guard
        .poll_compaction_completion_event()
        .map(compaction_server_event)
}

async fn refresh_session_control_handle(
    session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
) -> SessionControlHandle {
    let started = Instant::now();
    let agent_guard = match agent.try_lock() {
        Ok(agent_guard) => agent_guard,
        Err(_) => {
            crate::logging::warn(&format!(
                "refresh_session_control_handle: waiting for busy agent lock for session {}; cancel/control requests on this connection may be delayed",
                session_id
            ));
            let fallback_stop_signal = shutdown_signals.read().await.get(session_id).cloned();
            let fallback_soft_interrupt_queue =
                soft_interrupt_queues.read().await.get(session_id).cloned();
            if let Some(soft_interrupt_queue) = fallback_soft_interrupt_queue {
                // A missing shutdown-signal registration (e.g. a session created
                // through the headless spawn path) must not force this connection
                // to block on the busy agent mutex: cancels fired through the
                // handle fan out to the running turn's own signal via the
                // turn-cancel registry (issue #428), so a detached signal is a
                // safe stand-in.
                let stop_signal = fallback_stop_signal.unwrap_or_else(|| {
                    crate::logging::warn(&format!(
                        "refresh_session_control_handle: no registered shutdown signal for busy session {}; using detached signal (cancels reach the running turn via the turn cancel registry)",
                        session_id
                    ));
                    InterruptSignal::new()
                });
                crate::logging::warn(&format!(
                    "refresh_session_control_handle: using lock-free cancel-only control handle for busy session {} after {}ms",
                    session_id,
                    started.elapsed().as_millis()
                ));
                return SessionControlHandle::cancel_only(
                    session_id,
                    soft_interrupt_queue,
                    stop_signal,
                );
            }
            let agent_guard = agent.lock().await;
            crate::logging::warn(&format!(
                "refresh_session_control_handle: acquired agent lock for session {} after {}ms",
                session_id,
                started.elapsed().as_millis()
            ));
            agent_guard
        }
    };
    SessionControlHandle::new(
        session_id,
        agent_guard.soft_interrupt_queue(),
        agent_guard.background_tool_signal(),
        agent_guard.graceful_shutdown_signal(),
    )
    .with_provider(agent_guard.provider_handle())
}

#[expect(
    clippy::too_many_arguments,
    reason = "client lifecycle wiring spans sessions, swarm state, file state, channels, debug, and runtime coordination"
)]
pub(super) async fn handle_client(
    stream: Stream,
    sessions: SessionAgents,
    _global_event_tx: broadcast::Sender<ServerEvent>,
    provider_template: Arc<dyn Provider>,
    _global_is_processing: Arc<RwLock<bool>>,
    global_session_id: Arc<RwLock<String>>,
    client_count: Arc<RwLock<usize>>,
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    file_touch: FileTouchService,
    channel_subscriptions: ChannelSubscriptions,
    channel_subscriptions_by_session: ChannelSubscriptions,
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    server_name: String,
    server_icon: String,
    mcp_pool: Arc<crate::mcp::SharedMcpPool>,
    shutdown_signals: Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: SessionInterruptQueues,
    await_members_runtime: AwaitMembersRuntime,
    swarm_mutation_runtime: SwarmMutationRuntime,
) -> Result<()> {
    let handler_start = Instant::now();
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let mut line = String::new();

    let mut initial_request = loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(error) => {
                crate::logging::error(&format!(
                    "Client read error before initialization: {}",
                    error
                ));
                return Ok(());
            }
        };
        if n == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }

        match decode_request(&line) {
            Ok(request) => {
                if !request.is_lightweight_control_request() {
                    crate::logging::info(&format!(
                        "[TIMING] handle_client initial request read: bytes={}, handler_to_read={}ms",
                        line.len(),
                        handler_start.elapsed().as_millis()
                    ));
                }
                if request.is_lightweight_control_request() {
                    let keep_connection_open = matches!(request, Request::Ping { .. });
                    handle_lightweight_control_request(
                        request,
                        Arc::clone(&writer),
                        LightweightControlContext {
                            sessions: &sessions,
                            global_session_id: &global_session_id,
                            provider_template: &provider_template,
                            swarm_members: &swarm_members,
                            swarms_by_id: &swarms_by_id,
                            shared_context: &shared_context,
                            swarm_plans: &swarm_plans,
                            swarm_coordinators: &swarm_coordinators,
                            file_touch: &file_touch,
                            channel_subscriptions: &channel_subscriptions,
                            channel_subscriptions_by_session: &channel_subscriptions_by_session,
                            client_connections: &client_connections,
                            event_history: &event_history,
                            event_counter: &event_counter,
                            swarm_event_tx: &swarm_event_tx,
                            mcp_pool: &mcp_pool,
                            soft_interrupt_queues: &soft_interrupt_queues,
                            await_members_runtime: &await_members_runtime,
                            swarm_mutation_runtime: &swarm_mutation_runtime,
                        },
                    )
                    .await?;
                    // Native SSH probes daemon capability before sending its
                    // Subscribe on this same stream. Ping must not consume the
                    // connection, unlike the other one-shot control requests.
                    if keep_connection_open {
                        continue;
                    }
                    return Ok(());
                }
                break request;
            }
            Err(error) => {
                write_direct_event(
                    &writer,
                    &ServerEvent::Error {
                        id: 0,
                        message: format!("Invalid request: {}", error),
                        retry_after_secs: None,
                    },
                )
                .await?;
            }
        }
    };

    let supports_turn_identity = matches!(
        &initial_request,
        Request::Subscribe {
            supports_turn_identity: true,
            ..
        }
    );
    let mut initialization_activity = Some(crate::runtime_activity::global().try_enter()?);
    let initial_working_dir =
        match resolve_target_subscribe_working_dir(&mut initial_request, &sessions, &swarm_members)
            .await
            .and_then(|()| initial_subscribe_working_dir(&initial_request))
        {
            Ok(working_dir) => working_dir,
            Err(message) => {
                write_direct_event(
                    &writer,
                    &ServerEvent::Error {
                        id: initial_request.id(),
                        message,
                        retry_after_secs: None,
                    },
                )
                .await?;
                return Ok(());
            }
        };
    let mut active_terminal_env = initial_subscribe_terminal_env(&initial_request);

    // Per-client state
    let mut client_is_processing = false;
    let (processing_done_tx, mut processing_done_rx) =
        mpsc::unbounded_channel::<ProcessingCompletion>();
    let mut processing_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut processing_message_id: Option<u64> = None;
    let mut processing_session_id: Option<String> = None;
    let mut current_client_instance_id: Option<String> = None;
    let mut continue_on_disconnect = false;
    let mut model_usage_updates_enabled = false;
    let mut supports_pdf_panels = false;
    let mut supports_user_response = false;
    let mut supports_server_failover = false;
    // Client selfdev status is determined by Subscribe request, not server's env
    let mut client_selfdev = false;

    let client_start = std::time::Instant::now();

    // A target-aware Subscribe for an existing session restores that
    // session's own model onto this placeholder. Choosing a fresh pool route
    // for it is wasted work, and in a reload storm the forks serialize on that
    // choice (seconds when an account's quota state changes mid-storm).
    let resumes_persisted_session = match &initial_request {
        Request::Subscribe {
            target_session_id: Some(target),
            ..
        } => crate::session::session_exists(target),
        _ => false,
    };
    let provider = if resumes_persisted_session {
        provider_template.fork()
    } else {
        provider_template.fork_for_new_session()
    };
    let provider_fork_ms = client_start.elapsed().as_millis();
    let t0 = std::time::Instant::now();
    let registry = Registry::new(provider.clone()).await;
    let registry_ms = t0.elapsed().as_millis();

    let mut swarm_enabled = crate::config::config().features.swarm;
    let mut last_available_models_snapshot: Option<Arc<str>> = None;

    // Create a new session for this client
    let t0 = std::time::Instant::now();
    let mut new_agent =
        crate::hooks::with_client_terminal_env(active_terminal_env.clone(), async {
            super::run_blocking_io(|| {
                Agent::new_provisional_with_initial_working_dir(
                    Arc::clone(&provider),
                    registry.clone(),
                    Some(&initial_working_dir),
                )
            })
        })
        .await;
    let agent_new_ms = t0.elapsed().as_millis();

    new_agent.set_memory_enabled(crate::config::config().features.memory);
    let prewarm_start = std::time::Instant::now();
    // A target-aware Subscribe replaces this placeholder with the restored
    // session right away, so warming its provider is wasted work on the
    // resume critical path (p50 ~200ms per reconnect in a reload storm).
    let resuming_existing_session = matches!(
        &initial_request,
        Request::Subscribe {
            target_session_id: Some(_),
            ..
        }
    );
    if !resuming_existing_session {
        new_agent.prewarm_provider_idle().await;
    }
    let prewarm_ms = prewarm_start.elapsed().as_millis();

    crate::logging::info(&format!(
        "[TIMING] handle_client setup: provider_fork={provider_fork_ms}ms, registry={registry_ms}ms, agent_new={agent_new_ms}ms, prewarm={prewarm_ms}ms, total={}ms",
        client_start.elapsed().as_millis()
    ));
    let mut client_session_id = new_agent.session_id().to_string();
    let friendly_name = new_agent.session_short_name().map(|s| s.to_string());
    let client_connection_id = id::new_id("conn");
    let connected_at = Instant::now();
    let (disconnect_tx, mut disconnect_rx) = mpsc::unbounded_channel::<()>();

    {
        let mut connections = client_connections.write().await;
        connections.insert(
            client_connection_id.clone(),
            ClientConnectionInfo {
                client_id: client_connection_id.clone(),
                session_id: client_session_id.clone(),
                client_instance_id: None,
                debug_client_id: None,
                connected_at,
                last_seen: connected_at,
                is_processing: false,
                current_tool_name: None,
                terminal_env: active_terminal_env.clone(),
                disconnect_tx: disconnect_tx.clone(),
            },
        );
    }

    {
        let mut current = global_session_id.write().await;
        if current.is_empty() || *current != client_session_id {
            *current = client_session_id.clone();
        }
    }

    // Get lock-free control-plane handles BEFORE wrapping in Mutex.
    // This allows cancel/soft-interrupt/background-tool requests while the agent is processing.
    let mut session_control = SessionControlHandle::new(
        client_session_id.clone(),
        new_agent.soft_interrupt_queue(),
        new_agent.background_tool_signal(),
        new_agent.graceful_shutdown_signal(),
    )
    .with_provider(new_agent.provider_handle());

    // Register the shutdown signal in the server-level map so
    // graceful_shutdown_sessions can signal it without locking the agent mutex
    {
        let mut signals = shutdown_signals.write().await;
        signals.insert(
            client_session_id.clone(),
            session_control.stop_current_turn_signal(),
        );
    }
    register_session_interrupt_queue(
        &soft_interrupt_queues,
        &client_session_id,
        new_agent.soft_interrupt_queue(),
    )
    .await;

    let mut agent = Arc::new(Mutex::new(new_agent));
    sessions
        .register_new(client_session_id.clone(), Arc::clone(&agent))
        .await?;
    crate::runtime_memory_log::emit_event(
        crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
            "session_created",
            "new_live_session_attached",
        )
        .with_session_id(client_session_id.clone())
        .force_attribution(),
    );

    // Per-client event channel (not shared with other clients)
    let _sdk_connection_guard = crate::tool::sdk::ConnectionGuard(client_connection_id.clone());
    let (client_event_tx, mut client_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ServerEvent>();

    // Spawn event forwarder for this client only
    let writer_clone = Arc::clone(&writer);
    let client_connection_id_for_events = client_connection_id.clone();
    let client_connections_for_events = Arc::clone(&client_connections);
    let event_handle = tokio::spawn(async move {
        while let Some(event) = client_event_rx.recv().await {
            {
                let mut connections = client_connections_for_events.write().await;
                if let Some(info) = connections.get_mut(&client_connection_id_for_events) {
                    let observed = match &event {
                        ServerEvent::TurnEvent { event, .. } => event.as_ref(),
                        event => event,
                    };
                    match observed {
                        ServerEvent::TurnStarted { .. } => {
                            info.is_processing = true;
                            info.current_tool_name = None;
                        }
                        ServerEvent::ToolStart { name, .. } => {
                            info.is_processing = true;
                            info.current_tool_name = Some(name.clone());
                        }
                        ServerEvent::ToolDone { .. } => {
                            info.current_tool_name = None;
                        }
                        ServerEvent::Done { .. }
                        | ServerEvent::Error { .. }
                        | ServerEvent::Interrupted => {
                            info.is_processing = false;
                            info.current_tool_name = None;
                        }
                        _ => {}
                    }
                }
            }
            let Some(event) = event_for_transport(event, supports_turn_identity) else {
                continue;
            };
            let json = encode_event(&event);
            let mut w = writer_clone.lock().await;
            if let Err(error) = w.write_all(json.as_bytes()).await {
                // A broken pipe here is routine (client reload/disconnect mid
                // broadcast), so keep the line short: a full Debug dump of e.g.
                // a SwarmStatus event prints every member and floods the log.
                let event_desc = crate::logging::truncate_for_log(&format!("{:?}", event), 200);
                crate::logging::warn(&format!(
                    "event_forwarder write failed for connection {} while sending {}: {}",
                    client_connection_id_for_events, event_desc, error
                ));
                break;
            }
        }
    });

    // Note: Don't send initial SessionId here - it's sent by the Subscribe handler
    // Sending it via the channel causes race conditions where it can arrive after
    // other events (like History) that are written directly to the socket.

    // Set up client debug command channel
    // This client becomes the "active" debug client that receives client: commands
    let (debug_cmd_tx, mut debug_cmd_rx) = mpsc::unbounded_channel::<(u64, String)>();
    let client_debug_id = id::new_id("client");
    {
        let mut debug_state = client_debug_state.write().await;
        debug_state.register(client_debug_id.clone(), debug_cmd_tx);
    }
    {
        let mut connections = client_connections.write().await;
        if let Some(info) = connections.get_mut(&client_connection_id) {
            info.debug_client_id = Some(client_debug_id.clone());
        }
    }

    let stdin_responses: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Subscribe to bus events so we can forward ModelsUpdated to this client
    // (e.g. when Copilot finishes async init after the initial History was sent)
    let mut bus_rx = Bus::global().subscribe();

    // Set up stdin request forwarding: tools send StdinInputRequest, we forward to TUI
    let (stdin_req_tx, mut stdin_req_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::tool::StdinInputRequest>();
    {
        let mut agent_guard = agent.lock().await;
        agent_guard.set_stdin_request_tx(stdin_req_tx.clone());
    }
    let stdin_forwarder = {
        let client_event_tx = client_event_tx.clone();
        let stdin_responses = stdin_responses.clone();
        let tool_call_id = String::new();
        tokio::spawn(async move {
            while let Some(req) = stdin_req_rx.recv().await {
                let request_id = req.request_id.clone();
                stdin_responses
                    .lock()
                    .await
                    .insert(request_id.clone(), req.response_tx);
                let _ = client_event_tx.send(ServerEvent::StdinRequest {
                    request_id,
                    prompt: req.prompt,
                    is_password: req.is_password,
                    tool_call_id: tool_call_id.clone(),
                });
            }
        })
    };

    // Do not drain global bus traffic until the client has completed its first
    // subscribe. Under heavy swarm file-activity load, ignored bus frames can
    // otherwise monopolize the select loop before the initial subscribe/read.
    let mut client_subscribed = false;
    let mut provisional_session = true;
    let mut pending_request = Some(initial_request);

    let connection_result: Result<()> = async {
    loop {
        let mut request = if let Some(request) = pending_request.take() {
            request
        } else {
            line.clear();
            tokio::select! {
            biased;
            // Prioritize direct client I/O so subscribe/ping/message requests do not get
            // starved behind noisy background bus traffic.
            n = reader.read_line(&mut line) => {
                let n = match n {
                    Ok(n) => n,
                    Err(e) => {
                        crate::logging::error(&format!("Client read error: {}", e));
                        break;
                    }
                };
                if n == 0 {
                    break; // Client disconnected
                }
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(&client_connection_id) {
                    info.last_seen = Instant::now();
                }
            }
            done = processing_done_rx.recv() => {
                if let Some(done) = done {
                    reconcile_processing_completions(
                        Some(done), &mut processing_done_rx,
                        &mut ProcessingState {
                            client_is_processing: &mut client_is_processing,
                            message_id: &mut processing_message_id,
                            session_id: &mut processing_session_id,
                            task: &mut processing_task,
                        },
                        &client_connection_id, &client_connections,
                        &SwarmStatusRefs {
                            members: &swarm_members,
                            swarms_by_id: &swarms_by_id,
                            event_history: &event_history,
                            event_counter: &event_counter,
                            event_tx: &swarm_event_tx,
                        },
                    ).await;
                } else {
                    break;
                }
                continue;
            }
            disconnect_signal = disconnect_rx.recv() => {
                if disconnect_signal.is_some() {
                    crate::logging::info(&format!(
                        "Client connection {} was superseded; disconnecting old owner of session {}",
                        client_connection_id, client_session_id
                    ));
                    break;
                }
                continue;
            }
            // Forward bus events to this client
            bus_event = bus_rx.recv(), if client_subscribed => {
                match bus_event {
                    Ok(BusEvent::ModelUsageUpdated(route)) => {
                        if model_usage_updates_enabled {
                            let _ = client_event_tx.send(ServerEvent::ModelUsageUpdated { route });
                        }
                    }
                    Ok(BusEvent::ModelsUpdated) => {
                        let Some(catalog) = try_available_models_catalog_offloaded(&agent).await else {
                            continue;
                        };
                        // Equivalent clients share the snapshot, normalized key,
                        // and size/downgrade decision. Pointer equality makes an
                        // unchanged shared catalog a constant-time fast path.
                        if last_available_models_snapshot.as_ref().is_some_and(|previous| {
                            Arc::ptr_eq(previous, &catalog.dedup_key)
                                || previous.as_ref() == catalog.dedup_key.as_ref()
                        }) {
                            continue;
                        }
                        if let Some(event) = catalog.live_event() {
                            let _ = client_event_tx.send(event.clone());
                        } else {
                            crate::logging::warn(&format!(
                                "Skipping oversized catalog for connection {} ({} bytes)",
                                client_connection_id, catalog.encoded_len
                            ));
                        }
                        last_available_models_snapshot = Some(Arc::clone(&catalog.dedup_key));
                    }
                    Ok(BusEvent::BatchProgress(progress)) => {
                        if progress.session_id == client_session_id {
                            let _ = client_event_tx.send(ServerEvent::BatchProgress { progress });
                        }
                    }
                    Ok(BusEvent::SidePanelUpdated(update)) => {
                        if update.session_id == client_session_id {
                            let _ = client_event_tx.send(ServerEvent::SidePanelState {
                                snapshot: super::client_writer::side_panel_for_client(
                                    update.snapshot, supports_pdf_panels,
                                ),
                            });
                        }
                    }
                    Ok(BusEvent::AppletsUpdated(update)) => {
                        if update.session_id == client_session_id {
                            let _ = client_event_tx.send(ServerEvent::AppletState {
                                session_id: update.session_id,
                                snapshot: update.snapshot,
                            });
                        }
                    }
                    Ok(BusEvent::CompactionFinished) => {
                        let agent = Arc::clone(&agent);
                        let tx = client_event_tx.clone();
                        tokio::spawn(async move {
                            if let Some(event) = poll_agent_compaction_completion(agent).await {
                                let _ = tx.send(event);
                            }
                        });
                    }
                    _ => {}
                }
                continue;
            }
            // Handle client debug commands from debug socket
            debug_cmd = debug_cmd_rx.recv() => {
                if let Some((request_id, command)) = debug_cmd
                    && client_event_tx
                        .send(ServerEvent::ClientDebugRequest {
                            id: request_id,
                            command,
                        })
                        .is_err()
                {
                    let _ = client_debug_response_tx.send((
                        request_id,
                        "No TUI client connected".to_string(),
                    ));
                }
                continue;
            }
            }

            match decode_request(&line) {
                Ok(r) => r,
                Err(e) => {
                    let event = ServerEvent::Error {
                        id: 0,
                        message: format!("Invalid request: {}", e),
                        retry_after_secs: None,
                    };
                    let json = encode_event(&event);
                    let mut w = writer.lock().await;
                    if w.write_all(json.as_bytes()).await.is_err() {
                        break;
                    }
                    continue;
                }
            }
        };
        let request_decoded_at = Instant::now();
        // Direct input wins the biased select above. A client may already have
        // observed Done/Error while this actor still holds the finished turn's
        // state. Completion is enqueued before the terminal event, so drain it
        // before any request (especially Message/SoftInterrupt) inspects that
        // state. Keep streaming terminal events on their ordered fanout channel.
        reconcile_processing_completions(
            None, &mut processing_done_rx,
            &mut ProcessingState {
                client_is_processing: &mut client_is_processing,
                message_id: &mut processing_message_id,
                session_id: &mut processing_session_id,
                task: &mut processing_task,
            },
            &client_connection_id, &client_connections,
            &SwarmStatusRefs {
                members: &swarm_members,
                swarms_by_id: &swarms_by_id,
                event_history: &event_history,
                event_counter: &event_counter,
                event_tx: &swarm_event_tx,
            },
        ).await;
        // Reserve before Ack or any session mutation, not just when the turn
        // eventually polls. Upgrade requests themselves must not reserve work.
        let _request_activity = if matches!(&request, Request::UpgradeRuntime { .. }) {
            None
        } else {
            match crate::runtime_activity::global().try_enter() {
                Ok(activity) => Some(activity),
                Err(error) => {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id: request.id(), message: error.to_string(), retry_after_secs: None,
                    });
                    continue;
                }
            }
        };
        // The first request now owns its admission reservation. Setup can stop
        // holding its own without leaving a registration-to-dispatch gap.
        drop(initialization_activity.take());
        let request_id = request.id();
        let request_kind = request_type_from_line(&line);
        let request_lifecycle_logged = !request_type_is_read_only(&request_kind);
        let request_lifecycle_start = Instant::now();
        let _request_watchdog = RequestHandlerWatchdog::spawn(RequestHandlerWatchdogContext {
            request_id,
            request_kind: request_kind.clone(),
            client_session_id: client_session_id.clone(),
            client_connection_id: client_connection_id.clone(),
            client_instance_id: current_client_instance_id.clone(),
            client_is_processing,
            message_id: processing_message_id,
            processing_session_id: processing_session_id.clone(),
            line_bytes: line.len(),
            lifecycle_logged: request_lifecycle_logged,
        });
        if request_lifecycle_logged {
            let mut fields = server_request_lifecycle_fields(ServerRequestLifecycleFields {
                phase: "received",
                request_id,
                request_kind: &request_kind,
                client_session_id: &client_session_id,
                client_connection_id: &client_connection_id,
                client_instance_id: current_client_instance_id.as_deref(),
                client_is_processing,
                message_id: processing_message_id,
                processing_session_id: processing_session_id.as_deref(),
                line_bytes: line.len(),
            });
            fields.extend(request_payload_summary(&request_kind, &line));
            crate::logging::event_info("SERVER_REQUEST_LIFECYCLE", fields);
        }
        if let Some(fields) = interrupt_request_log_fields(
            &request,
            &client_session_id,
            client_is_processing,
            processing_message_id,
            processing_task.is_some(),
            line.len(),
        ) {
            crate::logging::info(&format!("SERVER_INTERRUPT_REQUEST_DECODED {}", fields));
        }

        // A cancellation request must never be gated on writing an Ack to the client.
        // The normal Ack path takes the shared outbound writer before dispatching the
        // request. During heavy streaming, history replay, or client-side backpressure,
        // that writer can be busy long enough that an already-decoded cancel would sit
        // behind outbound bytes instead of signalling the agent's lock-free cancel
        // handle. Queue the Ack through the event channel and signal cancellation first.
        if let Request::Cancel { id } = request {
            let ack_queued = client_event_tx.send(ServerEvent::Ack { id }).is_ok();
            crate::logging::info(&format!(
                "SERVER_INTERRUPT_CANCEL_PRE_ACK_DISPATCH id={} session={} ack_queued={} decoded_to_dispatch_ms={}",
                id,
                client_session_id,
                ack_queued,
                request_decoded_at.elapsed().as_millis()
            ));
            let cancel_dispatch_start = Instant::now();
            cancel_processing_message(
                &mut ProcessingState {
                    client_is_processing: &mut client_is_processing,
                    message_id: &mut processing_message_id,
                    session_id: &mut processing_session_id,
                    task: &mut processing_task,
                },
                &session_control,
                &client_event_tx,
                &SwarmStatusRefs {
                    members: &swarm_members,
                    swarms_by_id: &swarms_by_id,
                    event_history: &event_history,
                    event_counter: &event_counter,
                    event_tx: &swarm_event_tx,
                },
                Some(id),
                Some(request_decoded_at),
                Some(&sessions),
            )
            .await;
            crate::logging::info(&format!(
                "SERVER_INTERRUPT_CANCEL_PRE_ACK_DONE id={} session={} dispatch_ms={} total_since_decode_ms={}",
                id,
                client_session_id,
                cancel_dispatch_start.elapsed().as_millis(),
                request_decoded_at.elapsed().as_millis()
            ));
            if !client_is_processing {
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(&client_connection_id) {
                    info.is_processing = false;
                    info.current_tool_name = None;
                }
            }
            if request_lifecycle_logged {
                log_request_lifecycle_handled(
                    ServerRequestLifecycleFields {
                        phase: "handled",
                        request_id,
                        request_kind: &request_kind,
                        client_session_id: &client_session_id,
                        client_connection_id: &client_connection_id,
                        client_instance_id: current_client_instance_id.as_deref(),
                        client_is_processing,
                        message_id: processing_message_id,
                        processing_session_id: processing_session_id.as_deref(),
                        line_bytes: line.len(),
                    },
                    request_lifecycle_start,
                    request_decoded_at,
                );
            }
            continue;
        }

        // SDK controls reply only after validation. Callback results must never
        // wait for the Agent mutex held by the turn awaiting that callback.
        if matches!(&request, Request::ConfigureTools { .. } | Request::ListTools { .. } | Request::ToolResult { .. }) {
            let id = request.id();
            let response: anyhow::Result<ServerEvent> = match request {
                Request::ToolResult { call_id, output, error, .. } => {
                    crate::tool::sdk::complete(&client_connection_id, &client_session_id, &call_id, output, error)
                        .map(|()| ServerEvent::Ack { id })
                }
                Request::ConfigureTools { tools, .. } => {
                    if client_is_processing || crate::turn_cancel_registry::has_active_turn(&client_session_id) { Err(anyhow::anyhow!("Session is busy")) }
                    else if let Ok(mut locked) = agent.try_lock() {
                        let result = crate::tool::sdk::configure(locked.session_id(), &client_connection_id, tools, client_event_tx.clone());
                        if result.is_ok() {
                            locked.invalidate_sdk_tools();
                            // Queue the acknowledgment before releasing the session lock,
                            // so a second client cannot start a new-policy call first.
                            let _ = client_event_tx.send(ServerEvent::Ack { id });
                            continue;
                        }
                        result.map(|()| ServerEvent::Ack { id })
                    } else { Err(anyhow::anyhow!("Session is busy")) }
                }
                Request::ListTools { .. } => {
                    if let Ok(locked) = agent.try_lock() {
                        Ok(ServerEvent::Tools { id, tools: crate::tool::sdk::wire_definitions(locked.tool_definitions_for_debug().await) })
                    } else { Err(anyhow::anyhow!("Session is busy")) }
                }
                _ => unreachable!(),
            };
            let event = response.unwrap_or_else(|error| ServerEvent::Error { id, message: error.to_string(), retry_after_secs: None });
            if let Err(error) = client_event_tx.send(event) {
                crate::logging::debug(&format!("Client disconnected before event delivery: {error}"));
            }
            continue;
        }

        // Send ack
        let ack = ServerEvent::Ack { id: request.id() };
        let json = encode_event(&ack);
        {
            let ack_start = Instant::now();
            let mut w = writer.lock().await;
            if w.write_all(json.as_bytes()).await.is_err() {
                if request_lifecycle_logged {
                    let mut fields =
                        server_request_lifecycle_fields(ServerRequestLifecycleFields {
                            phase: "ack_write_failed",
                            request_id,
                            request_kind: &request_kind,
                            client_session_id: &client_session_id,
                            client_connection_id: &client_connection_id,
                            client_instance_id: current_client_instance_id.as_deref(),
                            client_is_processing,
                            message_id: processing_message_id,
                            processing_session_id: processing_session_id.as_deref(),
                            line_bytes: line.len(),
                        });
                    fields.push((
                        "ack_write_ms".to_string(),
                        ack_start.elapsed().as_millis().to_string(),
                    ));
                    crate::logging::event_warn("SERVER_REQUEST_LIFECYCLE", fields);
                }
                break;
            }
            if request_lifecycle_logged {
                let mut fields = server_request_lifecycle_fields(ServerRequestLifecycleFields {
                    phase: "acked",
                    request_id,
                    request_kind: &request_kind,
                    client_session_id: &client_session_id,
                    client_connection_id: &client_connection_id,
                    client_instance_id: current_client_instance_id.as_deref(),
                    client_is_processing,
                    message_id: processing_message_id,
                    processing_session_id: processing_session_id.as_deref(),
                    line_bytes: line.len(),
                });
                fields.push((
                    "ack_write_ms".to_string(),
                    ack_start.elapsed().as_millis().to_string(),
                ));
                fields.push((
                    "since_decode_ms".to_string(),
                    request_decoded_at.elapsed().as_millis().to_string(),
                ));
                crate::logging::event_info("SERVER_REQUEST_LIFECYCLE", fields);
            }
        }

        // Legacy/direct clients can send a prompt without Subscribe. Their
        // first session action commits ownership, but inspection/attach does not.
        if provisional_session
            && matches!(
                &request,
                Request::Message { .. }
                    | Request::SoftInterrupt { .. }
                    | Request::RunSubagent { .. }
            )
        {
            agent.lock().await.activate_concurrency_tracking();
            provisional_session = false;
        }

        if let Err(message) = resolve_target_subscribe_working_dir(
            &mut request, &sessions, &swarm_members,
        ).await {
            let _ = client_event_tx.send(ServerEvent::Error {
                id: request.id(), message, retry_after_secs: None,
            });
            continue;
        }
        let sdk_session_before_request = client_session_id.clone();
        match request {
            Request::ConfigureTools { .. } | Request::ListTools { .. } | Request::ToolResult { .. } => unreachable!("SDK controls dispatched before acknowledgment"),
            Request::Message {
                id,
                content,
                images,
                system_reminder,
                active_skill,
                skill_arguments,
                no_reply,
            } => {
                if no_reply {
                    append_context_message(
                        id,
                        &content,
                        images,
                        &client_session_id,
                        client_is_processing,
                        &agent,
                        &client_event_tx,
                    )
                    .await;
                    continue;
                }
                if !client_is_processing {
                    // A live resume cannot replace stdin routing while the old
                    // turn owns the agent. Restore it when this client starts a
                    // later turn, without reviving any disconnected prompt.
                    if continue_on_disconnect && let Ok(mut agent) = agent.try_lock() {
                        agent.set_stdin_request_tx(stdin_req_tx.clone());
                    }
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.is_processing = true;
                        info.current_tool_name = None;
                    }
                }
                let peer_content = content.clone();
                start_processing_message(
                    ProcessingMessage {
                        id,
                        content,
                        images,
                        system_reminder,
                        active_skill,
                        skill_arguments,
                        server_failover: supports_server_failover,
                    },
                    &client_session_id,
                    &mut ProcessingState {
                        client_is_processing: &mut client_is_processing,
                        message_id: &mut processing_message_id,
                        session_id: &mut processing_session_id,
                        task: &mut processing_task,
                    },
                    &agent,
                    &client_event_tx,
                    &processing_done_tx,
                    active_terminal_env.clone(),
                    &SwarmStatusRefs {
                        members: &swarm_members,
                        swarms_by_id: &swarms_by_id,
                        event_history: &event_history,
                        event_counter: &event_counter,
                        event_tx: &swarm_event_tx,
                    },
                )
                .await;
                // Other windows attached to this session only see the turn's
                // stream events, never the message that started it. Tell them
                // so the prompt (or auto-poke notice) shows up everywhere.
                if processing_message_id == Some(id) && !peer_content.trim().is_empty() {
                    super::state::fanout_peer_client_event(
                        &swarm_members,
                        &client_session_id,
                        &client_connection_id,
                        ServerEvent::PeerUserMessage {
                            content: peer_content,
                        },
                    )
                    .await;
                }
            }

            Request::Cancel { id } => {
                cancel_processing_message(
                    &mut ProcessingState {
                        client_is_processing: &mut client_is_processing,
                        message_id: &mut processing_message_id,
                        session_id: &mut processing_session_id,
                        task: &mut processing_task,
                    },
                    &session_control,
                    &client_event_tx,
                    &SwarmStatusRefs {
                        members: &swarm_members,
                        swarms_by_id: &swarms_by_id,
                        event_history: &event_history,
                        event_counter: &event_counter,
                        event_tx: &swarm_event_tx,
                    },
                    Some(id),
                    Some(request_decoded_at),
                    Some(&sessions),
                )
                .await;
                if !client_is_processing {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.is_processing = false;
                        info.current_tool_name = None;
                    }
                }
            }

            Request::SoftInterrupt {
                id,
                content,
                images,
                urgent,
                client_id,
            } => {
                // A soft interrupt has somewhere to go only while a turn is
                // active. When the session is idle, queueing it would strand
                // the user's prompt until an unrelated future message starts
                // a turn. Claim the idle session and process it as the next
                // user message instead. The connection-map claim is atomic
                // with the cross-client busy check, so two attachments cannot
                // both decide that the same session is idle.
                let start_idle_turn = {
                    let mut connections = client_connections.write().await;
                    let active_turn_registered =
                        !crate::turn_cancel_registry::active_turn_signals(&client_session_id)
                            .is_empty();
                    let session_connection_busy = connections
                        .values()
                        .any(|info| info.session_id == client_session_id && info.is_processing);
                    let start = should_start_idle_soft_interrupt(
                        client_is_processing,
                        active_turn_registered,
                        session_connection_busy,
                    );
                    if start && let Some(info) = connections.get_mut(&client_connection_id) {
                        info.is_processing = true;
                    }
                    start
                };
                if start_idle_turn {
                    let accepted_content = content.clone();
                    start_processing_message(
                        ProcessingMessage {
                            id,
                            content,
                            images,
                            system_reminder: None,
                            active_skill: None,
                            skill_arguments: None,
                            server_failover: supports_server_failover,
                        },
                        &client_session_id,
                        &mut ProcessingState {
                            client_is_processing: &mut client_is_processing,
                            message_id: &mut processing_message_id,
                            session_id: &mut processing_session_id,
                            task: &mut processing_task,
                        },
                        &agent,
                        &client_event_tx,
                        &processing_done_tx,
                        active_terminal_env.clone(),
                        &SwarmStatusRefs {
                            members: &swarm_members,
                            swarms_by_id: &swarms_by_id,
                            event_history: &event_history,
                            event_counter: &event_counter,
                            event_tx: &swarm_event_tx,
                        },
                    )
                    .await;
                    // Ack only confirms receipt, not injection. TUI callers
                    // have no SDK client_id, but still retain a queued preview
                    // until this pickup event. Publish only after acceptance:
                    // a rejected prompt must remain recoverable by its caller.
                    if processing_message_id == Some(id) {
                        let _ = client_event_tx.send(ServerEvent::SoftInterruptInjected {
                            content: accepted_content,
                            display_role: None,
                            point: "turn_start".to_string(),
                            tools_skipped: None,
                            client_ids: client_id.into_iter().collect(),
                        });
                    }
                    if !client_is_processing {
                        let mut connections = client_connections.write().await;
                        if let Some(info) = connections.get_mut(&client_connection_id) {
                            info.is_processing = false;
                        }
                    }
                } else {
                    queue_soft_interrupt(
                        id,
                        content,
                        images,
                        urgent,
                        client_id,
                        &session_control,
                        &client_event_tx,
                    );
                }
            }

            Request::CancelSoftInterrupts { id, client_ids } => match client_ids {
                Some(client_ids) => cancel_soft_interrupts_by_id(
                    id,
                    &client_session_id,
                    &client_ids,
                    &session_control,
                    &client_event_tx,
                ),
                None => {
                    clear_soft_interrupts(id, &client_session_id, &session_control, &client_event_tx)
                }
            },

            Request::BackgroundTool { id } => {
                move_tool_to_background(id, &session_control, &client_event_tx);
            }

            Request::Clear { id } => {
                if reject_if_agent_busy_for_request(
                    id,
                    "clear",
                    &client_session_id,
                    client_is_processing,
                    &agent,
                    &client_event_tx,
                ) {
                    continue;
                }
                agent = crate::hooks::with_client_terminal_env(
                    active_terminal_env.clone(),
                    handle_clear_session(
                        id,
                        client_selfdev,
                        &mut client_session_id,
                        &client_connection_id,
                        &agent,
                        &provider,
                        &registry,
                        &sessions,
                        &shutdown_signals,
                        &soft_interrupt_queues,
                        &client_connections,
                        &swarm_members,
                        &swarms_by_id,
                        &file_touch,
                        &channel_subscriptions,
                        &channel_subscriptions_by_session,
                        &swarm_plans,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                        &client_event_tx,
                    ),
                )
                .await;
                session_control = refresh_session_control_handle(
                    &client_session_id,
                    &agent,
                    &shutdown_signals,
                    &soft_interrupt_queues,
                )
                .await;
            }

            Request::Rewind { id, message_index } => {
                if client_is_processing {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Cannot rewind while a turn is processing.".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }

                let rewind_result = {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.rewind_to_message(message_index)
                };

                match rewind_result {
                    Ok(removed) => {
                        crate::logging::info(&format!(
                            "Rewound session {} to message {} (removed {})",
                            client_session_id, message_index, removed
                        ));
                        if handle_get_history(
                            id,
                            &client_session_id,
                            client_is_processing,
                            &agent,
                            &provider,
                            &sessions,
                            &client_connections,
                            &client_count,
                            &writer,
                            &server_name,
                            &server_icon,
                            None,
                            supports_pdf_panels,
                            supports_turn_identity,
                            None,
                    )
                        .await
                        .is_err()
                        {
                            break;
                        }
                        // The truncated History replaces the client transcript
                        // (dropping the inline plan graph); re-send the plan so
                        // the diagram comes back.
                        send_swarm_plan_to_session(
                            &client_session_id,
                            &swarm_members,
                            &swarm_plans,
                        )
                        .await;
                    }
                    Err(message) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message,
                            retry_after_secs: None,
                        });
                    }
                }
            }

            Request::RewindUndo { id } => {
                if client_is_processing {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Cannot undo rewind while a turn is processing.".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }

                let undo_result = {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.undo_rewind()
                };

                match undo_result {
                    Ok(restored) => {
                        crate::logging::info(&format!(
                            "Undid rewind for session {} (restored {})",
                            client_session_id, restored
                        ));
                        if handle_get_history(
                            id,
                            &client_session_id,
                            client_is_processing,
                            &agent,
                            &provider,
                            &sessions,
                            &client_connections,
                            &client_count,
                            &writer,
                            &server_name,
                            &server_icon,
                            None,
                            supports_pdf_panels,
                            supports_turn_identity,
                            None,
                    )
                        .await
                        .is_err()
                        {
                            break;
                        }
                        // Same as rewind: restore the inline plan graph after
                        // the transcript replacement.
                        send_swarm_plan_to_session(
                            &client_session_id,
                            &swarm_members,
                            &swarm_plans,
                        )
                        .await;
                    }
                    Err(message) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message,
                            retry_after_secs: None,
                        });
                    }
                }
            }

            Request::Ping { id } => {
                let event = super::session_connections::connection_presence_pong(id, &client_connections, &swarm_members).await;
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
            }

            Request::PrepareDisconnect { id } => {
                drop(crate::tool::sdk::ConnectionGuard(client_connection_id.clone()));
                let json = encode_event(&ServerEvent::Done { id });
                let mut w = writer.lock().await;
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
            }

            Request::GetTodos { .. } => {
                let payload = serde_json::json!({
                    "todos": crate::todo::load_todos(&client_session_id).unwrap_or_else(|error| {
                        crate::logging::warn(&format!("Failed to load todos for {client_session_id}: {error}"));
                        Vec::new()
                    }),
                    "plan": crate::todo::load_plan(&client_session_id).unwrap_or_default(),
                    "goals": crate::todo::load_goals(&client_session_id).unwrap_or_default(),
                });
                let _ = client_event_tx.send(ServerEvent::Todos {
                    session_id: client_session_id.clone(),
                    payload: payload.to_string(),
                });
            }

            Request::GetState { id } => {
                if handle_get_state(
                    id,
                    &client_session_id,
                    client_is_processing,
                    session_control.pending_soft_interrupt_count(),
                    &sessions,
                    &writer,
                )
                .await
                .is_err()
                {
                    break;
                }
            }

            Request::Subscribe {
                id,
                system_prompt,
                supports_pdf_panels: requested_pdf_panels,
                supports_user_response: requested_user_response,
                supports_server_failover: requested_server_failover,
                supports_turn_identity: _,
                working_dir: subscribe_working_dir,
                selfdev,
                target_session_id,
                client_instance_id,
                client_has_local_history,
                history_base,
                allow_session_takeover,
                crash_on_disconnect: _,
                continue_on_disconnect: requested_continuation,
                terminal_env,
            } => {
                if let Err(message) =
                    validated_subscribe_working_dir(
                        subscribe_working_dir.as_deref(), requested_continuation,
                    )
                {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message,
                        retry_after_secs: None,
                    });
                    continue;
                }
                // Overrides are creation-only. In particular, never apply one to
                // a target attachment or a repeated Subscribe on this connection.
                if let Some(prompt) = new_session_system_prompt(
                    provisional_session,
                    target_session_id.as_deref(),
                    system_prompt.as_deref(),
                ) {
                    agent.lock().await.set_system_prompt(prompt);
                }
                // Every Subscribe carries an authoritative snapshot. An empty
                // snapshot must clear terminal vars inherited by the daemon
                // rather than retaining a prior pane's values.
                continue_on_disconnect = requested_continuation;
                supports_pdf_panels = requested_pdf_panels;
                supports_user_response = requested_user_response;
                supports_server_failover = requested_server_failover;
                active_terminal_env = terminal_env;
                current_client_instance_id = client_instance_id.clone();
                {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.client_instance_id = client_instance_id.clone();
                        info.terminal_env = active_terminal_env.clone();
                    }
                }
                if let Some(target_session_id) = target_session_id {
                    // A brand-new desktop panel has no transcript on disk until
                    // its first prompt. Its creator connection can detach before
                    // the panel connection arrives, while the live agent is
                    // already registered in memory. Treat that as an existing
                    // session or the target-aware subscribe silently creates a
                    // different session and every subsequent command reports a
                    // wrong-session attachment.
                    if crate::session::session_exists(&target_session_id)
                        || sessions.read().await.contains_key(&target_session_id)
                    {
                        let pre_resume_session_id = client_session_id.clone();
                        agent = crate::hooks::with_client_terminal_env(
                            active_terminal_env.clone(),
                            handle_resume_session(
                                id,
                                target_session_id.clone(),
                                subscribe_working_dir.as_deref(),
                                client_instance_id.as_deref(),
                                client_has_local_history,
                                history_base,
                                allow_session_takeover,
                                &mut client_selfdev,
                                &mut client_session_id,
                                &client_connection_id,
                                &agent,
                                &provider,
                                &registry,
                                &sessions,
                                &shutdown_signals,
                                &soft_interrupt_queues,
                                &client_connections,
                                &client_debug_state,
                                &swarm_members,
                                &swarms_by_id,
                                &file_touch,
                                &channel_subscriptions,
                                &channel_subscriptions_by_session,
                                &swarm_plans,
                                &swarm_coordinators,
                                &client_count,
                                &writer,
                                &server_name,
                                &server_icon,
                                &client_event_tx,
                                &mcp_pool,
                                &event_history,
                                &event_counter,
                                &swarm_event_tx,
                                supports_pdf_panels,
                                supports_turn_identity,
                            ),
                        )
                        .await?;
                        session_control = refresh_session_control_handle(
                            &client_session_id,
                            &agent,
                            &shutdown_signals,
                            &soft_interrupt_queues,
                        )
                        .await;
                        if client_session_id == target_session_id {
                            handle_subscribe(
                                id,
                                subscribe_working_dir,
                                selfdev,
                                false,
                                &mut client_selfdev,
                                &client_session_id,
                                &client_connection_id,
                                &friendly_name,
                                &agent,
                                &registry,
                                swarm_enabled,
                                &swarm_members,
                                &swarms_by_id,
                                &channel_subscriptions,
                                &channel_subscriptions_by_session,
                                &swarm_plans,
                                &swarm_coordinators,
                                &client_event_tx,
                                &mcp_pool,
                                &event_history,
                                &event_counter,
                                &swarm_event_tx,
                                &sessions,
                                &soft_interrupt_queues,
                            )
                            .await;
                            if let Some(snapshot) = try_available_models_snapshot(&agent).await {
                                last_available_models_snapshot = Some(snapshot);
                            }
                        } else {
                            crate::logging::warn(&format!(
                                "Target-aware subscribe failed to bind {} from temporary {}; closing temporary client connection {}",
                                target_session_id, pre_resume_session_id, client_connection_id
                            ));
                            break;
                        }
                    } else {
                        if provisional_session {
                            agent.lock().await.activate_concurrency_tracking();
                        }
                        handle_subscribe(
                            id,
                            subscribe_working_dir,
                            selfdev,
                            true,
                            &mut client_selfdev,
                            &client_session_id,
                            &client_connection_id,
                            &friendly_name,
                            &agent,
                            &registry,
                            swarm_enabled,
                            &swarm_members,
                            &swarms_by_id,
                            &channel_subscriptions,
                            &channel_subscriptions_by_session,
                            &swarm_plans,
                            &swarm_coordinators,
                            &client_event_tx,
                            &mcp_pool,
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            &sessions,
                            &soft_interrupt_queues,
                        )
                        .await;
                    }
                } else {
                    if provisional_session {
                        agent.lock().await.activate_concurrency_tracking();
                    }
                    handle_subscribe(
                        id,
                        subscribe_working_dir,
                        selfdev,
                        true,
                        &mut client_selfdev,
                        &client_session_id,
                        &client_connection_id,
                        &friendly_name,
                        &agent,
                        &registry,
                        swarm_enabled,
                        &swarm_members,
                        &swarms_by_id,
                        &channel_subscriptions,
                        &channel_subscriptions_by_session,
                        &swarm_plans,
                        &swarm_coordinators,
                        &client_event_tx,
                        &mcp_pool,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                        &sessions,
                        &soft_interrupt_queues,
                    )
                    .await;
                    // No catalog dedup baseline here. A fresh subscribe sends
                    // no catalog to the client (History does, and GetHistory
                    // records the baseline right after). Building the snapshot
                    // here cost ~200ms on a cold server and sat directly in
                    // front of the client's GetHistory, delaying first paint.
                    last_available_models_snapshot = None;
                }
                super::client_session::apply_or_defer_desktop_response_capability(
                    &agent,
                    supports_user_response,
                )
                .await;
                if requested_continuation {
                    super::runtime_ssh_loops::initialize_remote_session(
                        &client_session_id,
                        crate::config::config().features.auto_poke,
                    )?;
                }
                client_subscribed = true;
                provisional_session = false;
            }

            Request::GetHistory { id } => {
                if handle_get_history(
                    id,
                    &client_session_id,
                    client_is_processing,
                    &agent,
                    &provider,
                    &sessions,
                    &client_connections,
                    &client_count,
                    &writer,
                    &server_name,
                    &server_icon,
                    None,
                    supports_pdf_panels,
                    supports_turn_identity,
                    None,
            )
                .await
                .is_err()
                {
                    break;
                }
                // Follow the History payload with the current swarm plan: a
                // session-changing History clears the client's plan snapshot
                // (and the inline plan graph), so re-send it afterwards
                // instead of leaving the graph blank until the next plan
                // mutation broadcast.
                send_swarm_plan_to_session(&client_session_id, &swarm_members, &swarm_plans).await;
                if let Some(snapshot) = try_available_models_snapshot(&agent).await {
                    last_available_models_snapshot = Some(snapshot);
                }
            }

            Request::GetModelCatalog { id, subscribe_usage_updates } => {
                model_usage_updates_enabled = subscribe_usage_updates;
                if handle_get_model_catalog(id, &client_session_id, &agent, &provider, &writer)
                    .await
                    .is_err()
                {
                    break;
                }
                if let Some(snapshot) = try_available_models_snapshot(&agent).await {
                    last_available_models_snapshot = Some(snapshot);
                }
            }

            Request::GetCompactedHistory {
                id,
                visible_messages,
            } => {
                if handle_get_compacted_history(
                    id,
                    &client_session_id,
                    &agent,
                    &writer,
                    visible_messages,
                )
                .await
                .is_err()
                {
                    break;
                }
            }

            Request::DebugCommand { id, .. } => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: "debug_command is only supported on the debug socket".to_string(),
                    retry_after_secs: None,
                });
            }

            Request::UpgradeRuntime { id, candidate } => {
                super::runtime_upgrade::handle(id, &candidate, &writer, &sessions).await?;
            }

            Request::Reload { id, force } => {
                handle_reload(
                    id,
                    force,
                    &client_session_id,
                    &agent,
                    &swarm_members,
                    &client_event_tx,
                )
                .await;
            }

            Request::ResumeSession {
                id,
                session_id,
                client_instance_id,
                client_has_local_history,
                allow_session_takeover,
            } => {
                let pre_resume_session_id = client_session_id.clone();
                let resume_working_dir = {
                    let agent_guard = agent.lock().await;
                    agent_guard.working_dir().map(str::to_string)
                };
                current_client_instance_id = client_instance_id.clone();
                {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.client_instance_id = client_instance_id.clone();
                    }
                }
                agent = crate::hooks::with_client_terminal_env(
                    active_terminal_env.clone(),
                    handle_resume_session(
                        id,
                        session_id,
                        resume_working_dir.as_deref(),
                        client_instance_id.as_deref(),
                        client_has_local_history,
                        None,
                        allow_session_takeover,
                        &mut client_selfdev,
                        &mut client_session_id,
                        &client_connection_id,
                        &agent,
                        &provider,
                        &registry,
                        &sessions,
                        &shutdown_signals,
                        &soft_interrupt_queues,
                        &client_connections,
                        &client_debug_state,
                        &swarm_members,
                        &swarms_by_id,
                        &file_touch,
                        &channel_subscriptions,
                        &channel_subscriptions_by_session,
                        &swarm_plans,
                        &swarm_coordinators,
                        &client_count,
                        &writer,
                        &server_name,
                        &server_icon,
                        &client_event_tx,
                        &mcp_pool,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                        supports_pdf_panels,
                        supports_turn_identity,
                    ),
                )
                .await?;
                if client_session_id != pre_resume_session_id {
                    provisional_session = false;
                }
                super::client_session::apply_or_defer_desktop_response_capability(
                    &agent,
                    supports_user_response,
                )
                .await;
                session_control = refresh_session_control_handle(
                    &client_session_id,
                    &agent,
                    &shutdown_signals,
                    &soft_interrupt_queues,
                )
                .await;
                if let Some(snapshot) = try_available_models_snapshot(&agent).await {
                    last_available_models_snapshot = Some(snapshot);
                }
            }

            Request::ResumeAllSessions { id } => {
                super::client_actions::handle_resume_all_sessions(
                    id,
                    &sessions,
                    &swarm_members,
                    &swarms_by_id,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &client_event_tx,
                )
                .await;
            }

            Request::CycleModel { id, direction } => {
                handle_cycle_model(id, direction, &agent, &client_event_tx).await;
            }

            Request::RefreshModels { id } => {
                handle_refresh_models(id, &provider, &agent, &client_event_tx).await;
            }

            Request::SetPremiumMode { id, mode } => {
                handle_set_premium_mode(id, mode, &agent, &client_event_tx).await;
            }

            Request::SetModel { id, model } => {
                handle_set_model(id, model, &agent, &client_event_tx).await;
            }

            Request::SetRoute { id, selection, automatic_failover } => {
                handle_set_route(id, selection, automatic_failover, &agent, &client_event_tx).await;
            }

            Request::SetAgentModel { id, target, model, global } => {
                let result = if global {
                    super::client_actions::set_global_agent_model(
                        &agent, &client_session_id, &target, model,
                    ).await
                } else {
                    super::client_actions::set_session_agent_model(
                        &agent, &client_session_id, &target, model,
                    ).await
                };
                let event = match result {
                    Ok(overrides) => ServerEvent::AgentModelsChanged { id, session_id: client_session_id.clone(), overrides },
                    Err(error) => ServerEvent::Error { id, message: error.to_string(), retry_after_secs: None },
                };
                if let Err(error) = client_event_tx.send(event) {
                crate::logging::debug(&format!("Client disconnected before event delivery: {error}"));
            }
            }

            Request::SetSubagentModel { id, model } => {
                if reject_if_agent_busy_for_request(
                    id,
                    "set_subagent_model",
                    &client_session_id,
                    client_is_processing,
                    &agent,
                    &client_event_tx,
                ) {
                    continue;
                }
                handle_set_subagent_model(id, model, &agent, &client_event_tx).await;
            }

            Request::RunSubagent {
                id,
                prompt,
                subagent_type,
                model,
                session_id,
            } => {
                handle_run_subagent(
                    id,
                    prompt,
                    subagent_type,
                    model,
                    session_id,
                    &agent,
                    &client_event_tx,
                );
            }

            Request::SetContextCap {
                id,
                max_context_tokens,
                session_id,
            } => {
                let target_agent = sessions.read().await.get(&session_id).cloned();
                if let Some(target_agent) = target_agent {
                    handle_set_context_cap(id, session_id, max_context_tokens, &target_agent, &client_event_tx).await;
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "target session not found".to_string(),
                        retry_after_secs: None,
                    });
                }
            }

            Request::SetReasoningEffort {
                id,
                effort,
                target_session_id,
            } => {
                if let Some(target_session_id) = target_session_id {
                    let target_agent = { sessions.read().await.get(&target_session_id).cloned() };
                    if let Some(target_agent) = target_agent {
                        handle_set_reasoning_effort(id, effort, &target_agent, &client_event_tx)
                            .await;
                    } else {
                        let _ = client_event_tx.send(ServerEvent::ReasoningEffortChanged {
                            id,
                            effort: None,
                            error: Some(format!("target session not found: {target_session_id}")),
                        });
                    }
                } else {
                    handle_set_reasoning_effort(id, effort, &agent, &client_event_tx).await;
                }
            }

            Request::SetServiceTier { id, service_tier } => {
                handle_set_service_tier(id, service_tier, &agent, session_control.provider_handle(), &client_event_tx).await;
            }

            Request::SetTransport { id, transport } => {
                handle_set_transport(id, transport, &agent, &client_event_tx).await;
            }

            Request::SetCompactionMode { id, mode } => {
                handle_set_compaction_mode(id, mode, &agent, &client_event_tx).await;
            }

            Request::SetSessionEnv { id, env } => {
                let result = super::set_live_session_env(&sessions, &client_session_id, env).await;
                let _ = client_event_tx.send(match result {
                    Ok(()) => ServerEvent::Done { id },
                    Err(_) => ServerEvent::Error {
                        id,
                        message: "Invalid session environment".to_string(),
                        retry_after_secs: None,
                    },
                });
            }

            Request::SetSessionSaved { id, saved, label } => {
                if reject_if_agent_busy_for_request(
                    id,
                    "set_session_saved",
                    &client_session_id,
                    client_is_processing,
                    &agent,
                    &client_event_tx,
                ) {
                    continue;
                }
                handle_set_session_saved(
                    id,
                    saved,
                    label,
                    &agent,
                    &client_session_id,
                    &swarm_members,
                    &client_event_tx,
                )
                .await;
            }

            Request::PersistSession { id } => {
                // Wait for the lock instead of rejecting: this runs at an idle
                // turn boundary and must not fail spuriously.
                let result = agent.lock().await.persist_session_now();
                let _ = client_event_tx.send(match result {
                    Ok(()) => ServerEvent::Done { id },
                    Err(error) => ServerEvent::Error {
                        id,
                        message: crate::util::format_error_chain(&error),
                        retry_after_secs: None,
                    },
                });
            }

            Request::RenameSession { id, title } => {
                if reject_if_agent_busy_for_request(
                    id,
                    "rename_session",
                    &client_session_id,
                    client_is_processing,
                    &agent,
                    &client_event_tx,
                ) {
                    continue;
                }
                handle_rename_session(
                    id,
                    title,
                    &agent,
                    &client_session_id,
                    &swarm_members,
                    &client_event_tx,
                )
                .await;
            }

            Request::NotifyAuthChanged {
                id,
                provider: provider_hint,
                auth,
                prefer_strongest,
            } => {
                handle_notify_auth_changed(
                    id,
                    provider_hint,
                    auth,
                    prefer_strongest,
                    &provider,
                    &provider_template,
                    &sessions,
                    &client_session_id,
                    &agent,
                    &client_event_tx,
                )
                .await;
            }

            Request::SwitchAnthropicAccount { id, label } => {
                handle_switch_anthropic_account(id, label, &agent, &client_event_tx).await;
            }

            Request::SwitchOpenAiAccount { id, label } => {
                handle_switch_openai_account(id, label, &agent, &client_event_tx).await;
            }

            Request::AccountUsage { id } => {
                super::account_usage::request(id, client_event_tx.clone());
            }

            Request::InvalidateOpenAiUsage { id, account_label } => {
                handle_invalidate_openai_usage(id, account_label, &client_event_tx).await;
            }

            Request::InvalidateAnthropicUsage { id, account_label } => {
                handle_invalidate_anthropic_usage(id, account_label, &client_event_tx).await;
            }

            Request::SetFeature {
                id,
                feature,
                enabled,
            } => {
                if reject_if_agent_busy_for_request(
                    id,
                    "set_feature",
                    &client_session_id,
                    client_is_processing,
                    &agent,
                    &client_event_tx,
                ) {
                    continue;
                }
                handle_set_feature(
                    id,
                    feature,
                    enabled,
                    &agent,
                    &client_session_id,
                    &friendly_name,
                    &mut swarm_enabled,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &channel_subscriptions,
                    &channel_subscriptions_by_session,
                    &swarm_plans,
                    &client_event_tx,
                )
                .await;
            }

            Request::Split { id } => {
                handle_split(id, &client_session_id, &agent, &client_event_tx).await;
            }

            Request::Transfer { id } => {
                if reject_if_agent_busy_for_request(
                    id,
                    "transfer",
                    &client_session_id,
                    client_is_processing,
                    &agent,
                    &client_event_tx,
                ) {
                    continue;
                }
                handle_transfer(id, &client_session_id, &agent, &client_event_tx).await;
            }

            Request::Compact { id } => {
                handle_compact(id, &agent, &client_event_tx);
            }

            Request::TriggerMemoryExtraction { id } => {
                if reject_if_agent_busy_for_request(
                    id,
                    "trigger_memory_extraction",
                    &client_session_id,
                    client_is_processing,
                    &agent,
                    &client_event_tx,
                ) {
                    continue;
                }
                handle_trigger_memory_extraction(id, &agent, &client_event_tx).await;
            }

            // Agent-to-agent communication
            Request::AgentRegister { id, .. } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::StdinResponse {
                id,
                request_id,
                input,
            } => {
                handle_stdin_response(id, request_id, input, &stdin_responses, &client_event_tx)
                    .await;
            }

            Request::AgentTask { id, task, .. } => {
                handle_agent_task(
                    id,
                    task,
                    &client_session_id,
                    &agent,
                    &AgentTaskContext {
                        client_event_tx: &client_event_tx,
                        swarm_members: &swarm_members,
                        swarms_by_id: &swarms_by_id,
                        event_history: &event_history,
                        event_counter: &event_counter,
                        swarm_event_tx: &swarm_event_tx,
                    },
                )
                .await;
            }

            Request::AgentCapabilities { id } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::RuntimeCommand { id, command, args } => {
                let result: anyhow::Result<(serde_json::Value, String)> = async {
                    // Stop/status must remain available while a loop's model or
                    // tool turn owns the Agent mutex.
                    if let Some(result) = super::runtime_ssh_loops::control(&client_session_id, &command, &args)? {
                        return Ok(result);
                    }
                    // Supplemental SSH/cloud commands intentionally precede the shared
                    // enum parser so new host workflows do not require a wire-enum release.
                    if super::runtime_ssh_sessions::handles(&command)
                        || super::runtime_ssh_loops::handles(&command)
                        || super::runtime_ssh_accounts::handles(&command)
                        || super::runtime_ssh_host::handles(&command)
                    {
                        if super::runtime_ssh_loops::handles(&command)
                            && (client_is_processing || crate::turn_cancel_registry::has_active_turn(&client_session_id)) {
                            anyhow::bail!("Session is busy. Stop/status are available now. Start or resume after the current turn.");
                        }
                        let mut guard = agent.try_lock().map_err(|_| anyhow::anyhow!("Session is busy. Retry this command after the current turn."))?;
                        if super::runtime_ssh_sessions::handles(&command) {
                            return super::runtime_ssh_sessions::execute(&mut guard, &command, &args).await;
                        }
                        if super::runtime_ssh_loops::handles(&command) {
                            return super::runtime_ssh_loops::execute(&mut guard, &command, &args).await;
                        }
                        if super::runtime_ssh_accounts::handles(&command) {
                            return super::runtime_ssh_accounts::execute(&mut guard, &command, &args).await;
                        }
                        return super::runtime_ssh_host::execute(&mut guard, &command, &args).await;
                    }
                    let parsed: jcode_harness_api::RuntimeCommand = serde_json::from_value(serde_json::json!(command))?;
                    if parsed == jcode_harness_api::RuntimeCommand::ServerReload {
                        if !args.is_empty() { anyhow::bail!("Server reload accepts no arguments. Forced reload is intentionally not exposed."); }
                        let _ = client_event_tx.send(ServerEvent::RuntimeCommandResult { id, command: command.clone(),
                            data: serde_json::json!({"requested": true, "force": false}),
                            message: "Requested a state-preserving server upgrade if a newer binary is available. Watch reload progress for completion.".into() });
                        handle_reload(id, false, &client_session_id, &agent, &swarm_members, &client_event_tx).await;
                        return Ok((serde_json::Value::Null, String::new()));
                    }
                    if matches!(parsed, jcode_harness_api::RuntimeCommand::Continue | jcode_harness_api::RuntimeCommand::ResumeAll) {
                        if !args.is_empty() { anyhow::bail!("This command accepts no arguments"); }
                        let (tx, mut rx) = mpsc::unbounded_channel();
                        super::client_actions::handle_resume_all_sessions(id, &sessions, &swarm_members, &swarms_by_id,
                            &event_history, &event_counter, &swarm_event_tx, &tx).await;
                        while let Ok(event) = rx.try_recv() {
                            if let ServerEvent::ResumeAllResult { resumed, skipped, message, resumed_sessions, .. } = event {
                                return Ok((serde_json::json!({"resumed": resumed, "skipped": skipped, "resumed_sessions": resumed_sessions}), message));
                            }
                        }
                        anyhow::bail!("Runtime did not report continuation outcome");
                    }
                    if parsed == jcode_harness_api::RuntimeCommand::RefreshModelList {
                        if !args.is_empty() { anyhow::bail!("This command accepts no arguments"); }
                        provider.refresh_model_catalog().await?;
                        crate::bus::Bus::global().publish_models_updated();
                        let guard = agent.try_lock().map_err(|_| anyhow::anyhow!("Catalog refreshed, but session is busy. Open the model picker after the turn."))?;
                        let data = serde_json::json!({"refreshed": true, "models": guard.available_models_for_switching(), "routes": guard.model_routes()});
                        return Ok((data, "Provider model catalog refreshed.".into()));
                    }
                    if parsed == jcode_harness_api::RuntimeCommand::Swarm {
                        if let Some(enabled) = super::runtime_commands::boolean_action(&args, swarm_enabled, false)? {
                            if reject_if_agent_busy_for_request(id, "runtime_command", &client_session_id, client_is_processing, &agent, &client_event_tx) {
                                return Ok((serde_json::Value::Null, String::new()));
                            }
                            let (tx, mut rx) = mpsc::unbounded_channel();
                            handle_set_feature(id, crate::protocol::FeatureToggle::Swarm, enabled, &agent, &client_session_id, &friendly_name,
                                &mut swarm_enabled, &swarm_members, &swarms_by_id, &swarm_coordinators,
                                &channel_subscriptions, &channel_subscriptions_by_session, &swarm_plans, &tx).await;
                            while let Ok(event) = rx.try_recv() {
                                if let ServerEvent::Error { message, .. } = event { anyhow::bail!("{message}"); }
                            }
                        }
                        let members = swarm_members.read().await;
                        let member = members.get(&client_session_id);
                        let data = serde_json::json!({"enabled": swarm_enabled, "config_default": crate::config::config().features.swarm,
                            "swarm_id": member.and_then(|m| m.swarm_id.as_deref()), "role": member.map(|m| &m.role)});
                        return Ok((data.clone(), serde_json::to_string_pretty(&data)?));
                    }
                    if parsed == jcode_harness_api::RuntimeCommand::Subscription {
                        if !args.is_empty() && args != ["status".to_string()] { anyhow::bail!("Usage: subscription [status]"); }
                        let data = if crate::subscription_catalog::has_credentials() {
                            serde_json::to_value(crate::subscription_api::fetch_subscription_me().await?)?
                        } else { serde_json::json!({"signed_in": false, "status": "not_signed_in"}) };
                        return Ok((data.clone(), serde_json::to_string_pretty(&data)?));
                    }
                    let mut guard = agent.try_lock().map_err(|_| anyhow::anyhow!("Session is busy. Retry this runtime command after the current turn."))?;
                    super::runtime_commands::execute(&mut guard, parsed, &args, swarm_enabled, serde_json::Value::Null).await
                }.await;
                match result {
                    Ok((mut data, message)) if !data.is_null() => {
                        // Only the server loop implementation can supply a
                        // launch item. Never dispatch arbitrary response data
                        // from an unrelated runtime command as a model prompt.
                        let next_prompt = if super::runtime_ssh_loops::handles(&command) {
                            data.as_object_mut().and_then(|object| object.remove("next_prompt"))
                                .and_then(|value| value.as_str().map(str::to_owned))
                        } else { None };
                        if next_prompt.is_some() { data["started"] = serde_json::json!(true); }
                        let _ = client_event_tx.send(ServerEvent::RuntimeCommandResult { id, command, data, message });
                        if let Some(content) = next_prompt {
                            // The execute closure has returned and dropped its
                            // Agent guard. Reuse the actual message lifecycle:
                            // processing state, task ownership, fanout, cancel,
                            // failover, completion and disconnected continuation.
                            if provisional_session {
                                agent.lock().await.activate_concurrency_tracking();
                                provisional_session = false;
                            }
                            if !client_is_processing {
                                if continue_on_disconnect && let Ok(mut guard) = agent.try_lock() {
                                    guard.set_stdin_request_tx(stdin_req_tx.clone());
                                }
                                let mut connections = client_connections.write().await;
                                if let Some(info) = connections.get_mut(&client_connection_id) {
                                    info.is_processing = true;
                                    info.current_tool_name = None;
                                }
                            }
                            start_processing_message(
                                ProcessingMessage {
                                    id, content, images: Vec::new(), system_reminder: None,
                                    active_skill: None, skill_arguments: None, server_failover: true,
                                },
                                &client_session_id,
                                &mut ProcessingState {
                                    client_is_processing: &mut client_is_processing,
                                    message_id: &mut processing_message_id,
                                    session_id: &mut processing_session_id,
                                    task: &mut processing_task,
                                },
                                &agent, &client_event_tx, &processing_done_tx,
                                active_terminal_env.clone(),
                                &SwarmStatusRefs {
                                    members: &swarm_members, swarms_by_id: &swarms_by_id,
                                    event_history: &event_history, event_counter: &event_counter,
                                    event_tx: &swarm_event_tx,
                                },
                            ).await;
                        }
                    },
                    Ok(_) => {},
                    Err(error) => { let _ = client_event_tx.send(ServerEvent::Error { id, message: error.to_string(), retry_after_secs: None }); },
                }
            }
            Request::AgentContext { id } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::NotifySession {
                id,
                session_id,
                message,
            } => {
                handle_notify_session(
                    id,
                    session_id,
                    message,
                    NotifySessionContext {
                        sessions: &sessions,
                        soft_interrupt_queues: &soft_interrupt_queues,
                        client_connections: &client_connections,
                        swarm_members: &swarm_members,
                        swarms_by_id: &swarms_by_id,
                        event_history: &event_history,
                        event_counter: &event_counter,
                        swarm_event_tx: &swarm_event_tx,
                        client_event_tx: &client_event_tx,
                    },
                )
                .await;
            }

            Request::AppletAction {
                id,
                session_id,
                instance,
                action,
                state,
                source_key,
            } => {
                super::client_actions::handle_applet_action(
                    id,
                    session_id,
                    instance,
                    action,
                    state,
                    source_key,
                    NotifySessionContext {
                        sessions: &sessions,
                        soft_interrupt_queues: &soft_interrupt_queues,
                        client_connections: &client_connections,
                        swarm_members: &swarm_members,
                        swarms_by_id: &swarms_by_id,
                        event_history: &event_history,
                        event_counter: &event_counter,
                        swarm_event_tx: &swarm_event_tx,
                        client_event_tx: &client_event_tx,
                    },
                )
                .await;
            }

            Request::CloseApplet {
                id,
                session_id,
                instance,
            } => {
                super::client_actions::handle_close_applet(
                    id,
                    session_id,
                    instance,
                    &client_event_tx,
                );
            }

            Request::Transcript {
                id,
                text,
                mode,
                session_id,
            } => {
                match super::debug::inject_transcript(
                    id,
                    text,
                    mode,
                    session_id,
                    &client_connections,
                    &client_debug_state,
                    &swarm_members,
                )
                .await
                {
                    Ok(event) => {
                        if let Err(error) = client_event_tx.send(event) {
                crate::logging::debug(&format!("Client disconnected before event delivery: {error}"));
            }
                    }
                    Err(error) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: error.to_string(),
                            retry_after_secs: None,
                        });
                    }
                }
            }

            Request::InputShell { id, command } => {
                handle_input_shell(id, command, &agent, &client_event_tx);
            }

            // === Agent communication ===
            Request::CommShare {
                id,
                session_id: req_session_id,
                key,
                value,
                append,
            } => {
                handle_comm_share(
                    id,
                    req_session_id,
                    key,
                    value,
                    append,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &shared_context,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                )
                .await;
            }

            Request::CommRead {
                id,
                session_id: req_session_id,
                key,
            } => {
                handle_comm_read(
                    id,
                    req_session_id,
                    key,
                    &client_event_tx,
                    &swarm_members,
                    &shared_context,
                )
                .await;
            }

            Request::CommMessage {
                id,
                from_session,
                message,
                to_session,
                channel,
                delivery,
                wake,
                tldr,
                to_swarm,
                require_target,
            } => {
                handle_comm_message(
                    id,
                    from_session,
                    message,
                    to_session,
                    channel,
                    delivery,
                    wake,
                    tldr,
                    to_swarm,
                    require_target,
                    &client_event_tx,
                    &sessions,
                    &soft_interrupt_queues,
                    &swarm_members,
                    &swarms_by_id,
                    &channel_subscriptions,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &client_connections,
                )
                .await;
            }

            Request::CommList {
                id,
                session_id: req_session_id,
            } => {
                handle_comm_list(
                    id,
                    req_session_id,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &file_touch,
                    &sessions,
                    &client_connections,
                )
                .await;
            }

            Request::CommListSwarms {
                id,
                session_id: req_session_id,
            } => {
                handle_comm_list_swarms(
                    id,
                    req_session_id,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                )
                .await;
            }

            Request::CommSetSwarmLabel {
                id,
                session_id: req_session_id,
                label,
            } => {
                handle_comm_set_swarm_label(
                    id,
                    req_session_id,
                    label,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                )
                .await;
            }

            Request::CommListChannels {
                id,
                session_id: req_session_id,
            } => {
                handle_comm_list_channels(
                    id,
                    req_session_id,
                    &client_event_tx,
                    &swarm_members,
                    &channel_subscriptions,
                )
                .await;
            }

            Request::CommChannelMembers {
                id,
                session_id: req_session_id,
                channel,
            } => {
                handle_comm_channel_members(
                    id,
                    req_session_id,
                    channel,
                    &client_event_tx,
                    &swarm_members,
                    &channel_subscriptions,
                )
                .await;
            }

            Request::CommProposePlan {
                id,
                session_id: req_session_id,
                items,
            } => {
                handle_comm_propose_plan(
                    id,
                    req_session_id,
                    items,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &shared_context,
                    &swarm_plans,
                    &swarm_coordinators,
                    &sessions,
                    &soft_interrupt_queues,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &swarm_mutation_runtime,
                )
                .await;
            }

            Request::CommApprovePlan {
                id,
                session_id: req_session_id,
                proposer_session,
            } => {
                handle_comm_approve_plan(
                    id,
                    req_session_id,
                    proposer_session,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &shared_context,
                    &swarm_plans,
                    &swarm_coordinators,
                    &sessions,
                    &soft_interrupt_queues,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &swarm_mutation_runtime,
                )
                .await;
            }

            Request::CommRejectPlan {
                id,
                session_id: req_session_id,
                proposer_session,
                reason,
            } => {
                handle_comm_reject_plan(
                    id,
                    req_session_id,
                    proposer_session,
                    reason,
                    &client_event_tx,
                    &swarm_members,
                    &shared_context,
                    &swarm_coordinators,
                    &sessions,
                    &soft_interrupt_queues,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &swarm_mutation_runtime,
                )
                .await;
            }

            Request::CommSpawn {
                id,
                session_id: req_session_id,
                lane_id,
                working_dir,
                initial_message,
                request_nonce,
                spawn_mode,
                model,
                effort,
                label,
            } => {
                let spawn_mode = match parse_swarm_spawn_mode(id, spawn_mode, &client_event_tx) {
                    Some(spawn_mode) => spawn_mode,
                    None => return Ok(()),
                };
                handle_comm_spawn(
                    id,
                    req_session_id,
                    lane_id,
                    working_dir,
                    initial_message,
                    request_nonce,
                    spawn_mode,
                    model,
                    effort,
                    label,
                    &client_event_tx,
                    &sessions,
                    &global_session_id,
                    &provider_template,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &swarm_plans,
                    &channel_subscriptions,
                    &channel_subscriptions_by_session,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &mcp_pool,
                    &soft_interrupt_queues,
                    &swarm_mutation_runtime,
                    &client_connections,
                )
                .await;
            }

            Request::CommListModels {
                id,
                session_id: req_session_id,
            } => {
                super::comm_session::handle_comm_list_models(
                    id,
                    &req_session_id,
                    &sessions,
                    &provider_template,
                    |event| {
                        if let Err(error) = client_event_tx.send(event) {
                crate::logging::debug(&format!("Client disconnected before event delivery: {error}"));
            }
                    },
                )
                .await;
            }

            Request::CommStop {
                id,
                session_id: req_session_id,
                target_session,
                force,
            } => {
                handle_comm_stop(
                    id,
                    req_session_id,
                    target_session,
                    force.unwrap_or(false),
                    &client_event_tx,
                    &sessions,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &swarm_plans,
                    &channel_subscriptions,
                    &channel_subscriptions_by_session,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &soft_interrupt_queues,
                    &swarm_mutation_runtime,
                )
                .await;
            }

            Request::CommAssignRole {
                id,
                session_id: req_session_id,
                target_session,
                role,
            } => {
                handle_comm_assign_role(
                    id,
                    req_session_id,
                    target_session,
                    role,
                    &client_event_tx,
                    &sessions,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &swarm_plans,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &swarm_mutation_runtime,
                )
                .await;
            }

            Request::CommSummary {
                id,
                session_id: req_session_id,
                target_session,
                limit,
            } => {
                handle_comm_summary(
                    id,
                    req_session_id,
                    target_session,
                    limit,
                    &sessions,
                    &swarm_members,
                    &client_event_tx,
                )
                .await;
            }

            Request::CommStatus {
                id,
                session_id: req_session_id,
                target_session,
            } => {
                handle_comm_status(
                    id,
                    req_session_id,
                    target_session,
                    &sessions,
                    &swarm_members,
                    &client_connections,
                    &file_touch,
                    &client_event_tx,
                )
                .await;
            }

            Request::CommReport {
                id,
                session_id: req_session_id,
                status,
                message,
                validation,
                follow_up,
                tldr,
            } => {
                let status = status.unwrap_or_else(|| "ready".to_string());
                let report = format_structured_completion_report(
                    &message,
                    validation.as_deref(),
                    follow_up.as_deref(),
                );
                let detail = Some(truncate_detail(&message, 160));
                update_member_status_with_report_tldr(
                    &req_session_id,
                    &status,
                    detail,
                    Some(report),
                    tldr,
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                    Some(&swarm_event_tx),
                )
                .await;
                super::comm_control::close_reporter_task_on_terminal_report(
                    &req_session_id,
                    &status,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_plans,
                    &swarm_coordinators,
                )
                .await;
                let _ = client_event_tx.send(ServerEvent::CommReportResponse {
                    id,
                    status,
                    message: "Report recorded and delivered to the coordinator when applicable."
                        .to_string(),
                });
            }

            Request::CommPlanStatus {
                id,
                session_id: req_session_id,
            } => {
                handle_comm_plan_status(
                    id,
                    req_session_id,
                    &swarm_members,
                    &swarm_plans,
                    &client_event_tx,
                )
                .await;
            }

            Request::CommReadContext {
                id,
                session_id: req_session_id,
                target_session,
            } => {
                handle_comm_read_context(
                    id,
                    req_session_id,
                    target_session,
                    &sessions,
                    &swarm_members,
                    &client_event_tx,
                )
                .await;
            }

            Request::CommResyncPlan {
                id,
                session_id: req_session_id,
            } => {
                handle_comm_resync_plan(
                    id,
                    req_session_id,
                    &CommResyncPlanContext {
                        client_event_tx: &client_event_tx,
                        swarm_members: &swarm_members,
                        swarms_by_id: &swarms_by_id,
                        swarm_plans: &swarm_plans,
                        swarm_coordinators: &swarm_coordinators,
                        event_history: &event_history,
                        event_counter: &event_counter,
                        swarm_event_tx: &swarm_event_tx,
                    },
                )
                .await;
            }

            Request::CommAssignTask {
                id,
                session_id: req_session_id,
                target_session,
                task_id,
                message,
            } => {
                handle_comm_assign_task(
                    id,
                    req_session_id,
                    target_session,
                    task_id,
                    message,
                    &client_event_tx,
                    &sessions,
                    &soft_interrupt_queues,
                    &client_connections,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_plans,
                    &swarm_coordinators,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &swarm_mutation_runtime,
                )
                .await;
            }

            Request::CommAssignNext {
                id,
                session_id: req_session_id,
                target_session,
                working_dir,
                prefer_spawn,
                spawn_if_needed,
                message,
                model,
                effort,
            } => {
                handle_comm_assign_next(
                    id,
                    req_session_id,
                    target_session,
                    working_dir,
                    prefer_spawn,
                    spawn_if_needed,
                    message,
                    model,
                    effort,
                    &client_event_tx,
                    &sessions,
                    &global_session_id,
                    &provider_template,
                    &soft_interrupt_queues,
                    &client_connections,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_plans,
                    &swarm_coordinators,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &mcp_pool,
                    &swarm_mutation_runtime,
                )
                .await;
            }

            Request::CommTaskControl {
                id,
                session_id: req_session_id,
                action,
                task_id,
                target_session,
                message,
            } => {
                handle_comm_task_control(
                    id,
                    req_session_id,
                    action,
                    task_id,
                    target_session,
                    message,
                    &client_event_tx,
                    &sessions,
                    &soft_interrupt_queues,
                    &client_connections,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_plans,
                    &swarm_coordinators,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &swarm_mutation_runtime,
                )
                .await;
            }

            Request::CommSubscribeChannel {
                id,
                session_id: req_session_id,
                channel,
            } => {
                handle_comm_subscribe_channel(
                    id,
                    req_session_id,
                    channel,
                    &client_event_tx,
                    &swarm_members,
                    &channel_subscriptions,
                    &channel_subscriptions_by_session,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                )
                .await;
            }

            Request::CommUnsubscribeChannel {
                id,
                session_id: req_session_id,
                channel,
            } => {
                handle_comm_unsubscribe_channel(
                    id,
                    req_session_id,
                    channel,
                    &client_event_tx,
                    &swarm_members,
                    &channel_subscriptions,
                    &channel_subscriptions_by_session,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                )
                .await;
            }

            Request::CommAwaitMembers {
                id,
                session_id: req_session_id,
                target_status,
                session_ids: requested_ids,
                mode,
                timeout_secs,
                background,
                notify,
                wake,
            } => {
                handle_comm_await_members(
                    id,
                    req_session_id,
                    target_status,
                    requested_ids,
                    mode,
                    timeout_secs,
                    background,
                    notify,
                    wake,
                    CommAwaitMembersContext {
                        client_event_tx: &client_event_tx,
                        swarm_members: &swarm_members,
                        swarms_by_id: &swarms_by_id,
                        swarm_event_tx: &swarm_event_tx,
                        await_members_runtime: &await_members_runtime,
                    },
                )
                .await;
            }

            // These are handled via channels, not direct requests from TUI
            Request::ClientDebugCommand { id, .. } => {
                handle_client_debug_command(id, &client_event_tx).await;
            }
            Request::ClientDebugResponse { id, output } => {
                handle_client_debug_response(id, output, &client_debug_response_tx);
            }
        }
        if sdk_session_before_request != client_session_id {
            drop(crate::tool::sdk::ConnectionGuard(client_connection_id.clone()));
        }
        if request_lifecycle_logged {
            log_request_lifecycle_handled(
                ServerRequestLifecycleFields {
                    phase: "handled",
                    request_id,
                    request_kind: &request_kind,
                    client_session_id: &client_session_id,
                    client_connection_id: &client_connection_id,
                    client_instance_id: current_client_instance_id.as_deref(),
                    client_is_processing,
                    message_id: processing_message_id,
                    processing_session_id: processing_session_id.as_deref(),
                    line_bytes: line.len(),
                },
                request_lifecycle_start,
                request_decoded_at,
            );
        }
    }

    Ok(())
    }.await;

    drop(_sdk_connection_guard);

    super::client_disconnect_cleanup::record_transport_closed(
        &client_session_id,
        &client_connection_id,
        &client_connections,
    )
    .await;

    // Cleanup serializes the successor decision against live attachment claims.
    // A successor must inherit the active owner, including its completion receiver,
    // rather than just an Agent whose processing task was already aborted.
    let event_handle = if !continue_on_disconnect {
        let retained = crate::hooks::with_client_terminal_env(
            active_terminal_env.clone(),
            cleanup_client_connection(
                &sessions,
                &client_session_id,
                client_is_processing,
                &mut processing_task,
                event_handle,
                &swarm_members,
                &swarms_by_id,
                &swarm_coordinators,
                &swarm_plans,
                &file_touch,
                &channel_subscriptions,
                &channel_subscriptions_by_session,
                &client_debug_state,
                &client_debug_id,
                &client_connections,
                &client_connection_id,
                &shutdown_signals,
                &soft_interrupt_queues,
                &event_history,
                &event_counter,
                &swarm_event_tx,
                &client_event_tx,
                super::client_disconnect_cleanup::IDLE_RECONNECT_GRACE,
            ),
        )
        .await?;
        match retained {
            Some(handle) => handle,
            None => return connection_result,
        }
    } else {
        event_handle
    };

    {
        // Retain the existing turn owner, not the socket. Its JoinHandle and
        // completion receiver stay alive so normal finalization still runs and
        // the daemon cannot idle-shutdown midway through remote work. New
        // attachments receive future events through the existing session fanout.
        detach_client_attachment(
            &client_session_id,
            &client_connection_id,
            &client_debug_id,
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await;
        event_handle.abort();
        drop(reader);
        drop(writer);
        // Input prompts belong to this transport and cannot safely be replayed
        // to a new client. Close response channels instead of waiting forever.
        stdin_forwarder.abort();
        if let Err(error) = stdin_forwarder.await
            && !error.is_cancelled()
        {
            crate::logging::warn(&format!("Client stdin forwarder failed: {error}"));
        }
        stdin_responses.lock().await.clear();
        if let Some(handle) = processing_task.take() {
            crate::logging::info(&format!(
                "Retaining disconnected turn for session {}",
                client_session_id
            ));
            let owner_id = handle.id();
            if let Err(error) = handle.await {
                crate::logging::warn(&format!("Disconnected turn task failed: {error}"));
            }
            while let Ok((done_id, result, report, owner, _finalization)) =
                processing_done_rx.try_recv()
            {
                if Some(done_id) == processing_message_id && owner == owner_id {
                    record_processing_completion(
                        processing_session_id.as_deref(),
                        result,
                        report,
                        &SwarmStatusRefs {
                            members: &swarm_members,
                            swarms_by_id: &swarms_by_id,
                            event_history: &event_history,
                            event_counter: &event_counter,
                            event_tx: &swarm_event_tx,
                        },
                    )
                    .await;
                }
            }
            client_is_processing = false;
        } else {
            // A reattached remote connection may disconnect again while the
            // original lifecycle owns the task. Wait for its active-turn lease
            // before attempting cleanup. Returning early here would leak the
            // session if the original owner had just skipped cleanup for this
            // successor. All finishers serialize cleanup against live attach.
            while crate::turn_cancel_registry::has_active_turn(&client_session_id) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            client_is_processing = false;
        }
    }

    crate::hooks::with_client_terminal_env(
        active_terminal_env,
        cleanup_client_connection(
            &sessions,
            &client_session_id,
            client_is_processing,
            &mut processing_task,
            event_handle,
            &swarm_members,
            &swarms_by_id,
            &swarm_coordinators,
            &swarm_plans,
            &file_touch,
            &channel_subscriptions,
            &channel_subscriptions_by_session,
            &client_debug_state,
            &client_debug_id,
            &client_connections,
            &client_connection_id,
            &shutdown_signals,
            &soft_interrupt_queues,
            &event_history,
            &event_counter,
            &swarm_event_tx,
            &client_event_tx,
            super::client_disconnect_cleanup::IDLE_RECONNECT_GRACE,
        ),
    )
    .await?;
    connection_result
}

#[path = "client_processing.rs"]
mod client_processing;
use client_processing::{
    append_context_message, cancel_processing_message, cancel_soft_interrupts_by_id,
    clear_soft_interrupts, move_tool_to_background, queue_soft_interrupt,
    reconcile_processing_completions, record_processing_completion, start_processing_message,
    try_available_models_snapshot,
};

/// Process a message and stream events (mpsc channel - per-client)
#[cfg(test)]
pub(super) async fn process_message_streaming_mpsc(
    agent: Arc<Mutex<Agent>>,
    content: &str,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    event_tx: tokio::sync::mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    let mut agent = agent.lock().await;
    process_locked_message_streaming_mpsc(&mut agent, content, images, system_reminder, event_tx)
        .await
}

/// Same as [`process_message_streaming_mpsc`] for a caller that already holds
/// the agent lock (e.g. a wake turn that reserved the idle agent up front, see
/// #1152).
#[cfg(test)]
pub(super) async fn process_locked_message_streaming_mpsc(
    agent: &mut Agent,
    content: &str,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    event_tx: tokio::sync::mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    let lease = agent.claim_server_turn()?;
    process_reserved_message_streaming_mpsc(
        agent,
        &lease,
        content,
        images,
        system_reminder,
        event_tx,
    )
    .await
}

/// Execute only with this runtime's non-cloneable turn reservation.
pub(super) async fn process_reserved_message_streaming_mpsc(
    agent: &mut Agent,
    lease: &crate::agent::TurnLease,
    content: &str,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    agent.validate_server_turn(lease)?;
    let session_id = agent.session_id().to_string();
    let result = agent
        .run_reserved_streaming_mpsc(lease, content, images, system_reminder, event_tx, None)
        .await;
    if result.is_ok() {
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "turn_completed",
                "message_turn_finished",
            )
            .with_session_id(session_id)
            .force_attribution(),
        );
        crate::process_memory::release_retained_heap_debounced(
            "server_turn_completed",
            std::time::Duration::from_secs(30),
        );
    }
    result
}

#[cfg(test)]
#[path = "client_lifecycle_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "client_target_attach_tests.rs"]
mod target_attach_tests;
