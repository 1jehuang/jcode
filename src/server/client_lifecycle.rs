use super::client_actions::{
    AgentTaskContext, NotifySessionContext, handle_agent_task, handle_compact, handle_input_shell,
    handle_notify_session, handle_run_subagent, handle_set_feature, handle_set_subagent_model,
    handle_split, handle_stdin_response, handle_transfer, handle_trigger_memory_extraction,
};
use super::client_comm::{
    handle_comm_channel_members, handle_comm_list, handle_comm_list_channels, handle_comm_message,
    handle_comm_read, handle_comm_share, handle_comm_subscribe_channel,
    handle_comm_unsubscribe_channel,
};
use super::client_disconnect_cleanup::cleanup_client_connection;
use super::client_session::{
    handle_clear_session, handle_reload, handle_resume_session, handle_subscribe,
};
use super::client_state::{handle_get_compacted_history, handle_get_history, handle_get_state};
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
    handle_cycle_model, handle_notify_auth_changed, handle_refresh_models,
    handle_set_compaction_mode, handle_set_model, handle_set_premium_mode,
    handle_set_reasoning_effort, handle_set_service_tier, handle_set_transport,
    handle_switch_anthropic_account, handle_switch_openai_account,
    try_available_models_updated_event,
};
use super::{
    AwaitMembersRuntime, ClientConnectionInfo, ClientDebugState, FileAccess, SessionControlHandle,
    SessionInterruptQueues, SharedContext, SwarmEvent, SwarmMember, SwarmMutationRuntime,
    VersionedPlan, format_structured_completion_report, register_session_interrupt_queue,
    truncate_detail, update_member_status, update_member_status_with_report,
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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;
const RELOAD_STARTING_GUARD_MAX_AGE: Duration = Duration::from_secs(30);

struct ProcessingMessage {
    id: u64,
    content: String,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
}

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

fn server_reload_starting() -> bool {
    matches!(
        crate::server::recent_reload_state(RELOAD_STARTING_GUARD_MAX_AGE),
        Some(state) if state.phase == crate::server::ReloadPhase::Starting
    )
}

fn is_lightweight_control_request(request: &Request) -> bool {
    matches!(
        request,
        Request::Ping { .. }
            | Request::CommShare { .. }
            | Request::CommRead { .. }
            | Request::CommMessage { .. }
            | Request::CommList { .. }
            | Request::CommListChannels { .. }
            | Request::CommChannelMembers { .. }
            | Request::CommProposePlan { .. }
            | Request::CommApprovePlan { .. }
            | Request::CommRejectPlan { .. }
            | Request::CommSpawn { .. }
            | Request::CommStop { .. }
            | Request::CommAssignRole { .. }
            | Request::CommSummary { .. }
            | Request::CommStatus { .. }
            | Request::CommReport { .. }
            | Request::CommPlanStatus { .. }
            | Request::CommReadContext { .. }
            | Request::CommResyncPlan { .. }
            | Request::CommAssignTask { .. }
            | Request::CommAssignNext { .. }
            | Request::CommTaskControl { .. }
            | Request::CommSubscribeChannel { .. }
            | Request::CommUnsubscribeChannel { .. }
            | Request::CommAwaitMembers { .. }
    )
}

async fn write_direct_event(
    writer: &Arc<Mutex<crate::transport::WriteHalf>>,
    event: &ServerEvent,
) -> Result<()> {
    let json = encode_event(event);
    let mut w = writer.lock().await;
    w.write_all(json.as_bytes()).await?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "lightweight comm dispatch still needs access to the same shared swarm/session state"
)]
async fn handle_lightweight_control_request(
    request: Request,
    writer: Arc<Mutex<crate::transport::WriteHalf>>,
    sessions: &SessionAgents,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn Provider>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: &Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    soft_interrupt_queues: &SessionInterruptQueues,
    await_members_runtime: &AwaitMembersRuntime,
    swarm_mutation_runtime: &SwarmMutationRuntime,
) -> Result<()> {
    if let Request::Ping { id } = request {
        write_direct_event(&writer, &ServerEvent::Pong { id }).await?;
        return Ok(());
    }

    write_direct_event(&writer, &ServerEvent::Ack { id: request.id() }).await?;

    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let writer_clone = Arc::clone(&writer);
    let event_handle = tokio::spawn(async move {
        while let Some(event) = client_event_rx.recv().await {
            if let Err(error) = write_direct_event(&writer_clone, &event).await {
                crate::logging::warn(&format!(
                    "lightweight control writer failed while sending {:?}: {}",
                    event, error
                ));
                break;
            }
        }
    });

    match request {
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
                swarm_members,
                swarms_by_id,
                shared_context,
                event_history,
                event_counter,
                swarm_event_tx,
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
                swarm_members,
                shared_context,
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
        } => {
            handle_comm_message(
                id,
                from_session,
                message,
                to_session,
                channel,
                delivery,
                wake,
                &client_event_tx,
                sessions,
                soft_interrupt_queues,
                swarm_members,
                swarms_by_id,
                channel_subscriptions,
                event_history,
                event_counter,
                swarm_event_tx,
                client_connections,
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
                swarm_members,
                swarms_by_id,
                files_touched_by_session,
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
                swarm_members,
                channel_subscriptions,
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
                swarm_members,
                channel_subscriptions,
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
                swarm_members,
                swarms_by_id,
                shared_context,
                swarm_plans,
                swarm_coordinators,
                sessions,
                soft_interrupt_queues,
                event_history,
                event_counter,
                swarm_event_tx,
                swarm_mutation_runtime,
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
                swarm_members,
                swarms_by_id,
                shared_context,
                swarm_plans,
                swarm_coordinators,
                sessions,
                soft_interrupt_queues,
                event_history,
                event_counter,
                swarm_event_tx,
                swarm_mutation_runtime,
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
                swarm_members,
                shared_context,
                swarm_coordinators,
                sessions,
                soft_interrupt_queues,
                event_history,
                event_counter,
                swarm_event_tx,
                swarm_mutation_runtime,
            )
            .await;
        }
        Request::CommSpawn {
            id,
            session_id: req_session_id,
            working_dir,
            initial_message,
            request_nonce,
        } => {
            handle_comm_spawn(
                id,
                req_session_id,
                working_dir,
                initial_message,
                request_nonce,
                &client_event_tx,
                sessions,
                global_session_id,
                provider_template,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
                channel_subscriptions,
                channel_subscriptions_by_session,
                event_history,
                event_counter,
                swarm_event_tx,
                mcp_pool,
                soft_interrupt_queues,
                swarm_mutation_runtime,
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
                sessions,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
                channel_subscriptions,
                channel_subscriptions_by_session,
                event_history,
                event_counter,
                swarm_event_tx,
                soft_interrupt_queues,
                swarm_mutation_runtime,
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
                sessions,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
                event_history,
                event_counter,
                swarm_event_tx,
                swarm_mutation_runtime,
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
                sessions,
                swarm_members,
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
                sessions,
                swarm_members,
                client_connections,
                files_touched_by_session,
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
        } => {
            let status = status.unwrap_or_else(|| "ready".to_string());
            let report = format_structured_completion_report(
                &message,
                validation.as_deref(),
                follow_up.as_deref(),
            );
            let detail = Some(truncate_detail(&message, 160));
            update_member_status_with_report(
                &req_session_id,
                &status,
                detail,
                Some(report.clone()),
                swarm_members,
                swarms_by_id,
                Some(event_history),
                Some(event_counter),
                Some(swarm_event_tx),
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
                swarm_members,
                swarm_plans,
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
                sessions,
                swarm_members,
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
                    swarm_members,
                    swarms_by_id,
                    swarm_plans,
                    swarm_coordinators,
                    event_history,
                    event_counter,
                    swarm_event_tx,
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
                sessions,
                soft_interrupt_queues,
                client_connections,
                swarm_members,
                swarms_by_id,
                swarm_plans,
                swarm_coordinators,
                event_history,
                event_counter,
                swarm_event_tx,
                swarm_mutation_runtime,
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
        } => {
            handle_comm_assign_next(
                id,
                req_session_id,
                target_session,
                working_dir,
                prefer_spawn,
                spawn_if_needed,
                message,
                &client_event_tx,
                sessions,
                global_session_id,
                provider_template,
                soft_interrupt_queues,
                client_connections,
                swarm_members,
                swarms_by_id,
                swarm_plans,
                swarm_coordinators,
                event_history,
                event_counter,
                swarm_event_tx,
                mcp_pool,
                swarm_mutation_runtime,
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
                sessions,
                soft_interrupt_queues,
                client_connections,
                swarm_members,
                swarms_by_id,
                swarm_plans,
                swarm_coordinators,
                event_history,
                event_counter,
                swarm_event_tx,
                swarm_mutation_runtime,
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
                swarm_members,
                channel_subscriptions,
                channel_subscriptions_by_session,
                event_history,
                event_counter,
                swarm_event_tx,
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
                swarm_members,
                channel_subscriptions,
                channel_subscriptions_by_session,
                event_history,
                event_counter,
                swarm_event_tx,
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
        } => {
            handle_comm_await_members(
                id,
                req_session_id,
                target_status,
                requested_ids,
                mode,
                timeout_secs,
                CommAwaitMembersContext {
                    client_event_tx: &client_event_tx,
                    swarm_members,
                    swarms_by_id,
                    swarm_event_tx,
                    await_members_runtime,
                },
            )
            .await;
        }
        other => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id: other.id(),
                message: "unsupported lightweight control request".to_string(),
                retry_after_secs: None,
            });
        }
    }

    drop(client_event_tx);
    let _ = event_handle.await;
    Ok(())
}

