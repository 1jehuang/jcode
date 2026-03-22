use super::client_actions::{
    handle_agent_task, handle_compact, handle_input_shell, handle_notify_session,
    handle_set_feature, handle_split, handle_stdin_response, handle_trigger_memory_extraction,
};
use super::client_comm::{
    handle_comm_list, handle_comm_message, handle_comm_read, handle_comm_share,
    handle_comm_subscribe_channel, handle_comm_unsubscribe_channel,
};
use super::client_disconnect_cleanup::cleanup_client_connection;
use super::client_session::{
    handle_clear_session, handle_reload, handle_resume_session, handle_subscribe,
};
use super::client_state::{handle_get_history, handle_get_state};
use super::comm_control::{
    handle_client_debug_command, handle_client_debug_response, handle_comm_assign_role,
    handle_comm_assign_task, handle_comm_await_members,
};
use super::comm_plan::{
    handle_comm_approve_plan, handle_comm_propose_plan, handle_comm_reject_plan,
};
use super::comm_session::{handle_comm_spawn, handle_comm_stop};
use super::comm_sync::{handle_comm_read_context, handle_comm_resync_plan, handle_comm_summary};
use super::provider_control::{
    handle_cycle_model, handle_notify_auth_changed, handle_set_compaction_mode, handle_set_model,
    handle_set_premium_mode, handle_set_reasoning_effort, handle_set_service_tier,
    handle_set_transport, handle_switch_anthropic_account, handle_switch_openai_account,
};
use super::{
    ClientConnectionInfo, ClientDebugState, FileAccess, SessionInterruptQueues, SharedContext,
    SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan, broadcast_swarm_status,
    enqueue_soft_interrupt, record_swarm_event, register_session_interrupt_queue, swarm_id_for_dir,
    truncate_detail, update_member_status,
};
use crate::agent::{Agent, InterruptSignal, SoftInterruptQueue, SoftInterruptSource, StreamError};
use crate::bus::{Bus, BusEvent};
use crate::id;
use crate::protocol::{NotificationType, Request, ServerEvent, decode_request, encode_event};
use crate::provider::Provider;
use crate::tool::Registry;
use crate::transport::Stream;
use anyhow::Result;
use futures::FutureExt;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

