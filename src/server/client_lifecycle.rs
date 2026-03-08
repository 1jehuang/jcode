use super::client_comm::{
    handle_comm_list, handle_comm_read, handle_comm_share, handle_comm_subscribe_channel,
    handle_comm_unsubscribe_channel,
};
use super::provider_control::{
    handle_cycle_model, handle_notify_auth_changed, handle_set_model, handle_set_premium_mode,
    handle_switch_anthropic_account,
};
use super::reload::{do_server_reload_with_progress, normalize_model_arg, provider_cli_arg};
use super::{
    broadcast_swarm_plan, broadcast_swarm_status, create_headless_session, is_jcode_repo_or_parent,
    is_selfdev_env, record_swarm_event, record_swarm_event_for_session, remove_plan_participant,
    remove_session_from_swarm, rename_plan_participant, server_has_newer_binary, socket_path,
    summarize_plan_items, swarm_id_for_dir, truncate_detail, update_member_status,
    ClientConnectionInfo, ClientDebugState, FileAccess, SharedContext, SwarmEvent, SwarmEventType,
    SwarmMember, VersionedPlan,
};
use crate::agent::{Agent, StreamError};
use crate::bus::{Bus, BusEvent};
use crate::id;
use crate::plan::PlanItem;
use crate::protocol::{
    decode_request, encode_event, FeatureToggle, NotificationType, Request, ServerEvent,
};
use crate::provider::Provider;
use crate::session::Session;
use crate::tool::Registry;
use crate::transport::Stream;
use anyhow::Result;
use futures::FutureExt;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

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

    let provider = provider_template.fork();
    let registry = Registry::new(provider.clone()).await;
    let mut swarm_enabled = crate::config::config().features.swarm;

    // Create a new session for this client
    let mut new_agent = Agent::new(Arc::clone(&provider), registry.clone());
    new_agent.set_memory_enabled(crate::config::config().features.memory);
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
    if let Some(ref id) = swarm_id {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(id.clone())
            .or_insert_with(HashSet::new)
            .insert(client_session_id.clone());
        record_swarm_event(
            &event_history,
            &event_counter,
            &swarm_event_tx,
            client_session_id.clone(),
            friendly_name.clone(),
            Some(id.clone()),
            SwarmEventType::MemberChange {
                action: "joined".to_string(),
            },
        )
        .await;
    }
    let mut is_new_coordinator = false;
    if let Some(ref id) = swarm_id {
        let mut coordinators = swarm_coordinators.write().await;
        if coordinators.get(id).is_none() {
            coordinators.insert(id.clone(), client_session_id.clone());
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
    if let Some(ref id) = swarm_id {
        broadcast_swarm_status(id, &swarm_members, &swarms_by_id).await;
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
                            agent_guard.queue_soft_interrupt(notification, false);
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
                            let _ = client_event_tx.send(ServerEvent::Error {
                                id: done_id,
                                message: e.to_string(),
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
            } => {
                // Check if this client is already processing
                if client_is_processing {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Already processing a message".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }

                // Set processing flag for this client
                client_is_processing = true;
                processing_message_id = Some(id);
                processing_session_id = Some(client_session_id.clone());

                update_member_status(
                    &client_session_id,
                    "running",
                    Some(truncate_detail(&content, 120)),
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                    Some(&swarm_event_tx),
                )
                .await;

                let agent = Arc::clone(&agent);
                let tx = client_event_tx.clone();
                let done_tx = processing_done_tx.clone();
                crate::logging::info(&format!("Processing message id={} spawning task", id));
                processing_task = Some(tokio::spawn(async move {
                    let result = match std::panic::AssertUnwindSafe(process_message_streaming_mpsc(
                        agent, &content, images, tx,
                    ))
                    .catch_unwind()
                    .await
                    {
                        Ok(r) => r,
                        Err(panic_payload) => {
                            let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                                s.to_string()
                            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                                s.clone()
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
                        Err(e) => crate::logging::warn(&format!(
                            "Processing task completed with error for message id={}: {}",
                            id, e
                        )),
                    }
                    let _ = done_tx.send((id, result));
                }));
            }

            Request::Cancel { id } => {
                let _ = id; // cancel request id (not the message id)
                if let Some(mut handle) = processing_task.take() {
                    if handle.is_finished() {
                        processing_task = Some(handle);
                        continue;
                    }
                    // Signal graceful shutdown so the agent checkpoints partial content
                    // before exiting, then wait briefly for it to finish.
                    cancel_signal.fire();
                    match tokio::time::timeout(std::time::Duration::from_millis(500), &mut handle)
                        .await
                    {
                        Ok(_) => {
                            // Task exited gracefully within timeout
                        }
                        Err(_) => {
                            // Timed out waiting for graceful exit, force abort and wait
                            // for the task to actually release resources (e.g. agent mutex)
                            handle.abort();
                            match tokio::time::timeout(
                                std::time::Duration::from_millis(2000),
                                handle,
                            )
                            .await
                            {
                                Ok(_) => {
                                    crate::logging::info(
                                        "Aborted processing task released resources",
                                    );
                                }
                                Err(_) => {
                                    crate::logging::warn(
                                        "Aborted processing task did not release resources within 2s",
                                    );
                                }
                            }
                        }
                    }
                    // Reset the signal for future turns
                    cancel_signal.reset();
                    processing_task = None;
                    client_is_processing = false;
                    if let Some(session_id) = processing_session_id.take() {
                        update_member_status(
                            &session_id,
                            "stopped",
                            Some("cancelled".to_string()),
                            &swarm_members,
                            &swarms_by_id,
                            Some(&event_history),
                            Some(&event_counter),
                            Some(&swarm_event_tx),
                        )
                        .await;
                    }
                    if let Some(message_id) = processing_message_id.take() {
                        let _ = client_event_tx.send(ServerEvent::Interrupted);
                        let _ = client_event_tx.send(ServerEvent::Done { id: message_id });
                    }
                }
            }

            Request::SoftInterrupt {
                id,
                content,
                urgent,
            } => {
                // Queue a soft interrupt message to be injected at the next safe point
                // Uses the pre-extracted queue handle so we don't need the agent lock
                if let Ok(mut q) = soft_interrupt_queue.lock() {
                    q.push(crate::agent::SoftInterruptMessage { content, urgent });
                }
                let _ = client_event_tx.send(ServerEvent::Ack { id });
            }

            Request::CancelSoftInterrupts { id } => {
                // Cancel all pending soft interrupts (drain the queue)
                if let Ok(mut q) = soft_interrupt_queue.lock() {
                    q.clear();
                }
                let _ = client_event_tx.send(ServerEvent::Ack { id });
            }

            Request::BackgroundTool { id } => {
                // Signal the agent to move the currently executing tool to background
                background_tool_signal.fire();
                let _ = client_event_tx.send(ServerEvent::Ack { id });
            }

            Request::Clear { id } => {
                // Clear this client's session (create new agent)
                let preserve_debug = {
                    let agent_guard = agent.lock().await;
                    agent_guard.is_debug()
                };

                // Mark the old session as closed before replacing
                {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.mark_closed();
                }

                let mut new_agent = Agent::new(Arc::clone(&provider), registry.clone());
                let new_id = new_agent.session_id().to_string();

                // Enable self-dev mode when running in a self-dev environment
                if client_selfdev {
                    new_agent.set_canary("self-dev");
                    // selfdev tools should already be registered from initial connection
                }
                if preserve_debug {
                    new_agent.set_debug(true);
                }

                // Replace the agent in place
                let mut agent_guard = agent.lock().await;
                *agent_guard = new_agent;
                drop(agent_guard);

                // Update sessions map
                {
                    let mut sessions_guard = sessions.write().await;
                    sessions_guard.remove(&client_session_id);
                    sessions_guard.insert(new_id.clone(), Arc::clone(&agent));
                }

                // Update swarm membership to the new session id
                let swarm_id_for_update = {
                    let mut members = swarm_members.write().await;
                    if let Some(mut member) = members.remove(&client_session_id) {
                        let sid = member.swarm_id.clone();
                        member.session_id = new_id.clone();
                        member.status = "ready".to_string();
                        member.detail = None;
                        members.insert(new_id.clone(), member);
                        sid
                    } else {
                        None
                    }
                };
                if let Some(ref swarm_id) = swarm_id_for_update {
                    let mut swarms = swarms_by_id.write().await;
                    if let Some(swarm) = swarms.get_mut(swarm_id) {
                        swarm.remove(&client_session_id);
                        swarm.insert(new_id.clone());
                    }
                }
                update_member_status(
                    &new_id,
                    "ready",
                    None,
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                    Some(&swarm_event_tx),
                )
                .await;
                if let Some(swarm_id) = swarm_id_for_update {
                    rename_plan_participant(&swarm_id, &client_session_id, &new_id, &swarm_plans)
                        .await;
                }

                client_session_id = new_id.clone();
                {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.session_id = new_id.clone();
                        info.last_seen = Instant::now();
                    }
                }
                let _ = client_event_tx.send(ServerEvent::SessionId { session_id: new_id });
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::Ping { id } => {
                let json = encode_event(&ServerEvent::Pong { id });
                let mut w = writer.lock().await;
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
            }

            Request::GetState { id } => {
                let sessions_guard = sessions.read().await;
                let all_sessions: Vec<String> = sessions_guard.keys().cloned().collect();
                let session_count = all_sessions.len();
                drop(sessions_guard);

                let event = ServerEvent::State {
                    id,
                    session_id: client_session_id.clone(),
                    message_count: session_count,
                    is_processing: client_is_processing,
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
            }

            Request::Subscribe {
                id,
                working_dir: subscribe_working_dir,
                selfdev,
            } => {
                // Update session working directory from client's cwd
                if let Some(ref dir) = subscribe_working_dir {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.set_working_dir(dir);
                    drop(agent_guard);

                    // Update swarm member's working directory + swarm id
                    let new_path = PathBuf::from(dir);
                    let new_swarm_id = swarm_id_for_dir(Some(new_path.clone()));
                    let mut old_swarm_id: Option<String> = None;
                    let mut updated_swarm_id: Option<String> = None;
                    // Update member fields (separate scope to avoid nested locks)
                    {
                        let mut members = swarm_members.write().await;
                        if let Some(member) = members.get_mut(&client_session_id) {
                            old_swarm_id = member.swarm_id.clone();
                            member.working_dir = Some(new_path);
                            member.swarm_id = if member.swarm_enabled {
                                new_swarm_id.clone()
                            } else {
                                None
                            };
                            updated_swarm_id = member.swarm_id.clone();
                        }
                    }
                    // Remove from old swarm group
                    if let Some(ref old_id) = old_swarm_id {
                        let mut swarms = swarms_by_id.write().await;
                        if let Some(swarm) = swarms.get_mut(old_id) {
                            swarm.remove(&client_session_id);
                            if swarm.is_empty() {
                                swarms.remove(old_id);
                            }
                        }
                    }
                    // Add to new swarm group
                    if let Some(ref new_id) = updated_swarm_id {
                        let mut swarms = swarms_by_id.write().await;
                        swarms
                            .entry(new_id.clone())
                            .or_insert_with(HashSet::new)
                            .insert(client_session_id.clone());
                    }
                    if let Some(old_id) = old_swarm_id.clone() {
                        let was_coordinator = {
                            let coordinators = swarm_coordinators.read().await;
                            coordinators
                                .get(&old_id)
                                .map(|id| id == &client_session_id)
                                .unwrap_or(false)
                        };
                        if was_coordinator {
                            let mut new_coordinator: Option<String> = None;
                            {
                                let swarms = swarms_by_id.read().await;
                                if let Some(swarm) = swarms.get(&old_id) {
                                    new_coordinator = swarm.iter().min().cloned();
                                }
                            }
                            {
                                let mut coordinators = swarm_coordinators.write().await;
                                coordinators.remove(&old_id);
                                if let Some(ref new_id) = new_coordinator {
                                    coordinators.insert(old_id.clone(), new_id.clone());
                                }
                            }
                            // Notify new coordinator (separate scope to avoid nested lock)
                            if let Some(new_id) = new_coordinator.clone() {
                                let members = swarm_members.read().await;
                                if let Some(member) = members.get(&new_id) {
                                    let msg =
                                        "You are now the coordinator for this swarm.".to_string();
                                    let _ = member.event_tx.send(ServerEvent::Notification {
                                        from_session: new_id.clone(),
                                        from_name: member.friendly_name.clone(),
                                        notification_type: NotificationType::Message {
                                            scope: Some("swarm".to_string()),
                                            channel: None,
                                        },
                                        message: msg.clone(),
                                    });
                                }
                            }
                        }
                    }
                    if let Some(new_id) = updated_swarm_id.clone() {
                        let mut coordinators = swarm_coordinators.write().await;
                        if coordinators.get(&new_id).is_none() {
                            coordinators.insert(new_id.clone(), client_session_id.clone());
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
                    }
                    if let Some(old_id) = old_swarm_id.clone() {
                        if updated_swarm_id.as_ref() != Some(&old_id) {
                            remove_plan_participant(&old_id, &client_session_id, &swarm_plans)
                                .await;
                        }
                        broadcast_swarm_status(&old_id, &swarm_members, &swarms_by_id).await;
                    }
                    if let Some(new_id) = updated_swarm_id {
                        if old_swarm_id.as_ref() != Some(&new_id) {
                            broadcast_swarm_status(&new_id, &swarm_members, &swarms_by_id).await;
                        }
                    }
                }

                let mut should_selfdev = client_selfdev;
                if matches!(selfdev, Some(true)) {
                    should_selfdev = true;
                }

                if !should_selfdev {
                    if let Some(ref dir) = subscribe_working_dir {
                        let path = PathBuf::from(dir);
                        if is_jcode_repo_or_parent(&path) {
                            should_selfdev = true;
                        }
                    }
                }

                if should_selfdev {
                    client_selfdev = true;
                    let mut agent_guard = agent.lock().await;
                    if !agent_guard.is_canary() {
                        agent_guard.set_canary("self-dev");
                    }
                    drop(agent_guard);
                    registry.register_selfdev_tools().await;
                }

                // Register MCP tools (management tool + server tool proxies)
                // Shared pool means shared servers only spawn once across sessions
                registry
                    .register_mcp_tools(
                        Some(client_event_tx.clone()),
                        Some(Arc::clone(&mcp_pool)),
                        Some(client_session_id.clone()),
                    )
                    .await;

                // Note: Don't send SessionId here - it's included in the History response
                // from GetHistory. Sending it here causes race conditions when ResumeSession
                // is also called, as client_session_id may not yet be updated.
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::GetHistory { id } => {
                let _ = provider.prefetch_models().await;
                let (
                    messages,
                    is_canary,
                    provider_name,
                    provider_model,
                    available_models,
                    available_model_routes,
                    tool_names,
                    upstream_provider,
                ) = {
                    let agent_guard = agent.lock().await;
                    (
                        agent_guard.get_history(),
                        agent_guard.is_canary(),
                        agent_guard.provider_name(),
                        agent_guard.provider_model(),
                        agent_guard.available_models_display(),
                        agent_guard.model_routes(),
                        agent_guard.tool_names().await,
                        agent_guard.last_upstream_provider(),
                    )
                };

                // Build MCP server list with tool counts from registered tool names
                let mut mcp_map: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                for name in &tool_names {
                    if let Some(rest) = name.strip_prefix("mcp__") {
                        if let Some((server, _tool)) = rest.split_once("__") {
                            *mcp_map.entry(server.to_string()).or_default() += 1;
                        }
                    }
                }
                let mcp_servers: Vec<String> = mcp_map
                    .into_iter()
                    .map(|(name, count)| format!("{}:{}", name, count))
                    .collect();

                // Build skills list
                let skills = crate::skill::SkillRegistry::load()
                    .map(|r| r.list().iter().map(|s| s.name.clone()).collect())
                    .unwrap_or_default();

                // Get all session IDs and client count
                let (all_sessions, current_client_count) = {
                    let sessions_guard = sessions.read().await;
                    let all: Vec<String> = sessions_guard.keys().cloned().collect();
                    let count = *client_count.read().await;
                    (all, count)
                };

                let event = ServerEvent::History {
                    id,
                    session_id: client_session_id.clone(),
                    messages,
                    provider_name: Some(provider_name),
                    provider_model: Some(provider_model),
                    available_models,
                    available_model_routes,
                    mcp_servers,
                    skills,
                    total_tokens: None,
                    all_sessions,
                    client_count: Some(current_client_count),
                    is_canary: Some(is_canary),
                    server_version: Some(env!("JCODE_VERSION").to_string()),
                    server_name: Some(server_name.clone()),
                    server_icon: Some(server_icon.clone()),
                    server_has_update: Some(server_has_newer_binary()),
                    was_interrupted: None,
                    upstream_provider,
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                if w.write_all(json.as_bytes()).await.is_err() {
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
                // Notify this client that server is reloading
                let _ = client_event_tx.send(ServerEvent::Reloading { new_socket: None });

                // Capture provider/model from the live session, not the stale
                // provider template created when the client connected.
                let (provider_arg, model_arg) = {
                    let agent_guard = agent.lock().await;
                    (
                        provider_cli_arg(&agent_guard.provider_name()),
                        normalize_model_arg(agent_guard.provider_model()),
                    )
                };

                // Spawn reload process with progress streaming
                let progress_tx = client_event_tx.clone();
                let socket_arg = socket_path().to_string_lossy().to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if let Err(e) = do_server_reload_with_progress(
                        progress_tx.clone(),
                        provider_arg,
                        model_arg,
                        socket_arg,
                    )
                    .await
                    {
                        let _ = progress_tx.send(ServerEvent::ReloadProgress {
                            step: "error".to_string(),
                            message: format!("Reload failed: {}", e),
                            success: Some(false),
                            output: None,
                        });
                        crate::logging::error(&format!("Reload failed: {}", e));
                    }
                });

                // Send Done after starting the reload (client will reconnect after server restarts)
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::ResumeSession { id, session_id } => {
                // Mark the current session as closed before switching
                {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.mark_closed();
                }

                // Load the specified session into this client's agent
                let (result, is_canary) = {
                    let mut agent_guard = agent.lock().await;
                    let result = agent_guard.restore_session(&session_id);
                    if client_selfdev || is_selfdev_env() {
                        agent_guard.set_canary("self-dev");
                    }
                    let is_canary = agent_guard.is_canary();
                    (result, is_canary)
                };

                let was_interrupted = match &result {
                    Ok(status) => match status {
                        crate::session::SessionStatus::Crashed { .. } => true,
                        crate::session::SessionStatus::Active => {
                            let agent_guard = agent.lock().await;
                            let last_role = agent_guard.last_message_role();
                            let last_is_user = last_role
                                .as_ref()
                                .map(|r| *r == crate::message::Role::User)
                                .unwrap_or(false);
                            let last_is_reload_interrupted = last_role
                                .as_ref()
                                .map(|r| *r == crate::message::Role::Assistant)
                                .unwrap_or(false)
                                && agent_guard
                                    .last_message_text()
                                    .map(|t| {
                                        t.ends_with("[generation interrupted - server reloading]")
                                    })
                                    .unwrap_or(false);
                            if last_is_user {
                                crate::logging::info(&format!(
                                        "Session {} was Active with pending user message - treating as interrupted",
                                        session_id
                                    ));
                            }
                            if last_is_reload_interrupted {
                                crate::logging::info(&format!(
                                    "Session {} was interrupted by reload - will auto-resume",
                                    session_id
                                ));
                            }
                            last_is_user || last_is_reload_interrupted
                        }
                        _ => false,
                    },
                    Err(_) => false,
                };

                if result.is_ok() && is_canary {
                    client_selfdev = true;
                    registry.register_selfdev_tools().await;
                }

                // Register MCP tools for resumed sessions
                if result.is_ok() {
                    registry
                        .register_mcp_tools(
                            Some(client_event_tx.clone()),
                            Some(Arc::clone(&mcp_pool)),
                            Some(client_session_id.clone()),
                        )
                        .await;
                }

                match result {
                    Ok(_prev_status) => {
                        // Update client_session_id to match the restored session
                        let old_session_id = client_session_id.clone();
                        client_session_id = session_id.clone();

                        {
                            let mut sessions_guard = sessions.write().await;
                            sessions_guard.remove(&old_session_id);
                            sessions_guard.insert(session_id.clone(), Arc::clone(&agent));
                        }
                        {
                            let mut connections = client_connections.write().await;
                            if let Some(info) = connections.get_mut(&client_connection_id) {
                                info.session_id = session_id.clone();
                                info.last_seen = Instant::now();
                            }
                        }

                        {
                            let mut members = swarm_members.write().await;
                            if let Some(mut member) = members.remove(&old_session_id) {
                                if let Some(ref swarm_id) = member.swarm_id {
                                    let mut swarms = swarms_by_id.write().await;
                                    if let Some(swarm) = swarms.get_mut(swarm_id) {
                                        swarm.remove(&old_session_id);
                                        swarm.insert(session_id.clone());
                                    }
                                }
                                member.session_id = session_id.clone();
                                member.status = "ready".to_string();
                                member.detail = None;
                                members.insert(session_id.clone(), member);
                            }
                        }
                        {
                            let mut coordinators = swarm_coordinators.write().await;
                            for coord in coordinators.values_mut() {
                                if *coord == old_session_id {
                                    *coord = session_id.clone();
                                }
                            }
                        }
                        update_member_status(
                            &session_id,
                            "ready",
                            None,
                            &swarm_members,
                            &swarms_by_id,
                            Some(&event_history),
                            Some(&event_counter),
                            Some(&swarm_event_tx),
                        )
                        .await;
                        if let Some(swarm_id) = {
                            let members = swarm_members.read().await;
                            members.get(&session_id).and_then(|m| m.swarm_id.clone())
                        } {
                            rename_plan_participant(
                                &swarm_id,
                                &old_session_id,
                                &session_id,
                                &swarm_plans,
                            )
                            .await;
                        }

                        // Send updated history to client
                        let _ = provider.prefetch_models().await;
                        let (
                            messages,
                            is_canary,
                            provider_name,
                            provider_model,
                            available_models,
                            available_model_routes,
                            tool_names,
                            upstream_provider,
                        ) = {
                            let agent_guard = agent.lock().await;
                            (
                                agent_guard.get_history(),
                                agent_guard.is_canary(),
                                agent_guard.provider_name(),
                                agent_guard.provider_model(),
                                agent_guard.available_models_display(),
                                agent_guard.model_routes(),
                                agent_guard.tool_names().await,
                                agent_guard.last_upstream_provider(),
                            )
                        };

                        // Build MCP server list with tool counts
                        let mut mcp_map: std::collections::BTreeMap<String, usize> =
                            std::collections::BTreeMap::new();
                        for name in &tool_names {
                            if let Some(rest) = name.strip_prefix("mcp__") {
                                if let Some((server, _tool)) = rest.split_once("__") {
                                    *mcp_map.entry(server.to_string()).or_default() += 1;
                                }
                            }
                        }
                        let mcp_servers: Vec<String> = mcp_map
                            .into_iter()
                            .map(|(name, count)| format!("{}:{}", name, count))
                            .collect();

                        let skills = crate::skill::SkillRegistry::load()
                            .map(|r| r.list().iter().map(|s| s.name.clone()).collect())
                            .unwrap_or_default();

                        let (all_sessions, current_client_count) = {
                            let sessions_guard = sessions.read().await;
                            let all: Vec<String> = sessions_guard.keys().cloned().collect();
                            let count = *client_count.read().await;
                            (all, count)
                        };

                        let event = ServerEvent::History {
                            id,
                            session_id: session_id.clone(),
                            messages,
                            provider_name: Some(provider_name),
                            provider_model: Some(provider_model),
                            available_models,
                            available_model_routes,
                            mcp_servers,
                            skills,
                            total_tokens: None,
                            all_sessions,
                            client_count: Some(current_client_count),
                            is_canary: Some(is_canary),
                            server_version: Some(env!("JCODE_VERSION").to_string()),
                            server_name: Some(server_name.clone()),
                            server_icon: Some(server_icon.clone()),
                            server_has_update: Some(server_has_newer_binary()),
                            was_interrupted: if was_interrupted { Some(true) } else { None },
                            upstream_provider,
                        };
                        let json = encode_event(&event);
                        let mut w = writer.lock().await;
                        if w.write_all(json.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Failed to restore session: {}", e),
                            retry_after_secs: None,
                        });
                    }
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

            Request::NotifyAuthChanged { id } => {
                handle_notify_auth_changed(id, &provider, &agent, &client_event_tx).await;
            }

            Request::SwitchAnthropicAccount { id, label } => {
                handle_switch_anthropic_account(id, label, &agent, &client_event_tx).await;
            }

            Request::SetFeature {
                id,
                feature,
                enabled,
            } => match feature {
                FeatureToggle::Memory => {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.set_memory_enabled(enabled);
                    drop(agent_guard);
                    if !enabled {
                        crate::memory::clear_pending_memory(&client_session_id);
                    }
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                }
                FeatureToggle::Swarm => {
                    if swarm_enabled == enabled {
                        let _ = client_event_tx.send(ServerEvent::Done { id });
                        continue;
                    }
                    swarm_enabled = enabled;

                    let (old_swarm_id, working_dir) = {
                        let mut members = swarm_members.write().await;
                        if let Some(member) = members.get_mut(&client_session_id) {
                            let old = member.swarm_id.clone();
                            let wd = member.working_dir.clone();
                            member.swarm_enabled = enabled;
                            if !enabled {
                                member.swarm_id = None;
                                member.role = "agent".to_string();
                            }
                            (old, wd)
                        } else {
                            (None, None)
                        }
                    };

                    if let Some(ref old_id) = old_swarm_id {
                        remove_session_from_swarm(
                            &client_session_id,
                            old_id,
                            &swarm_members,
                            &swarms_by_id,
                            &swarm_coordinators,
                            &swarm_plans,
                        )
                        .await;
                    }

                    if enabled {
                        let new_swarm_id = swarm_id_for_dir(working_dir);
                        if let Some(ref id) = new_swarm_id {
                            {
                                let mut swarms = swarms_by_id.write().await;
                                swarms
                                    .entry(id.clone())
                                    .or_insert_with(HashSet::new)
                                    .insert(client_session_id.clone());
                            }

                            let mut is_new_coordinator = false;
                            {
                                let mut coordinators = swarm_coordinators.write().await;
                                if coordinators.get(id).is_none() {
                                    coordinators.insert(id.clone(), client_session_id.clone());
                                    is_new_coordinator = true;
                                }
                            }

                            {
                                let mut members = swarm_members.write().await;
                                if let Some(member) = members.get_mut(&client_session_id) {
                                    member.swarm_id = Some(id.clone());
                                    member.role = if is_new_coordinator {
                                        "coordinator".to_string()
                                    } else {
                                        "agent".to_string()
                                    };
                                }
                            }

                            broadcast_swarm_status(id, &swarm_members, &swarms_by_id).await;

                            if is_new_coordinator {
                                let msg = "You are the coordinator for this swarm.".to_string();
                                let _ = client_event_tx.send(ServerEvent::Notification {
                                    from_session: client_session_id.clone(),
                                    from_name: friendly_name.clone(),
                                    notification_type: NotificationType::Message {
                                        scope: Some("swarm".to_string()),
                                        channel: None,
                                    },
                                    message: msg,
                                });
                            }
                        } else {
                            let _ = client_event_tx.send(ServerEvent::SwarmStatus {
                                members: Vec::new(),
                            });
                        }
                    } else {
                        let _ = client_event_tx.send(ServerEvent::SwarmStatus {
                            members: Vec::new(),
                        });
                    }

                    let _ = client_event_tx.send(ServerEvent::Done { id });
                }
            },

            Request::Split { id } => {
                // Clone the current session's messages into a new session
                let (new_session_id, new_session_name) = {
                    let agent_guard = agent.lock().await;
                    let parent_session_id = agent_guard.session_id().to_string();
                    let messages = agent_guard.messages().to_vec();
                    let working_dir = agent_guard.working_dir().map(|s| s.to_string());
                    let model = Some(agent_guard.provider_model());

                    // Create a new session with the same messages
                    let mut child = Session::create(Some(parent_session_id), None);
                    child.messages = messages;
                    child.working_dir = working_dir;
                    child.model = model;
                    child.status = crate::session::SessionStatus::Closed;

                    if let Err(e) = child.save() {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Failed to save split session: {}", e),
                            retry_after_secs: None,
                        });
                        continue;
                    }

                    let name = child.display_name().to_string();
                    (child.id.clone(), name)
                };

                let _ = client_event_tx.send(ServerEvent::SplitResponse {
                    id,
                    new_session_id,
                    new_session_name,
                });
            }

            Request::Compact { id } => {
                let agent = Arc::clone(&agent);
                let tx = client_event_tx.clone();
                tokio::spawn(async move {
                    let agent_guard = agent.lock().await;
                    let provider = agent_guard.provider_fork();
                    let compaction = agent_guard.registry().compaction();
                    let messages = agent_guard.provider_messages();
                    drop(agent_guard);

                    if !provider.supports_compaction() {
                        let _ = tx.send(ServerEvent::CompactResult {
                            id,
                            message: "Manual compaction is not available for this provider."
                                .to_string(),
                            success: false,
                        });
                        return;
                    }

                    let result = match compaction.try_write() {
                        Ok(mut manager) => {
                            let stats = manager.stats_with(&messages);
                            let status_msg = format!(
                                "**Context Status:**\n\
                                • Messages: {} (active), {} (total history)\n\
                                • Token usage: ~{}k (estimate ~{}k) / {}k ({:.1}%)\n\
                                • Has summary: {}\n\
                                • Compacting: {}",
                                stats.active_messages,
                                stats.total_turns,
                                stats.effective_tokens / 1000,
                                stats.token_estimate / 1000,
                                manager.token_budget() / 1000,
                                stats.context_usage * 100.0,
                                if stats.has_summary { "yes" } else { "no" },
                                if stats.is_compacting {
                                    "in progress..."
                                } else {
                                    "no"
                                }
                            );

                            match manager.force_compact_with(&messages, provider) {
                                Ok(()) => ServerEvent::CompactResult {
                                    id,
                                    message: format!(
                                        "{}\n\n✓ **Compaction started** — summarizing older messages in background.\n\
                                        The summary will be applied automatically when ready.",
                                        status_msg
                                    ),
                                    success: true,
                                },
                                Err(reason) => ServerEvent::CompactResult {
                                    id,
                                    message: format!(
                                        "{}\n\n⚠ **Cannot compact:** {}",
                                        status_msg, reason
                                    ),
                                    success: false,
                                },
                            }
                        }
                        Err(_) => ServerEvent::CompactResult {
                            id,
                            message: "⚠ Cannot access compaction manager (lock held)".to_string(),
                            success: false,
                        },
                    };
                    let _ = tx.send(result);
                });
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
                if let Some(tx) = stdin_responses.lock().await.remove(&request_id) {
                    let _ = tx.send(input);
                }
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::AgentTask { id, task, .. } => {
                // Process as a message on this client's agent
                update_member_status(
                    &client_session_id,
                    "running",
                    Some(truncate_detail(&task, 120)),
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                    Some(&swarm_event_tx),
                )
                .await;
                let result = process_message_streaming_mpsc(
                    Arc::clone(&agent),
                    &task,
                    vec![],
                    client_event_tx.clone(),
                )
                .await;
                match result {
                    Ok(()) => {
                        update_member_status(
                            &client_session_id,
                            "completed",
                            None,
                            &swarm_members,
                            &swarms_by_id,
                            Some(&event_history),
                            Some(&event_counter),
                            Some(&swarm_event_tx),
                        )
                        .await;
                        let _ = client_event_tx.send(ServerEvent::Done { id });
                    }
                    Err(e) => {
                        update_member_status(
                            &client_session_id,
                            "failed",
                            Some(truncate_detail(&e.to_string(), 120)),
                            &swarm_members,
                            &swarms_by_id,
                            Some(&event_history),
                            Some(&event_counter),
                            Some(&swarm_event_tx),
                        )
                        .await;
                        let retry_after_secs = e
                            .downcast_ref::<StreamError>()
                            .and_then(|se| se.retry_after_secs);
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: e.to_string(),
                            retry_after_secs,
                        });
                    }
                }
            }

            Request::AgentCapabilities { id } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::AgentContext { id } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
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
                // Find the swarm id for this session
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members.get(&from_session).and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    let friendly_name = {
                        let members = swarm_members.read().await;
                        members
                            .get(&from_session)
                            .and_then(|m| m.friendly_name.clone())
                    };

                    let swarm_session_ids: Vec<String> = {
                        let swarms = swarms_by_id.read().await;
                        swarms
                            .get(&swarm_id)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default()
                    };

                    // Validate DM recipient exists in swarm
                    if let Some(ref target) = to_session {
                        if !swarm_session_ids.contains(target) {
                            let _ = client_event_tx.send(ServerEvent::Error {
                                id,
                                message: format!("DM failed: session '{}' not in swarm", target),
                                retry_after_secs: None,
                            });
                            continue;
                        }
                    }

                    let scope = if to_session.is_some() {
                        "dm"
                    } else if channel.is_some() {
                        "channel"
                    } else {
                        "broadcast"
                    };

                    let members = swarm_members.read().await;
                    let sessions = sessions.read().await;

                    let target_sessions: Vec<String> = if let Some(target) = to_session.clone() {
                        vec![target]
                    } else if let Some(ref ch) = channel {
                        // Channel message: check subscriptions
                        let subs = channel_subscriptions.read().await;
                        if let Some(channel_subs) = subs.get(&swarm_id).and_then(|s| s.get(ch)) {
                            // Send only to subscribed members (excluding sender)
                            channel_subs
                                .iter()
                                .filter(|sid| *sid != &from_session)
                                .cloned()
                                .collect()
                        } else {
                            // No subscriptions for this channel: broadcast to all
                            swarm_session_ids
                                .iter()
                                .filter(|sid| *sid != &from_session)
                                .cloned()
                                .collect()
                        }
                    } else {
                        swarm_session_ids
                            .iter()
                            .filter(|sid| *sid != &from_session)
                            .cloned()
                            .collect()
                    };

                    for sid in &target_sessions {
                        if !swarm_session_ids.contains(sid) {
                            continue;
                        }
                        if let Some(member) = members.get(sid) {
                            let from_label = friendly_name
                                .as_deref()
                                .unwrap_or(&from_session[..8.min(from_session.len())]);
                            let scope_label = match (scope, channel.as_deref()) {
                                ("channel", Some(ch)) => format!("#{}", ch),
                                ("dm", _) => "DM".to_string(),
                                _ => "broadcast".to_string(),
                            };
                            let notification_msg =
                                format!("{} from {}: {}", scope_label, from_label, message);
                            let _ = member.event_tx.send(ServerEvent::Notification {
                                from_session: from_session.clone(),
                                from_name: friendly_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some(scope.to_string()),
                                    channel: channel.clone(),
                                },
                                message: notification_msg.clone(),
                            });

                            // Also push to the agent's pending alerts
                            if let Some(agent) = sessions.get(sid) {
                                if let Ok(agent) = agent.try_lock() {
                                    agent.queue_soft_interrupt(notification_msg.clone(), false);
                                }
                            }
                        }
                    }
                    let scope_value = if scope == "channel" {
                        format!("#{}", channel.clone().unwrap_or_default())
                    } else {
                        scope.to_string()
                    };
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                        from_session.clone(),
                        friendly_name.clone(),
                        Some(swarm_id.clone()),
                        SwarmEventType::Notification {
                            notification_type: scope_value,
                            message: truncate_detail(&message, 220),
                        },
                    )
                    .await;
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm. Use a git repository to enable swarm features."
                            .to_string(),
                        retry_after_secs: None,
                    });
                }
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
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                let swarm_id = match swarm_id.as_ref() {
                    Some(sid) => sid.clone(),
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                            retry_after_secs: None,
                        });
                        continue;
                    }
                };

                let (from_name, coordinator_id) = {
                    let members = swarm_members.read().await;
                    let from_name = members
                        .get(&req_session_id)
                        .and_then(|m| m.friendly_name.clone());
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
                    continue;
                };

                if coordinator_id == req_session_id {
                    let (version, participant_ids) = {
                        let mut plans = swarm_plans.write().await;
                        let vp = plans
                            .entry(swarm_id.clone())
                            .or_insert_with(VersionedPlan::new);
                        vp.participants.insert(req_session_id.clone());
                        for item in &items {
                            if let Some(owner) = &item.assigned_to {
                                vp.participants.insert(owner.clone());
                            }
                        }
                        vp.items = items.clone();
                        vp.version += 1;
                        (vp.version, vp.participants.clone())
                    };

                    let members = swarm_members.read().await;
                    let agent_sessions = sessions.read().await;
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
                        if let Some(agent) = agent_sessions.get(&sid) {
                            if let Ok(agent) = agent.try_lock() {
                                agent.queue_soft_interrupt(notification_msg.clone(), false);
                            }
                        }
                    }

                    broadcast_swarm_plan(
                        &swarm_id,
                        Some("coordinator_direct_update".to_string()),
                        &swarm_plans,
                        &swarm_members,
                        &swarms_by_id,
                    )
                    .await;
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
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
                    continue;
                }

                let proposal_key = format!("plan_proposal:{}", req_session_id);
                let proposal_value =
                    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
                {
                    let mut ctx = shared_context.write().await;
                    let swarm_ctx = ctx.entry(swarm_id.clone()).or_insert_with(HashMap::new);
                    let now = Instant::now();
                    swarm_ctx.insert(
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
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
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
                let agent_sessions = sessions.read().await;
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
                if let Some(agent) = agent_sessions.get(&coordinator_id) {
                    if let Ok(agent) = agent.try_lock() {
                        agent.queue_soft_interrupt(notification_msg.clone(), false);
                    }
                }

                if let Some(member) = members.get(&req_session_id) {
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: req_session_id.clone(),
                        from_name: from_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some("plan_proposal".to_string()),
                            channel: None,
                        },
                        message: "Plan proposal sent to coordinator (not yet applied).".to_string(),
                    });
                }
                if let Some(agent) = agent_sessions.get(&req_session_id) {
                    if let Ok(agent) = agent.try_lock() {
                        agent.queue_soft_interrupt(
                            "Plan proposal sent to coordinator (not yet applied).".to_string(),
                            false,
                        );
                    }
                }

                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommApprovePlan {
                id,
                session_id: req_session_id,
                proposer_session,
            } => {
                // Verify the requester is the coordinator
                let (swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can approve plan proposals.".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }

                let swarm_id = match swarm_id.as_ref() {
                    Some(sid) => sid.clone(),
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                            retry_after_secs: None,
                        });
                        continue;
                    }
                };

                // Read proposal from shared context
                let proposal_key = format!("plan_proposal:{}", proposer_session);
                let proposal_value = {
                    let ctx = shared_context.read().await;
                    ctx.get(&swarm_id)
                        .and_then(|sc| sc.get(&proposal_key))
                        .map(|c| c.value.clone())
                };

                let proposal = match proposal_value {
                    Some(v) => v,
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!(
                                "No pending plan proposal from session '{}'",
                                proposer_session
                            ),
                            retry_after_secs: None,
                        });
                        continue;
                    }
                };

                // Parse and apply to swarm_plans
                if let Ok(items) = serde_json::from_str::<Vec<PlanItem>>(&proposal) {
                    let participant_ids = {
                        let mut plans = swarm_plans.write().await;
                        let vp = plans
                            .entry(swarm_id.clone())
                            .or_insert_with(VersionedPlan::new);
                        vp.items.extend(items.clone());
                        vp.version += 1;
                        vp.participants.insert(req_session_id.clone());
                        vp.participants.insert(proposer_session.clone());
                        for item in &items {
                            if let Some(owner) = &item.assigned_to {
                                vp.participants.insert(owner.clone());
                            }
                        }
                        vp.participants.clone()
                    };

                    // Remove proposal from shared context
                    {
                        let mut ctx = shared_context.write().await;
                        if let Some(swarm_ctx) = ctx.get_mut(&swarm_id) {
                            swarm_ctx.remove(&proposal_key);
                        }
                    }

                    broadcast_swarm_plan(
                        &swarm_id,
                        Some("proposal_approved".to_string()),
                        &swarm_plans,
                        &swarm_members,
                        &swarms_by_id,
                    )
                    .await;
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
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
                            .and_then(|m| m.friendly_name.clone())
                    };

                    let members = swarm_members.read().await;
                    let agent_sessions = sessions.read().await;
                    for sid in participant_ids {
                        if let Some(member) = members.get(&sid) {
                            let msg = format!(
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
                                message: msg.clone(),
                            });

                            if let Some(agent) = agent_sessions.get(&sid) {
                                if let Ok(agent) = agent.try_lock() {
                                    agent.queue_soft_interrupt(msg.clone(), false);
                                }
                            }
                        }
                    }
                }

                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommRejectPlan {
                id,
                session_id: req_session_id,
                proposer_session,
                reason,
            } => {
                // Verify the requester is the coordinator
                let (_swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can reject plan proposals.".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }

                let swarm_id = match swarm_id {
                    Some(ref sid) => sid.clone(),
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                            retry_after_secs: None,
                        });
                        continue;
                    }
                };

                // Check if proposal exists
                let proposal_key = format!("plan_proposal:{}", proposer_session);
                let proposal_exists = {
                    let ctx = shared_context.read().await;
                    ctx.get(&swarm_id)
                        .and_then(|sc| sc.get(&proposal_key))
                        .is_some()
                };

                if !proposal_exists {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!(
                            "No pending plan proposal from session '{}'",
                            proposer_session
                        ),
                        retry_after_secs: None,
                    });
                    continue;
                }

                // Remove proposal from shared context
                {
                    let mut ctx = shared_context.write().await;
                    if let Some(swarm_ctx) = ctx.get_mut(&swarm_id) {
                        swarm_ctx.remove(&proposal_key);
                    }
                }

                // Notify the proposer
                let coordinator_name = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.friendly_name.clone())
                };

                let members = swarm_members.read().await;
                let agent_sessions = sessions.read().await;
                if let Some(member) = members.get(&proposer_session) {
                    let reason_msg = reason
                        .as_ref()
                        .map(|r| format!(": {}", r))
                        .unwrap_or_default();
                    let msg = format!(
                        "Your plan proposal was rejected by the coordinator{}",
                        reason_msg
                    );
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: req_session_id.clone(),
                        from_name: coordinator_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some("dm".to_string()),
                            channel: None,
                        },
                        message: msg.clone(),
                    });

                    if let Some(agent) = agent_sessions.get(&proposer_session) {
                        if let Ok(agent) = agent.try_lock() {
                            agent.queue_soft_interrupt(msg, false);
                        }
                    }
                }
                record_swarm_event(
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
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

            Request::CommSpawn {
                id,
                session_id: req_session_id,
                working_dir,
                initial_message,
            } => {
                // Verify the requester is the coordinator
                let (_swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can spawn new agents.".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }
                let swarm_id = match swarm_id {
                    Some(ref sid) => sid.clone(),
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                            retry_after_secs: None,
                        });
                        continue;
                    }
                };

                let cmd = if let Some(ref dir) = working_dir {
                    format!("create_session:{}", dir)
                } else {
                    "create_session".to_string()
                };
                let coordinator_model = {
                    let agent_sessions = sessions.read().await;
                    agent_sessions.get(&req_session_id).and_then(|agent| {
                        agent
                            .try_lock()
                            .ok()
                            .map(|agent_guard| agent_guard.provider_model())
                    })
                };

                match create_headless_session(
                    &sessions,
                    &global_session_id,
                    &provider_template,
                    &cmd,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &swarm_plans,
                    coordinator_model,
                    Some(Arc::clone(&mcp_pool)),
                )
                .await
                {
                    Ok(result_json) => {
                        let new_session_id =
                            serde_json::from_str::<serde_json::Value>(&result_json)
                                .ok()
                                .and_then(|v| {
                                    v.get("session_id")
                                        .and_then(|s| s.as_str())
                                        .map(|s| s.to_string())
                                })
                                .unwrap_or_default();

                        {
                            let mut plans = swarm_plans.write().await;
                            if let Some(vp) = plans.get_mut(&swarm_id) {
                                if !vp.items.is_empty() || !vp.participants.is_empty() {
                                    vp.participants.insert(req_session_id.clone());
                                    vp.participants.insert(new_session_id.clone());
                                }
                            }
                        }

                        broadcast_swarm_plan(
                            &swarm_id,
                            Some("participant_spawned".to_string()),
                            &swarm_plans,
                            &swarm_members,
                            &swarms_by_id,
                        )
                        .await;
                        record_swarm_event_for_session(
                            &new_session_id,
                            SwarmEventType::MemberChange {
                                action: "joined".to_string(),
                            },
                            &swarm_members,
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                        )
                        .await;
                        // Spawn a background task to process the initial message
                        // on this headless agent. Without this, the agent sits idle forever.
                        if let Some(initial_msg) = initial_message {
                            let agent_arc = {
                                let agent_sessions = sessions.read().await;
                                agent_sessions.get(&new_session_id).cloned()
                            };
                            if let Some(agent_arc) = agent_arc {
                                let sid_clone = new_session_id.clone();
                                let swarm_members2 = Arc::clone(&swarm_members);
                                let swarms_by_id2 = Arc::clone(&swarms_by_id);
                                let event_history2 = Arc::clone(&event_history);
                                let event_counter2 = Arc::clone(&event_counter);
                                let swarm_event_tx2 = swarm_event_tx.clone();
                                tokio::spawn(async move {
                                    update_member_status(
                                        &sid_clone,
                                        "running",
                                        Some(truncate_detail(&initial_msg, 120)),
                                        &swarm_members2,
                                        &swarms_by_id2,
                                        Some(&event_history2),
                                        Some(&event_counter2),
                                        Some(&swarm_event_tx2),
                                    )
                                    .await;
                                    let (drain_tx, mut drain_rx) =
                                        tokio::sync::mpsc::unbounded_channel::<ServerEvent>();
                                    tokio::spawn(async move {
                                        while drain_rx.recv().await.is_some() {}
                                    });
                                    let result = process_message_streaming_mpsc(
                                        Arc::clone(&agent_arc),
                                        &initial_msg,
                                        vec![],
                                        drain_tx,
                                    )
                                    .await;
                                    let (new_status, new_detail) = match result {
                                        Ok(()) => ("ready", None),
                                        Err(ref e) => {
                                            ("failed", Some(truncate_detail(&e.to_string(), 120)))
                                        }
                                    };
                                    update_member_status(
                                        &sid_clone,
                                        new_status,
                                        new_detail,
                                        &swarm_members2,
                                        &swarms_by_id2,
                                        Some(&event_history2),
                                        Some(&event_counter2),
                                        Some(&swarm_event_tx2),
                                    )
                                    .await;
                                });
                            }
                        }

                        let _ = client_event_tx.send(ServerEvent::CommSpawnResponse {
                            id,
                            session_id: req_session_id.clone(),
                            new_session_id,
                        });
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Failed to spawn agent: {}", e),
                            retry_after_secs: None,
                        });
                    }
                }
            }

            Request::CommStop {
                id,
                session_id: req_session_id,
                target_session,
            } => {
                // Verify the requester is the coordinator
                let (_swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can stop agents.".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }

                // Remove session and trigger final memory extraction
                let mut sessions_guard = sessions.write().await;
                let removed_agent = sessions_guard.remove(&target_session);
                drop(sessions_guard);
                if let Some(agent_arc) = removed_agent {
                    {
                        let agent = agent_arc.lock().await;
                        let memory_enabled = agent.memory_enabled();
                        let transcript = if memory_enabled {
                            Some(agent.build_transcript_for_extraction())
                        } else {
                            None
                        };
                        let sid = target_session.clone();
                        drop(agent);
                        if let Some(transcript) = transcript {
                            crate::memory_agent::trigger_final_extraction(transcript, sid);
                        }
                    }
                    // Clean up swarm membership
                    let (removed_swarm_id, removed_name) = {
                        let mut members = swarm_members.write().await;
                        if let Some(member) = members.remove(&target_session) {
                            (member.swarm_id, member.friendly_name)
                        } else {
                            (None, None)
                        }
                    };
                    if let Some(ref sid) = removed_swarm_id {
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            target_session.clone(),
                            removed_name.clone(),
                            Some(sid.clone()),
                            SwarmEventType::MemberChange {
                                action: "left".to_string(),
                            },
                        )
                        .await;
                        remove_plan_participant(sid, &target_session, &swarm_plans).await;
                        let mut swarms = swarms_by_id.write().await;
                        if let Some(swarm) = swarms.get_mut(sid) {
                            swarm.remove(&target_session);
                            if swarm.is_empty() {
                                swarms.remove(sid);
                            }
                        }
                        // Handle coordinator re-election if needed
                        let was_coordinator = {
                            let coordinators = swarm_coordinators.read().await;
                            coordinators
                                .get(sid)
                                .map(|c| c == &target_session)
                                .unwrap_or(false)
                        };
                        if was_coordinator {
                            let new_coordinator = {
                                let swarms = swarms_by_id.read().await;
                                swarms.get(sid).and_then(|s| s.iter().min().cloned())
                            };
                            let mut coordinators = swarm_coordinators.write().await;
                            coordinators.remove(sid);
                            if let Some(ref new_id) = new_coordinator {
                                coordinators.insert(sid.clone(), new_id.clone());
                                let mut members = swarm_members.write().await;
                                if let Some(m) = members.get_mut(new_id) {
                                    m.role = "coordinator".to_string();
                                }
                                let mut plans = swarm_plans.write().await;
                                if let Some(vp) = plans.get_mut(sid) {
                                    vp.participants.insert(new_id.clone());
                                }
                            }
                        }
                        broadcast_swarm_status(sid, &swarm_members, &swarms_by_id).await;
                    }
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!("Unknown session '{}'", target_session),
                        retry_after_secs: None,
                    });
                }
            }

            Request::CommAssignRole {
                id,
                session_id: req_session_id,
                target_session,
                role,
            } => {
                // Verify the requester is the coordinator.
                // Exception: a session may self-promote to coordinator if the current
                // coordinator has no active agent session (disconnected/stale).
                let (swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());

                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        let current_coordinator = coordinators.get(sid).cloned();
                        drop(coordinators);

                        crate::logging::info(&format!(
                            "[CommAssignRole] req={} target={} role={} swarm={} current_coord={:?}",
                            req_session_id, target_session, role, sid, current_coordinator
                        ));

                        if current_coordinator.as_deref() == Some(req_session_id.as_str()) {
                            // Already the coordinator
                            true
                        } else if role == "coordinator" && target_session == req_session_id {
                            // Self-promotion: allowed if current coordinator's event channel
                            // is closed (zombie from a previous server instance), has no
                            // active agent session, or is a headless (spawned) session.
                            drop(members);
                            let coordinator_is_zombie = if let Some(ref coord_id) =
                                current_coordinator
                            {
                                let (channel_closed, coord_is_headless) = {
                                    let m = swarm_members.read().await;
                                    m.get(coord_id)
                                        .map(|mb| (mb.event_tx.is_closed(), mb.is_headless))
                                        .unwrap_or((true, false))
                                };
                                let not_in_sessions = !sessions.read().await.contains_key(coord_id);
                                channel_closed || not_in_sessions || coord_is_headless
                            } else {
                                true
                            };
                            coordinator_is_zombie
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can assign roles. (Tip: if the coordinator has disconnected, use assign_role with target_session set to your own session ID to self-promote.)".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }

                let swarm_id = match swarm_id {
                    Some(sid) => sid,
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                            retry_after_secs: None,
                        });
                        continue;
                    }
                };

                // Update role on target member
                {
                    let mut members = swarm_members.write().await;
                    if let Some(m) = members.get_mut(&target_session) {
                        m.role = role.clone();
                    } else {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Unknown session '{}'", target_session),
                            retry_after_secs: None,
                        });
                        continue;
                    }
                }

                // If assigning coordinator, update swarm_coordinators
                if role == "coordinator" {
                    {
                        let mut coordinators = swarm_coordinators.write().await;
                        coordinators.insert(swarm_id.clone(), target_session.clone());
                    }
                    // Demote the old coordinator to agent (separate scope to avoid nested lock)
                    let mut members = swarm_members.write().await;
                    if let Some(m) = members.get_mut(&req_session_id) {
                        if m.session_id != target_session {
                            m.role = "agent".to_string();
                        }
                    }
                }

                broadcast_swarm_status(&swarm_id, &swarm_members, &swarms_by_id).await;
                record_swarm_event(
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                    req_session_id.clone(),
                    None,
                    Some(swarm_id.clone()),
                    SwarmEventType::Notification {
                        notification_type: "role_assignment".to_string(),
                        message: format!("{} -> {}", target_session, role),
                    },
                )
                .await;
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommSummary {
                id,
                session_id: _req_session_id,
                target_session,
                limit,
            } => {
                let limit = limit.unwrap_or(10);
                let agent_sessions = sessions.read().await;
                if let Some(agent) = agent_sessions.get(&target_session) {
                    let tool_calls = if let Ok(agent) = agent.try_lock() {
                        let history = agent.get_history();
                        let mut calls: Vec<crate::protocol::ToolCallSummary> = Vec::new();
                        for msg in history.iter().rev() {
                            if calls.len() >= limit {
                                break;
                            }
                            if let Some(tool_names) = &msg.tool_calls {
                                for name in tool_names {
                                    calls.push(crate::protocol::ToolCallSummary {
                                        tool_name: name.clone(),
                                        brief_output: truncate_detail(&msg.content, 200),
                                        timestamp_secs: None,
                                    });
                                    if calls.len() >= limit {
                                        break;
                                    }
                                }
                            }
                        }
                        calls.reverse();
                        calls
                    } else {
                        Vec::new()
                    };
                    let _ = client_event_tx.send(ServerEvent::CommSummaryResponse {
                        id,
                        session_id: target_session,
                        tool_calls,
                    });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!("Unknown session '{}'", target_session),
                        retry_after_secs: None,
                    });
                }
            }

            Request::CommReadContext {
                id,
                session_id: _req_session_id,
                target_session,
            } => {
                let agent_sessions = sessions.read().await;
                if let Some(agent) = agent_sessions.get(&target_session) {
                    let messages = if let Ok(agent) = agent.try_lock() {
                        agent.get_history()
                    } else {
                        Vec::new()
                    };
                    let _ = client_event_tx.send(ServerEvent::CommContextHistory {
                        id,
                        session_id: target_session,
                        messages,
                    });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!("Unknown session '{}'", target_session),
                        retry_after_secs: None,
                    });
                }
            }

            Request::CommResyncPlan {
                id,
                session_id: req_session_id,
            } => {
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    let plan_state = {
                        let mut plans = swarm_plans.write().await;
                        plans.get_mut(&swarm_id).map(|vp| {
                            vp.participants.insert(req_session_id.clone());
                            (vp.version, vp.items.len())
                        })
                    };
                    if let Some((version, item_count)) = plan_state {
                        if let Some(member) = swarm_members.read().await.get(&req_session_id) {
                            let _ = member.event_tx.send(ServerEvent::Notification {
                                from_session: req_session_id.clone(),
                                from_name: member.friendly_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some("plan".to_string()),
                                    channel: None,
                                },
                                message: format!(
                                    "Plan attached to this session (v{}, {} items).",
                                    version, item_count
                                ),
                            });
                        }
                        broadcast_swarm_plan(
                            &swarm_id,
                            Some("resync".to_string()),
                            &swarm_plans,
                            &swarm_members,
                            &swarms_by_id,
                        )
                        .await;
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            req_session_id.clone(),
                            None,
                            Some(swarm_id.clone()),
                            SwarmEventType::PlanUpdate {
                                swarm_id: swarm_id.clone(),
                                item_count,
                            },
                        )
                        .await;
                        let _ = client_event_tx.send(ServerEvent::Done { id });
                    } else {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "No swarm plan exists for this swarm.".to_string(),
                            retry_after_secs: None,
                        });
                    }
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm.".to_string(),
                        retry_after_secs: None,
                    });
                }
            }

            Request::CommAssignTask {
                id,
                session_id: req_session_id,
                target_session,
                task_id,
                message,
            } => {
                // Verify the requester is the coordinator
                let (swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can assign tasks.".to_string(),
                        retry_after_secs: None,
                    });
                    continue;
                }

                let swarm_id = match swarm_id {
                    Some(sid) => sid,
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                            retry_after_secs: None,
                        });
                        continue;
                    }
                };

                // Find and update the plan item
                let (task_content, participant_ids, plan_item_count) = {
                    let mut plans = swarm_plans.write().await;
                    let vp = plans
                        .entry(swarm_id.clone())
                        .or_insert_with(VersionedPlan::new);
                    let found = vp.items.iter_mut().find(|t| t.id == task_id);
                    if let Some(item) = found {
                        item.assigned_to = Some(target_session.clone());
                        item.status = "queued".to_string();
                        vp.version += 1;
                        vp.participants.insert(req_session_id.clone());
                        vp.participants.insert(target_session.clone());
                        (
                            Some(item.content.clone()),
                            vp.participants.clone(),
                            vp.items.len(),
                        )
                    } else {
                        (None, HashSet::new(), 0)
                    }
                };

                match task_content {
                    Some(content) => {
                        broadcast_swarm_plan(
                            &swarm_id,
                            Some("task_assigned".to_string()),
                            &swarm_plans,
                            &swarm_members,
                            &swarms_by_id,
                        )
                        .await;
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            req_session_id.clone(),
                            None,
                            Some(swarm_id.clone()),
                            SwarmEventType::PlanUpdate {
                                swarm_id: swarm_id.clone(),
                                item_count: plan_item_count,
                            },
                        )
                        .await;

                        // Notify target via soft interrupt
                        let coordinator_name = {
                            let members = swarm_members.read().await;
                            members
                                .get(&req_session_id)
                                .and_then(|m| m.friendly_name.clone())
                        };
                        let msg = if let Some(ref extra) = message {
                            format!(
                                "Task assigned to you by coordinator: {} — {}",
                                content, extra
                            )
                        } else {
                            format!("Task assigned to you by coordinator: {}", content)
                        };

                        let target_agent = {
                            let agent_sessions = sessions.read().await;
                            agent_sessions.get(&target_session).cloned()
                        };
                        if let Some(agent) = target_agent.as_ref() {
                            if let Ok(agent) = agent.try_lock() {
                                agent.queue_soft_interrupt(msg.clone(), false);
                            }
                        }
                        if let Some(member) = swarm_members.read().await.get(&target_session) {
                            let _ = member.event_tx.send(ServerEvent::Notification {
                                from_session: req_session_id.clone(),
                                from_name: coordinator_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some("dm".to_string()),
                                    channel: None,
                                },
                                message: msg,
                            });
                        }

                        // Headless sessions do not have a request loop to consume queued
                        // interrupts, so run assigned work immediately when no client owns it.
                        let target_has_client = {
                            let conns = client_connections.read().await;
                            conns.values().any(|c| c.session_id == target_session)
                        };
                        if !target_has_client {
                            if let Some(agent_arc) = target_agent {
                                let target_session_for_run = target_session.clone();
                                let swarm_members_for_run = Arc::clone(&swarm_members);
                                let swarms_for_run = Arc::clone(&swarms_by_id);
                                let swarm_plans_for_run = Arc::clone(&swarm_plans);
                                let swarm_id_for_run = swarm_id.clone();
                                let task_id_for_run = task_id.clone();
                                let event_history_for_run = Arc::clone(&event_history);
                                let event_counter_for_run = Arc::clone(&event_counter);
                                let swarm_event_tx_for_run = swarm_event_tx.clone();
                                let assignment_text = if let Some(extra) = message.clone() {
                                    format!(
                                        "{}\n\nAdditional coordinator instructions:\n{}",
                                        content, extra
                                    )
                                } else {
                                    content.clone()
                                };
                                tokio::spawn(async move {
                                    {
                                        let mut plans = swarm_plans_for_run.write().await;
                                        if let Some(vp) = plans.get_mut(&swarm_id_for_run) {
                                            if let Some(item) = vp
                                                .items
                                                .iter_mut()
                                                .find(|t| t.id == task_id_for_run)
                                            {
                                                item.status = "running".to_string();
                                                vp.version += 1;
                                            }
                                        }
                                    }
                                    broadcast_swarm_plan(
                                        &swarm_id_for_run,
                                        Some("task_running".to_string()),
                                        &swarm_plans_for_run,
                                        &swarm_members_for_run,
                                        &swarms_for_run,
                                    )
                                    .await;
                                    update_member_status(
                                        &target_session_for_run,
                                        "running",
                                        Some(truncate_detail(&assignment_text, 120)),
                                        &swarm_members_for_run,
                                        &swarms_for_run,
                                        Some(&event_history_for_run),
                                        Some(&event_counter_for_run),
                                        Some(&swarm_event_tx_for_run),
                                    )
                                    .await;

                                    let result = {
                                        let mut agent = agent_arc.lock().await;
                                        agent.run_once_capture(&assignment_text).await
                                    };

                                    match result {
                                        Ok(_) => {
                                            {
                                                let mut plans = swarm_plans_for_run.write().await;
                                                if let Some(vp) = plans.get_mut(&swarm_id_for_run) {
                                                    if let Some(item) = vp
                                                        .items
                                                        .iter_mut()
                                                        .find(|t| t.id == task_id_for_run)
                                                    {
                                                        item.status = "done".to_string();
                                                        vp.version += 1;
                                                    }
                                                }
                                            }
                                            broadcast_swarm_plan(
                                                &swarm_id_for_run,
                                                Some("task_completed".to_string()),
                                                &swarm_plans_for_run,
                                                &swarm_members_for_run,
                                                &swarms_for_run,
                                            )
                                            .await;
                                            update_member_status(
                                                &target_session_for_run,
                                                "completed",
                                                None,
                                                &swarm_members_for_run,
                                                &swarms_for_run,
                                                Some(&event_history_for_run),
                                                Some(&event_counter_for_run),
                                                Some(&swarm_event_tx_for_run),
                                            )
                                            .await;
                                        }
                                        Err(e) => {
                                            {
                                                let mut plans = swarm_plans_for_run.write().await;
                                                if let Some(vp) = plans.get_mut(&swarm_id_for_run) {
                                                    if let Some(item) = vp
                                                        .items
                                                        .iter_mut()
                                                        .find(|t| t.id == task_id_for_run)
                                                    {
                                                        item.status = "failed".to_string();
                                                        vp.version += 1;
                                                    }
                                                }
                                            }
                                            broadcast_swarm_plan(
                                                &swarm_id_for_run,
                                                Some("task_failed".to_string()),
                                                &swarm_plans_for_run,
                                                &swarm_members_for_run,
                                                &swarms_for_run,
                                            )
                                            .await;
                                            update_member_status(
                                                &target_session_for_run,
                                                "failed",
                                                Some(truncate_detail(&e.to_string(), 120)),
                                                &swarm_members_for_run,
                                                &swarms_for_run,
                                                Some(&event_history_for_run),
                                                Some(&event_counter_for_run),
                                                Some(&swarm_event_tx_for_run),
                                            )
                                            .await;
                                        }
                                    }
                                });
                            }
                        }

                        let plan_msg = format!(
                            "Plan updated: task '{}' assigned to {}.",
                            task_id, target_session
                        );
                        let members = swarm_members.read().await;
                        for sid in participant_ids {
                            if sid == target_session || sid == req_session_id {
                                continue;
                            }
                            if let Some(member) = members.get(&sid) {
                                let _ = member.event_tx.send(ServerEvent::Notification {
                                    from_session: req_session_id.clone(),
                                    from_name: coordinator_name.clone(),
                                    notification_type: NotificationType::Message {
                                        scope: Some("plan".to_string()),
                                        channel: None,
                                    },
                                    message: plan_msg.clone(),
                                });
                            }
                        }

                        let _ = client_event_tx.send(ServerEvent::Done { id });
                    }
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Task '{}' not found in swarm plan", task_id),
                            retry_after_secs: None,
                        });
                    }
                }
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
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    // Determine which sessions to watch
                    let watch_ids: Vec<String> = if requested_ids.is_empty() {
                        // Watch all non-self members in the swarm
                        let swarms = swarms_by_id.read().await;
                        swarms
                            .get(&swarm_id)
                            .map(|s| {
                                s.iter()
                                    .filter(|sid| *sid != &req_session_id)
                                    .cloned()
                                    .collect()
                            })
                            .unwrap_or_default()
                    } else {
                        requested_ids
                    };

                    if watch_ids.is_empty() {
                        let _ = client_event_tx.send(ServerEvent::CommAwaitMembersResponse {
                            id,
                            completed: true,
                            members: vec![],
                            summary: "No other members in swarm to wait for.".to_string(),
                        });
                    } else {
                        let timeout = std::time::Duration::from_secs(timeout_secs.unwrap_or(3600));
                        let swarm_members_clone = swarm_members.clone();
                        let mut event_rx = swarm_event_tx.subscribe();
                        let client_tx = client_event_tx.clone();

                        // Spawn a task that watches for status changes
                        tokio::spawn(async move {
                            let deadline = tokio::time::Instant::now() + timeout;

                            loop {
                                // Check current state
                                let (all_done, member_statuses) = {
                                    let members = swarm_members_clone.read().await;
                                    let statuses: Vec<crate::protocol::AwaitedMemberStatus> =
                                        watch_ids
                                            .iter()
                                            .map(|sid| {
                                                let (name, status) = members
                                                    .get(sid)
                                                    .map(|m| {
                                                        (m.friendly_name.clone(), m.status.clone())
                                                    })
                                                    .unwrap_or((None, "unknown".to_string()));
                                                let done = target_status.contains(&status)
                                                    || (status == "unknown"
                                                        && (target_status
                                                            .contains(&"stopped".to_string())
                                                            || target_status.contains(
                                                                &"completed".to_string(),
                                                            )));
                                                crate::protocol::AwaitedMemberStatus {
                                                    session_id: sid.clone(),
                                                    friendly_name: name,
                                                    status,
                                                    done,
                                                }
                                            })
                                            .collect();
                                    let all = statuses.iter().all(|s| s.done);
                                    (all, statuses)
                                };

                                if all_done {
                                    let done_names: Vec<String> = member_statuses
                                        .iter()
                                        .map(|m| {
                                            m.friendly_name.clone().unwrap_or_else(|| {
                                                m.session_id[..8.min(m.session_id.len())]
                                                    .to_string()
                                            })
                                        })
                                        .collect();
                                    let _ = client_tx.send(ServerEvent::CommAwaitMembersResponse {
                                        id,
                                        completed: true,
                                        members: member_statuses,
                                        summary: format!(
                                            "All {} members are done: {}",
                                            done_names.len(),
                                            done_names.join(", ")
                                        ),
                                    });
                                    return;
                                }

                                // Wait for next status change event or timeout
                                let remaining =
                                    deadline.saturating_duration_since(tokio::time::Instant::now());
                                if remaining.is_zero() {
                                    let pending: Vec<String> = member_statuses
                                        .iter()
                                        .filter(|m| !m.done)
                                        .map(|m| {
                                            let name =
                                                m.friendly_name.clone().unwrap_or_else(|| {
                                                    m.session_id[..8.min(m.session_id.len())]
                                                        .to_string()
                                                });
                                            format!("{} ({})", name, m.status)
                                        })
                                        .collect();
                                    let _ = client_tx.send(ServerEvent::CommAwaitMembersResponse {
                                        id,
                                        completed: false,
                                        members: member_statuses,
                                        summary: format!(
                                            "Timed out. Still waiting on: {}",
                                            pending.join(", ")
                                        ),
                                    });
                                    return;
                                }

                                // Wait for a relevant status_change event
                                match tokio::time::timeout(remaining, event_rx.recv()).await {
                                    Ok(Ok(event)) => {
                                        // Only recheck on status changes for watched sessions
                                        if let SwarmEventType::StatusChange { .. } = &event.event {
                                            if watch_ids.contains(&event.session_id) {
                                                continue; // Recheck at top of loop
                                            }
                                        }
                                        // Also recheck on member leave events
                                        if let SwarmEventType::MemberChange { action } =
                                            &event.event
                                        {
                                            if action == "left"
                                                && watch_ids.contains(&event.session_id)
                                            {
                                                continue;
                                            }
                                        }
                                    }
                                    Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                                        // Missed events, recheck state immediately
                                        continue;
                                    }
                                    Ok(Err(broadcast::error::RecvError::Closed)) => {
                                        let _ =
                                            client_tx.send(ServerEvent::CommAwaitMembersResponse {
                                                id,
                                                completed: false,
                                                members: member_statuses,
                                                summary: "Server shutting down.".to_string(),
                                            });
                                        return;
                                    }
                                    Err(_) => {
                                        // Timeout
                                        continue; // Will hit the timeout check at top
                                    }
                                }
                            }
                        });
                    }
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm. Use a git repository to enable swarm features."
                            .to_string(),
                        retry_after_secs: None,
                    });
                }
            }

            // These are handled via channels, not direct requests from TUI
            Request::ClientDebugCommand { id, .. } => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: "ClientDebugCommand is for internal use only".to_string(),
                    retry_after_secs: None,
                });
            }
            Request::ClientDebugResponse { id, output } => {
                let _ = client_debug_response_tx.send((id, output));
            }
        }
    }

    // Clean up: remove this client's session from the map
    // Mark session status and persist before dropping the agent
    let disconnected_while_processing = client_is_processing
        || processing_task
            .as_ref()
            .map(|handle| !handle.is_finished())
            .unwrap_or(false);
    {
        let mut sessions_guard = sessions.write().await;
        if let Some(agent_arc) = sessions_guard.remove(&client_session_id) {
            drop(sessions_guard);
            // Use try_lock with timeout to avoid deadlocking when agent mutex
            // is held by a stuck task (e.g., interrupted tool call).
            let lock_result =
                tokio::time::timeout(std::time::Duration::from_secs(2), agent_arc.lock()).await;

            match lock_result {
                Ok(mut agent) => {
                    if disconnected_while_processing {
                        agent
                            .mark_crashed(Some("Client disconnected while processing".to_string()));
                    } else {
                        agent.mark_closed();
                    }

                    let memory_enabled = agent.memory_enabled();
                    let transcript = if memory_enabled {
                        Some(agent.build_transcript_for_extraction())
                    } else {
                        None
                    };
                    let sid = client_session_id.clone();
                    drop(agent);
                    if let Some(transcript) = transcript {
                        crate::memory_agent::trigger_final_extraction(transcript, sid);
                    }
                }
                Err(_) => {
                    // Agent mutex still held — skip graceful shutdown for this session.
                    // The agent_arc is dropped here, which will eventually allow the
                    // mutex holder to finish.
                    crate::logging::warn(&format!(
                        "Session {} cleanup timed out waiting for agent lock (stuck task); skipping graceful shutdown",
                        client_session_id
                    ));
                }
            }
        }
    }

    // Clean up: remove from swarm tracking
    {
        let crashed = disconnected_while_processing;
        let (status, detail) = if crashed {
            ("crashed", Some("disconnect while running".to_string()))
        } else {
            ("stopped", Some("disconnected".to_string()))
        };
        update_member_status(
            &client_session_id,
            status,
            detail,
            &swarm_members,
            &swarms_by_id,
            Some(&event_history),
            Some(&event_counter),
            Some(&swarm_event_tx),
        )
        .await;

        let (swarm_id, removed_name) = {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.remove(&client_session_id) {
                (member.swarm_id, member.friendly_name)
            } else {
                (None, None)
            }
        };

        if let Some(ref swarm_id) = swarm_id {
            record_swarm_event(
                &event_history,
                &event_counter,
                &swarm_event_tx,
                client_session_id.clone(),
                removed_name.clone(),
                Some(swarm_id.clone()),
                SwarmEventType::MemberChange {
                    action: "left".to_string(),
                },
            )
            .await;
            remove_session_from_swarm(
                &client_session_id,
                swarm_id,
                &swarm_members,
                &swarms_by_id,
                &swarm_coordinators,
                &swarm_plans,
            )
            .await;
        }
    }

    // Clean up: remove client debug channel
    {
        let mut debug_state = client_debug_state.write().await;
        debug_state.unregister(&client_debug_id);
    }
    {
        let mut connections = client_connections.write().await;
        connections.remove(&client_connection_id);
    }
    // Clean up shutdown signal for this session
    {
        let mut signals = shutdown_signals.write().await;
        signals.remove(&client_session_id);
    }

    if let Some(handle) = processing_task.take() {
        handle.abort();
    }

    event_handle.abort();
    Ok(())
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
    event_tx: tokio::sync::mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    let mut agent = agent.lock().await;
    agent
        .run_once_streaming_mpsc(content, images, event_tx)
        .await
}