async fn refresh_session_control_handle(
    session_id: &str,
    agent: &Arc<Mutex<Agent>>,
) -> SessionControlHandle {
    let agent_guard = agent.lock().await;
    SessionControlHandle::new(
        session_id,
        agent_guard.soft_interrupt_queue(),
        agent_guard.background_tool_signal(),
        agent_guard.graceful_shutdown_signal(),
    )
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
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
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
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let mut line = String::new();

    let initial_request = loop {
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
                if is_lightweight_control_request(&request) {
                    handle_lightweight_control_request(
                        request,
                        Arc::clone(&writer),
                        &sessions,
                        &global_session_id,
                        &provider_template,
                        &swarm_members,
                        &swarms_by_id,
                        &shared_context,
                        &swarm_plans,
                        &swarm_coordinators,
                        &files_touched_by_session,
                        &channel_subscriptions,
                        &channel_subscriptions_by_session,
                        &client_connections,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                        &mcp_pool,
                        &soft_interrupt_queues,
                        &await_members_runtime,
                        &swarm_mutation_runtime,
                    )
                    .await?;
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

    // Per-client state
    let mut client_is_processing = false;
    let (processing_done_tx, mut processing_done_rx) =
        mpsc::unbounded_channel::<(u64, Result<()>, Option<String>)>();
    let mut processing_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut processing_message_id: Option<u64> = None;
    let mut processing_session_id: Option<String> = None;
    // Client selfdev status is determined by Subscribe request, not server's env
    let mut client_selfdev = false;

    let client_start = std::time::Instant::now();

    let provider = provider_template.fork();
    let t0 = std::time::Instant::now();
    let registry = Registry::new(provider.clone()).await;
    let registry_ms = t0.elapsed().as_millis();

    let mut swarm_enabled = crate::config::config().features.swarm;
    let mut last_available_models_snapshot: Option<String> = None;
    const MAX_LIVE_AVAILABLE_MODELS_UPDATE_BYTES: usize = 64 * 1024;

    // Create a new session for this client
    let t0 = std::time::Instant::now();
    let mut new_agent = Agent::new(Arc::clone(&provider), registry.clone());
    let agent_new_ms = t0.elapsed().as_millis();

    new_agent.set_memory_enabled(crate::config::config().features.memory);

    crate::logging::info(&format!(
        "[TIMING] handle_client setup: registry={registry_ms}ms, agent_new={agent_new_ms}ms, total={}ms",
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
    );

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
    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.insert(client_session_id.clone(), Arc::clone(&agent));
    }
    crate::runtime_memory_log::emit_event(
        crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
            "session_created",
            "new_live_session_attached",
        )
        .with_session_id(client_session_id.clone())
        .force_attribution(),
    );

    // Per-client event channel (not shared with other clients)
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
                    match &event {
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
            let json = encode_event(&event);
            let mut w = writer_clone.lock().await;
            if let Err(error) = w.write_all(json.as_bytes()).await {
                crate::logging::warn(&format!(
                    "event_forwarder write failed for connection {} while sending {:?}: {}",
                    client_connection_id_for_events, event, error
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
        agent_guard.set_stdin_request_tx(stdin_req_tx);
    }
    let _stdin_forwarder = {
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
    let mut pending_request = Some(initial_request);

    loop {
        let request = if let Some(request) = pending_request.take() {
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
                if let Some((done_id, result, completion_report)) = done {
                    if Some(done_id) != processing_message_id {
                        crate::logging::warn(&format!(
                            "Done event id={} doesn't match processing_message_id={:?}, dropping",
                            done_id, processing_message_id
                        ));
                        continue;
                    }
                    crate::logging::info(&format!(
                        "Processing done for message id={}, result={}",
                        done_id,
                        if result.is_ok() { "ok" } else { "err" }
                    ));
                    processing_message_id = None;
                    processing_task = None;
                    client_is_processing = false;
                    {
                        let mut connections = client_connections.write().await;
                        if let Some(info) = connections.get_mut(&client_connection_id) {
                            info.is_processing = false;
                            info.current_tool_name = None;
                        }
                    }

                    let done_session = processing_session_id.take();
                    match result {
                        Ok(()) => {
                            if let Some(session_id) = done_session.as_deref() {
                                update_member_status_with_report(
                                    session_id,
                                    "ready",
                                    None,
                                    completion_report,
                                    &swarm_members,
                                    &swarms_by_id,
                                    Some(&event_history),
                                    Some(&event_counter),
                                    Some(&swarm_event_tx),
                                )
                                .await;
                            }
                            let _ = client_event_tx.send(ServerEvent::Done { id: done_id });
                        }
                        Err(e) => {
                            if let Some(session_id) = done_session.as_deref() {
                                update_member_status(
                                    session_id,
                                    "failed",
                                    Some(truncate_detail(&e.to_string(), 120)),
                                    &swarm_members,
                                    &swarms_by_id,
                                    Some(&event_history),
                                    Some(&event_counter),
                                    Some(&swarm_event_tx),
                                )
                                .await;
                            }
                            let retry_after_secs = e.downcast_ref::<StreamError>().and_then(|se| se.retry_after_secs);
                            if retry_after_secs.is_some() {
                                crate::telemetry::record_error(crate::telemetry::ErrorCategory::RateLimited);
                            } else {
                                let msg = e.to_string().to_lowercase();
                                if msg.contains("timeout") {
                                    crate::telemetry::record_error(crate::telemetry::ErrorCategory::ProviderTimeout);
                                } else if msg.contains("auth") || msg.contains("unauthorized") || msg.contains("forbidden") {
                                    crate::telemetry::record_error(crate::telemetry::ErrorCategory::AuthFailed);
                                }
                            }
                            let _ = client_event_tx.send(ServerEvent::Error {
                                id: done_id,
                                message: crate::util::format_error_chain(&e),
                                retry_after_secs,
                            });
                        }
                    }
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
                    Ok(BusEvent::ModelsUpdated) => {
                        let Some(event) = try_available_models_updated_event(&agent) else {
                            crate::logging::info(&format!(
                                "Skipping ModelsUpdated push for busy connection {}",
                                client_connection_id
                            ));
                            continue;
                        };
                        let encoded_event = crate::protocol::encode_event(&event);
                        if last_available_models_snapshot.as_ref() == Some(&encoded_event) {
                            continue;
                        }
                        let encoded_len = encoded_event.len();
                        if encoded_len > MAX_LIVE_AVAILABLE_MODELS_UPDATE_BYTES {
                            crate::logging::warn(&format!(
                                "Skipping oversized bus AvailableModelsUpdated frame for connection {} ({} bytes)",
                                client_connection_id, encoded_len
                            ));
                            last_available_models_snapshot = Some(encoded_event);
                            continue;
                        }
                        let _ = client_event_tx.send(event);
                        last_available_models_snapshot = Some(encoded_event);
                    }
                    Ok(BusEvent::BatchProgress(progress)) => {
                        if progress.session_id == client_session_id {
                            let _ = client_event_tx.send(ServerEvent::BatchProgress { progress });
                        }
                    }
                    Ok(BusEvent::SidePanelUpdated(update)) => {
                        if update.session_id == client_session_id {
                            let _ = client_event_tx.send(ServerEvent::SidePanelState {
                                snapshot: update.snapshot,
                            });
                        }
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

        // Send ack
        let ack = ServerEvent::Ack { id: request.id() };
        let json = encode_event(&ack);
        {
            let mut w = writer.lock().await;
            if w.write_all(json.as_bytes()).await.is_err() {
                break;
            }
        }

        match request {
            Request::Message {
                id,
                content,
                images,
                system_reminder,
            } => {
                if !client_is_processing {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.is_processing = true;
                        info.current_tool_name = None;
                    }
                }
                start_processing_message(
                    ProcessingMessage {
                        id,
                        content,
                        images,
                        system_reminder,
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

            Request::Cancel { id } => {
                let _ = id;
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
                urgent,
            } => {
                queue_soft_interrupt(
                    id,
                    content,
                    urgent,
                    SoftInterruptSource::User,
                    &session_control,
                    &client_event_tx,
                );
            }

            Request::CancelSoftInterrupts { id } => {
                clear_soft_interrupts(id, &client_session_id, &session_control, &client_event_tx);
            }

            Request::BackgroundTool { id } => {
                move_tool_to_background(id, &session_control, &client_event_tx);
            }

            Request::Clear { id } => {
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
                    &file_touches,
                    &files_touched_by_session,
                    &channel_subscriptions,
                    &channel_subscriptions_by_session,
                    &swarm_plans,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &client_event_tx,
                )
                .await;
                session_control = refresh_session_control_handle(&client_session_id, &agent).await;
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
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
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
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
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
                let json = encode_event(&ServerEvent::Pong { id });
                let mut w = writer.lock().await;
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
            }

            Request::GetState { id } => {
                if handle_get_state(
                    id,
                    &client_session_id,
                    client_is_processing,
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
                working_dir: subscribe_working_dir,
                selfdev,
                target_session_id,
                client_instance_id,
                client_has_local_history,
                allow_session_takeover,
            } => {
                {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.client_instance_id = client_instance_id.clone();
                    }
                }
                if let Some(target_session_id) = target_session_id {
                    if crate::session::session_exists(&target_session_id) {
                        let pre_resume_session_id = client_session_id.clone();
                        agent = handle_resume_session(
                            id,
                            target_session_id.clone(),
                            client_instance_id.as_deref(),
                            client_has_local_history,
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
                            &file_touches,
                            &files_touched_by_session,
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
                        )
                        .await?;
                        session_control =
                            refresh_session_control_handle(&client_session_id, &agent).await;
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
                            )
                            .await;
                            if let Some(snapshot) = try_available_models_snapshot(&agent) {
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
                        )
                        .await;
                    }
                } else {
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
                    )
                    .await;
                    if let Some(snapshot) = try_available_models_snapshot(&agent) {
                        last_available_models_snapshot = Some(snapshot);
                    }
                }
                client_subscribed = true;
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
                )
                .await
                .is_err()
                {
                    break;
                }
                if let Some(snapshot) = try_available_models_snapshot(&agent) {
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

            Request::Reload { id } => {
                handle_reload(id, &agent, &swarm_members, &client_event_tx).await;
            }

            Request::ResumeSession {
                id,
                session_id,
                client_instance_id,
                client_has_local_history,
                allow_session_takeover,
            } => {
                {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.client_instance_id = client_instance_id.clone();
                    }
                }
                agent = handle_resume_session(
                    id,
                    session_id,
                    client_instance_id.as_deref(),
                    client_has_local_history,
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
                    &file_touches,
                    &files_touched_by_session,
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
                )
                .await?;
                session_control = refresh_session_control_handle(&client_session_id, &agent).await;
                if let Some(snapshot) = try_available_models_snapshot(&agent) {
                    last_available_models_snapshot = Some(snapshot);
                }
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

            Request::SetSubagentModel { id, model } => {
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

            Request::SetReasoningEffort { id, effort } => {
                handle_set_reasoning_effort(id, effort, &agent, &client_event_tx).await;
            }

            Request::SetServiceTier { id, service_tier } => {
                handle_set_service_tier(id, service_tier, &agent, &client_event_tx).await;
            }

            Request::SetTransport { id, transport } => {
                handle_set_transport(id, transport, &agent, &client_event_tx).await;
            }

            Request::SetCompactionMode { id, mode } => {
                handle_set_compaction_mode(id, mode, &agent, &client_event_tx).await;
            }

            Request::NotifyAuthChanged { id } => {
                handle_notify_auth_changed(
                    id,
                    &provider,
                    &provider_template,
                    &sessions,
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

            Request::SetFeature {
                id,
                feature,
                enabled,
            } => {
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
                handle_split(id, &client_session_id, &client_event_tx).await;
            }

            Request::Transfer { id } => {
                handle_transfer(id, &client_session_id, &agent, &client_event_tx).await;
            }

            Request::Compact { id } => {
                handle_compact(id, &agent, &client_event_tx);
            }

            Request::TriggerMemoryExtraction { id } => {
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
                        client_event_tx: &client_event_tx,
                    },
                )
                .await;
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
                        let _ = client_event_tx.send(event);
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
            } => {
                handle_comm_message(
                    id,
                    from_session,
                    message,
                    to_session,
                    channel,
                    delivery,
                    wake,
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
                    &files_touched_by_session,
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
                working_dir,
                initial_message,
                request_nonce,
            } => {
                handle_comm_spawn(
                    id,
                    req_session_id,
                    working_dir,
                    initial_message,
                    request_nonce,
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
                    &files_touched_by_session,
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
            } => {
                let status = status.unwrap_or_else(|| "ready".to_string());
                let report = format_structured_completion_report(
                    &message,
                    validation.as_deref(),
                    follow_up.as_deref(),
                );
                let detail = Some(truncate_detail(&message, 160));
                update_member_status_with_report(
                    &req_session_id,
                    &status,
                    detail,
                    Some(report),
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                    Some(&swarm_event_tx),
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
            } => {
                handle_comm_assign_next(
                    id,
                    req_session_id,
                    target_session,
                    working_dir,
                    prefer_spawn,
                    spawn_if_needed,
                    message,
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
            } => {
                handle_comm_await_members(
                    id,
                    req_session_id,
                    target_status,
                    requested_ids,
                    mode,
                    timeout_secs,
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
    }

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
        &file_touches,
        &files_touched_by_session,
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
    )
    .await?;
    Ok(())
}

async fn start_processing_message(
    message: ProcessingMessage,
    client_session_id: &str,
    state: &mut ProcessingState<'_>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    processing_done_tx: &mpsc::UnboundedSender<(u64, Result<()>, Option<String>)>,
    swarm: &SwarmStatusRefs<'_>,
) {
    let ProcessingMessage {
        id,
        content,
        images,
        system_reminder,
    } = message;
    if server_reload_starting() {
        crate::logging::info(&format!(
            "Rejecting new message for session {} because server reload is starting",
            client_session_id
        ));
        let _ = client_event_tx.send(ServerEvent::Reloading { new_socket: None });
        return;
    }

    if *state.client_is_processing {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Already processing a message".to_string(),
            retry_after_secs: None,
        });
        return;
    }

    *state.client_is_processing = true;
    *state.message_id = Some(id);
    *state.session_id = Some(client_session_id.to_string());

    update_member_status(
        client_session_id,
        "running",
        Some(truncate_detail(&content, 120)),
        swarm.members,
        swarm.swarms_by_id,
        Some(swarm.event_history),
        Some(swarm.event_counter),
        Some(swarm.event_tx),
    )
    .await;

    let start_message_index = {
        let agent_guard = agent.lock().await;
        agent_guard.message_count()
    };
    let agent = Arc::clone(agent);
    let report_agent = Arc::clone(&agent);
    let tx = client_event_tx.clone();
    let done_tx = processing_done_tx.clone();
    crate::logging::info(&format!("Processing message id={} spawning task", id));
    *state.task = Some(tokio::spawn(async move {
        let event_tx = tx.clone();
        let result = match std::panic::AssertUnwindSafe(process_message_streaming_mpsc(
            agent,
            &content,
            images,
            system_reminder,
            event_tx,
        ))
        .catch_unwind()
        .await
        {
            Ok(result) => result,
            Err(panic_payload) => {
                let msg = if let Some(text) = panic_payload.downcast_ref::<&str>() {
                    text.to_string()
                } else if let Some(text) = panic_payload.downcast_ref::<String>() {
                    text.clone()
                } else {
                    "unknown panic".to_string()
                };
                crate::logging::error(&format!(
                    "Processing task PANICKED for message id={}: {}",
                    id, msg
                ));
                Err(anyhow::anyhow!("Processing task panicked: {}", msg))
            }
        };
        match &result {
            Ok(()) => crate::logging::info(&format!(
                "Processing task completed OK for message id={}",
                id
            )),
            Err(error) => crate::logging::warn(&format!(
                "Processing task completed with error for message id={}: {}",
                id, error
            )),
        }
        let completion_report = if result.is_ok() {
            let agent = report_agent.lock().await;
            agent.latest_assistant_text_after(start_message_index)
        } else {
            None
        };
        let _ = done_tx.send((id, result, completion_report));
    }));
}

async fn cancel_processing_message(
    state: &mut ProcessingState<'_>,
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm: &SwarmStatusRefs<'_>,
) {
    if let Some(mut handle) = state.task.take() {
        if handle.is_finished() {
            *state.task = Some(handle);
            return;
        }
        session_control.request_cancel();
        match tokio::time::timeout(std::time::Duration::from_millis(500), &mut handle).await {
            Ok(_) => {}
            Err(_) => {
                handle.abort();
                match tokio::time::timeout(std::time::Duration::from_millis(2000), handle).await {
                    Ok(_) => crate::logging::info("Aborted processing task released resources"),
                    Err(_) => crate::logging::warn(
                        "Aborted processing task did not release resources within 2s",
                    ),
                }
            }
        }
        session_control.reset_cancel();
        *state.task = None;
        *state.client_is_processing = false;
        if let Some(session_id) = state.session_id.take() {
            update_member_status(
                &session_id,
                "stopped",
                Some("cancelled".to_string()),
                swarm.members,
                swarm.swarms_by_id,
                Some(swarm.event_history),
                Some(swarm.event_counter),
                Some(swarm.event_tx),
            )
            .await;
        }
        if let Some(message_id) = state.message_id.take() {
            let _ = client_event_tx.send(ServerEvent::Interrupted);
            let _ = client_event_tx.send(ServerEvent::Done { id: message_id });
        }
    }
}

fn try_available_models_snapshot(agent: &Arc<Mutex<Agent>>) -> Option<String> {
    let event = try_available_models_updated_event(agent)?;
    Some(crate::protocol::encode_event(&event))
}

fn queue_soft_interrupt(
    id: u64,
    content: String,
    urgent: bool,
    source: SoftInterruptSource,
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let _ = session_control.queue_soft_interrupt(content, urgent, source);
    let _ = client_event_tx.send(ServerEvent::Ack { id });
}

fn clear_soft_interrupts(
    id: u64,
    session_id: &str,
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    session_control.clear_soft_interrupts();
    if let Err(err) = crate::soft_interrupt_store::clear(session_id) {
        crate::logging::warn(&format!(
            "Failed to clear persisted soft interrupts for {}: {}",
            session_id, err
        ));
    }
    let _ = client_event_tx.send(ServerEvent::Ack { id });
}

fn move_tool_to_background(
    id: u64,
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    session_control.request_background_current_tool();
    let _ = client_event_tx.send(ServerEvent::Ack { id });
}

/// Process a message and stream events (mpsc channel - per-client)
pub(super) async fn process_message_streaming_mpsc(
    agent: Arc<Mutex<Agent>>,
    content: &str,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    event_tx: tokio::sync::mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    let mut agent = agent.lock().await;
    let session_id = agent.session_id().to_string();
    let result = agent
        .run_once_streaming_mpsc(content, images, system_reminder, event_tx)
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
    }
    result
}

#[cfg(test)]
#[path = "client_lifecycle_tests.rs"]
mod tests;