pub(super) async fn handle_client(
    stream: Stream,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
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
    channel_subscriptions: Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    event_history: Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    server_name: String,
    server_icon: String,
    mcp_pool: Arc<crate::mcp::SharedMcpPool>,
    shutdown_signals: Arc<RwLock<HashMap<String, crate::agent::InterruptSignal>>>,
    soft_interrupt_queues: SessionInterruptQueues,
) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let mut line = String::new();

    // Per-client state
    let mut client_is_processing = false;
    let (processing_done_tx, mut processing_done_rx) =
        mpsc::unbounded_channel::<(u64, Result<()>)>();
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

    {
        let mut connections = client_connections.write().await;
        connections.insert(
            client_connection_id.clone(),
            ClientConnectionInfo {
                client_id: client_connection_id.clone(),
                session_id: client_session_id.clone(),
                debug_client_id: None,
                connected_at,
                last_seen: connected_at,
            },
        );
    }

    {
        let mut current = global_session_id.write().await;
        if current.is_empty() || *current != client_session_id {
            *current = client_session_id.clone();
        }
    }

    // Get a handle to the soft interrupt queue BEFORE wrapping in Mutex
    // This allows queueing interrupts while the agent is processing
    let soft_interrupt_queue = new_agent.soft_interrupt_queue();

    // Get a handle to the background tool signal BEFORE wrapping in Mutex
    // This allows signaling "move to background" while the agent is processing
    let background_tool_signal = new_agent.background_tool_signal();

    // Get a handle to the graceful shutdown signal BEFORE wrapping in Mutex
    // This allows signaling cancel (checkpoint partial response) without needing the lock
    let cancel_signal = new_agent.graceful_shutdown_signal();

    // Register the shutdown signal in the server-level map so
    // graceful_shutdown_sessions can signal it without locking the agent mutex
    {
        let mut signals = shutdown_signals.write().await;
        signals.insert(client_session_id.clone(), cancel_signal.clone());
    }
    register_session_interrupt_queue(
        &soft_interrupt_queues,
        &client_session_id,
        soft_interrupt_queue.clone(),
    )
    .await;

    let agent = Arc::new(Mutex::new(new_agent));
    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.insert(client_session_id.clone(), Arc::clone(&agent));
    }

    // Per-client event channel (not shared with other clients)
    let (client_event_tx, mut client_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ServerEvent>();

    // Get the working directory and swarm id (shared across worktrees)
    let working_dir = std::env::current_dir().ok();
    let swarm_id = if swarm_enabled {
        swarm_id_for_dir(working_dir.clone())
    } else {
        None
    };

    // Register this client as a swarm member
    {
        let mut members = swarm_members.write().await;
        let now = Instant::now();
        members.insert(
            client_session_id.clone(),
            SwarmMember {
                session_id: client_session_id.clone(),
                event_tx: client_event_tx.clone(),
                working_dir: working_dir.clone(),
                swarm_id: swarm_id.clone(),
                swarm_enabled,
                status: "spawned".to_string(),
                detail: None,
                friendly_name: friendly_name.clone(),
                role: "agent".to_string(),
                joined_at: now,
                last_status_change: now,
                is_headless: false,
            },
        );
    }
    // Add to swarm by swarm_id (separate scope to avoid holding swarm_members lock)
    if let Some(ref swarm_id_ref) = swarm_id {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(swarm_id_ref.to_string())
            .or_insert_with(HashSet::new)
            .insert(client_session_id.clone());
        record_swarm_event(
            &event_history,
            &event_counter,
            &swarm_event_tx,
            client_session_id.clone(),
            friendly_name.clone(),
            Some(swarm_id_ref.to_string()),
            SwarmEventType::MemberChange {
                action: "joined".to_string(),
            },
        )
        .await;
    }
    let mut is_new_coordinator = false;
    if let Some(ref swarm_id_ref) = swarm_id {
        let mut coordinators = swarm_coordinators.write().await;
        if coordinators.get(swarm_id_ref.as_str()).is_none() {
            coordinators.insert(swarm_id_ref.to_string(), client_session_id.clone());
            is_new_coordinator = true;
        }
    }
    // Update role separately to avoid nested lock
    if is_new_coordinator {
        let mut members = swarm_members.write().await;
        if let Some(m) = members.get_mut(&client_session_id) {
            m.role = "coordinator".to_string();
        }
    }
    if let Some(ref swarm_id_ref) = swarm_id {
        broadcast_swarm_status(swarm_id_ref, &swarm_members, &swarms_by_id).await;
    }
    update_member_status(
        &client_session_id,
        "ready",
        None,
        &swarm_members,
        &swarms_by_id,
        Some(&event_history),
        Some(&event_counter),
        Some(&swarm_event_tx),
    )
    .await;
    if is_new_coordinator {
        let msg = "You are the coordinator for this swarm.".to_string();
        let _ = client_event_tx.send(ServerEvent::Notification {
            from_session: client_session_id.clone(),
            from_name: friendly_name.clone(),
            notification_type: NotificationType::Message {
                scope: Some("swarm".to_string()),
                channel: None,
            },
            message: msg.clone(),
        });
    }

    // Spawn event forwarder for this client only
    let writer_clone = Arc::clone(&writer);
    let event_handle = tokio::spawn(async move {
        while let Some(event) = client_event_rx.recv().await {
            let json = encode_event(&event);
            let mut w = writer_clone.lock().await;
            if w.write_all(json.as_bytes()).await.is_err() {
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

    loop {
        line.clear();
        tokio::select! {
            // Forward bus events to this client
            bus_event = bus_rx.recv() => {
                match bus_event {
                    Ok(BusEvent::ModelsUpdated) => {
                        let (models, model_routes) = {
                            let agent_guard = agent.lock().await;
                            (
                                agent_guard.available_models_display(),
                                agent_guard.model_routes(),
                            )
                        };
                        let _ = client_event_tx.send(ServerEvent::AvailableModelsUpdated {
                            available_models: models,
                            available_model_routes: model_routes,
                        });
                    }
                    Ok(BusEvent::BatchProgress(progress)) => {
                        if progress.session_id == client_session_id {
                            let _ = client_event_tx.send(ServerEvent::BatchProgress { progress });
                        }
                    }
                    Ok(BusEvent::BackgroundTaskCompleted(ref task)) => {
                        if task.notify && task.session_id == client_session_id {
                            let status_str = match task.status {
                                crate::bus::BackgroundTaskStatus::Completed => "completed",
                                crate::bus::BackgroundTaskStatus::Failed => "failed",
                                crate::bus::BackgroundTaskStatus::Running => "running",
                            };
                            let notification = format!(
                                "[Background Task Completed]\n\
                                 Task: {} ({})\n\
                                 Status: {}\n\
                                 Duration: {:.1}s\n\
                                 Exit code: {}\n\n\
                                 Output preview:\n{}\n\n\
                                 Use `bg action=\"output\" task_id=\"{}\"` for full output.",
                                task.task_id,
                                task.tool_name,
                                status_str,
                                task.duration_secs,
                                task.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "N/A".to_string()),
                                task.output_preview,
                                task.task_id,
                            );
                            let agent_guard = agent.lock().await;
                            agent_guard.queue_soft_interrupt(
                                notification,
                                false,
                                SoftInterruptSource::System,
                            );
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
                if let Some((request_id, command)) = debug_cmd {
                    if client_event_tx
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
                }
                continue;
            }
            done = processing_done_rx.recv() => {
                if let Some((done_id, result)) = done {
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

                    let done_session = processing_session_id.take();
                    match result {
                        Ok(()) => {
                            if let Some(session_id) = done_session.as_deref() {
                                update_member_status(
                                    session_id,
                                    "ready",
                                    None,
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
        }

        let request = match decode_request(&line) {
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
                start_processing_message(
                    id,
                    content,
                    images,
                    system_reminder,
                    &client_session_id,
                    &mut client_is_processing,
                    &mut processing_message_id,
                    &mut processing_session_id,
                    &mut processing_task,
                    &agent,
                    &client_event_tx,
                    &processing_done_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                )
                .await;
            }

            Request::Cancel { id } => {
                let _ = id;
                cancel_processing_message(
                    &mut client_is_processing,
                    &mut processing_message_id,
                    &mut processing_session_id,
                    &mut processing_task,
                    &cancel_signal,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                )
                .await;
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
                    &soft_interrupt_queue,
                    &client_event_tx,
                );
            }

            Request::CancelSoftInterrupts { id } => {
                clear_soft_interrupts(id, &soft_interrupt_queue, &client_event_tx);
            }

            Request::BackgroundTool { id } => {
                move_tool_to_background(id, &background_tool_signal, &client_event_tx);
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
                    &soft_interrupt_queues,
                    &client_connections,
                    &swarm_members,
                    &swarms_by_id,
                    &channel_subscriptions,
                    &swarm_plans,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &client_event_tx,
                )
                .await;
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
            } => {
                handle_subscribe(
                    id,
                    subscribe_working_dir,
                    selfdev,
                    &mut client_selfdev,
                    &client_session_id,
                    &friendly_name,
                    &agent,
                    &registry,
                    &swarm_members,
                    &swarms_by_id,
                    &channel_subscriptions,
                    &swarm_plans,
                    &swarm_coordinators,
                    &client_event_tx,
                    &mcp_pool,
                )
                .await;
            }

            Request::GetHistory { id } => {
                if handle_get_history(
                    id,
                    &client_session_id,
                    &agent,
                    &provider,
                    &sessions,
                    &client_count,
                    &writer,
                    &server_name,
                    &server_icon,
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
                handle_reload(id, &agent, &client_event_tx).await;
            }

            Request::ResumeSession { id, session_id } => {
                if handle_resume_session(
                    id,
                    session_id,
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
                    &swarm_members,
                    &swarms_by_id,
                    &channel_subscriptions,
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
                .await
                .is_err()
                {
                    break;
                }
            }

            Request::CycleModel { id, direction } => {
                handle_cycle_model(id, direction, &agent, &client_event_tx).await;
            }

            Request::SetPremiumMode { id, mode } => {
                handle_set_premium_mode(id, mode, &agent, &client_event_tx).await;
            }

            Request::SetModel { id, model } => {
                handle_set_model(id, model, &agent, &client_event_tx).await;
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
                handle_notify_auth_changed(id, &provider, &agent, &client_event_tx).await;
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
                    &swarm_plans,
                    &client_event_tx,
                )
                .await;
            }

            Request::Split { id } => {
                handle_split(id, &client_session_id, &client_event_tx).await;
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
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
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
                    &sessions,
                    &soft_interrupt_queues,
                    &client_connections,
                    &swarm_members,
                    &client_event_tx,
                )
                .await;
            }

            Request::Transcript {
                id,
                text,
                mode,
                session_id,
            } => {
                let target_matches = session_id
                    .as_deref()
                    .map(|target| target == client_session_id.as_str())
                    .unwrap_or(true);

                if target_matches {
                    let _ = client_event_tx.send(ServerEvent::Transcript { text, mode });
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!(
                            "Transcript target '{}' does not match current client session '{}'.",
                            session_id.unwrap_or_default(),
                            client_session_id
                        ),
                        retry_after_secs: None,
                    });
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
            } => {
                handle_comm_share(
                    id,
                    req_session_id,
                    key,
                    value,
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
            } => {
                handle_comm_message(
                    id,
                    from_session,
                    message,
                    to_session,
                    channel,
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
                    &file_touches,
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
                )
                .await;
            }

            Request::CommSpawn {
                id,
                session_id: req_session_id,
                working_dir,
                initial_message,
            } => {
                handle_comm_spawn(
                    id,
                    req_session_id,
                    working_dir,
                    initial_message,
                    &client_event_tx,
                    &sessions,
                    &global_session_id,
                    &provider_template,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &swarm_plans,
                    &channel_subscriptions,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &mcp_pool,
                    &soft_interrupt_queues,
                )
                .await;
            }

            Request::CommStop {
                id,
                session_id: req_session_id,
                target_session,
            } => {
                handle_comm_stop(
                    id,
                    req_session_id,
                    target_session,
                    &client_event_tx,
                    &sessions,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &swarm_plans,
                    &channel_subscriptions,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    &soft_interrupt_queues,
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
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                )
                .await;
            }

            Request::CommSummary {
                id,
                session_id: _req_session_id,
                target_session,
                limit,
            } => {
                handle_comm_summary(id, target_session, limit, &sessions, &client_event_tx).await;
            }

            Request::CommReadContext {
                id,
                session_id: _req_session_id,
                target_session,
            } => {
                handle_comm_read_context(id, target_session, &sessions, &client_event_tx).await;
            }

            Request::CommResyncPlan {
                id,
                session_id: req_session_id,
            } => {
                handle_comm_resync_plan(
                    id,
                    req_session_id,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_plans,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
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
                timeout_secs,
            } => {
                handle_comm_await_members(
                    id,
                    req_session_id,
                    target_status,
                    requested_ids,
                    timeout_secs,
                    &client_event_tx,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_event_tx,
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
        &channel_subscriptions,
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

#[allow(clippy::too_many_arguments)]
async fn start_processing_message(
    id: u64,
    content: String,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    client_session_id: &str,
    client_is_processing: &mut bool,
    processing_message_id: &mut Option<u64>,
    processing_session_id: &mut Option<String>,
    processing_task: &mut Option<tokio::task::JoinHandle<()>>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    processing_done_tx: &mpsc::UnboundedSender<(u64, Result<()>)>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    if *client_is_processing {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Already processing a message".to_string(),
            retry_after_secs: None,
        });
        return;
    }

    *client_is_processing = true;
    *processing_message_id = Some(id);
    *processing_session_id = Some(client_session_id.to_string());

    update_member_status(
        client_session_id,
        "running",
        Some(truncate_detail(&content, 120)),
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;

    let agent = Arc::clone(agent);
    let tx = client_event_tx.clone();
    let done_tx = processing_done_tx.clone();
    crate::logging::info(&format!("Processing message id={} spawning task", id));
    *processing_task = Some(tokio::spawn(async move {
        let result = match std::panic::AssertUnwindSafe(process_message_streaming_mpsc(
            agent,
            &content,
            images,
            system_reminder,
            tx,
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
        let _ = done_tx.send((id, result));
    }));
}

#[allow(clippy::too_many_arguments)]
async fn cancel_processing_message(
    client_is_processing: &mut bool,
    processing_message_id: &mut Option<u64>,
    processing_session_id: &mut Option<String>,
    processing_task: &mut Option<tokio::task::JoinHandle<()>>,
    cancel_signal: &InterruptSignal,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    if let Some(mut handle) = processing_task.take() {
        if handle.is_finished() {
            *processing_task = Some(handle);
            return;
        }
        cancel_signal.fire();
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
        cancel_signal.reset();
        *processing_task = None;
        *client_is_processing = false;
        if let Some(session_id) = processing_session_id.take() {
            update_member_status(
                &session_id,
                "stopped",
                Some("cancelled".to_string()),
                swarm_members,
                swarms_by_id,
                Some(event_history),
                Some(event_counter),
                Some(swarm_event_tx),
            )
            .await;
        }
        if let Some(message_id) = processing_message_id.take() {
            let _ = client_event_tx.send(ServerEvent::Interrupted);
            let _ = client_event_tx.send(ServerEvent::Done { id: message_id });
        }
    }
}

fn queue_soft_interrupt(
    id: u64,
    content: String,
    urgent: bool,
    source: SoftInterruptSource,
    soft_interrupt_queue: &SoftInterruptQueue,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let _ = enqueue_soft_interrupt(soft_interrupt_queue, content, urgent, source);
    let _ = client_event_tx.send(ServerEvent::Ack { id });
}

fn clear_soft_interrupts(
    id: u64,
    soft_interrupt_queue: &SoftInterruptQueue,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Ok(mut queue) = soft_interrupt_queue.lock() {
        queue.clear();
    }
    let _ = client_event_tx.send(ServerEvent::Ack { id });
}

fn move_tool_to_background(
    id: u64,
    background_tool_signal: &InterruptSignal,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    background_tool_signal.fire();
    let _ = client_event_tx.send(ServerEvent::Ack { id });
}

/// Process a message and stream events (broadcast channel - deprecated)
#[allow(dead_code)]
pub(super) async fn process_message_streaming(
    agent: Arc<Mutex<Agent>>,
    content: &str,
    event_tx: broadcast::Sender<ServerEvent>,
) -> Result<()> {
    let mut agent = agent.lock().await;
    agent.run_once_streaming(content, event_tx).await
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
    agent
        .run_once_streaming_mpsc(content, images, system_reminder, event_tx)
        .await
}
