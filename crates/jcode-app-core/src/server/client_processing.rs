use super::*;

/// Retire finished turns before interpreting a request against client-local state.
/// The task publishes completion before its terminal event, and never waits on
/// this actor. Joining it also closes the cross-thread gap between those sends:
/// a new turn cannot start before the old terminal is on the ordered stream.
pub(super) async fn reconcile_processing_completions(
    mut completion: Option<ProcessingCompletion>,
    receiver: &mut mpsc::UnboundedReceiver<ProcessingCompletion>,
    state: &mut ProcessingState<'_>,
    connection_id: &str,
    connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm: &SwarmStatusRefs<'_>,
) {
    while let Some((done_id, result, completion_report, owner, _finalization)) =
        completion.take().or_else(|| receiver.try_recv().ok())
    {
        if Some(done_id) != *state.message_id
            || state.task.as_ref().map(|task| task.id()) != Some(owner)
        {
            crate::logging::warn(&format!(
                "Done event id={} doesn't match processing_message_id={:?}, dropping",
                done_id, state.message_id
            ));
            continue;
        }
        if let Some(task) = state.task.take()
            && let Err(error) = task.await
        {
            crate::logging::warn(&format!("Completed turn task failed: {error}"));
        }
        crate::logging::info(&format!(
            "Processing done for message id={}, result={}",
            done_id,
            if result.is_ok() { "ok" } else { "err" }
        ));
        *state.message_id = None;
        *state.client_is_processing = false;
        {
            let mut connections = connections.write().await;
            if let Some(info) = connections.get_mut(connection_id) {
                info.is_processing = false;
                info.current_tool_name = None;
            }
        }
        let done_session = state.session_id.take();
        record_processing_completion(done_session.as_deref(), result, completion_report, swarm)
            .await;
    }
}

pub(super) async fn record_processing_completion(
    done_session: Option<&str>,
    result: Result<()>,
    completion_report: Option<String>,
    swarm: &SwarmStatusRefs<'_>,
) {
    match result {
        Ok(()) => {
            if let Some(session_id) = done_session {
                update_member_status_with_report(
                    session_id,
                    "ready",
                    None,
                    completion_report,
                    swarm.members,
                    swarm.swarms_by_id,
                    Some(swarm.event_history),
                    Some(swarm.event_counter),
                    Some(swarm.event_tx),
                )
                .await;
            }
        }
        Err(e) => {
            if let Some(session_id) = done_session {
                update_member_status_with_report(
                    session_id,
                    "failed",
                    Some(truncate_detail(&e.to_string(), 120)),
                    completion_report,
                    swarm.members,
                    swarm.swarms_by_id,
                    Some(swarm.event_history),
                    Some(swarm.event_counter),
                    Some(swarm.event_tx),
                )
                .await;
            }
            let retry_after_secs = e
                .downcast_ref::<StreamError>()
                .and_then(|se| se.retry_after_secs);
            if retry_after_secs.is_some() {
                crate::telemetry::record_error(crate::telemetry::ErrorCategory::RateLimited);
            } else {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                if lower.contains("timeout") {
                    crate::telemetry::record_error(
                        crate::telemetry::ErrorCategory::ProviderTimeout,
                    );
                } else if crate::provider::error_looks_like_credential_failure(&msg)
                    || lower.contains("403 forbidden")
                {
                    // Use the shared credential-failure classifier instead of a
                    // bare `contains("auth")`: that substring also matched
                    // unrelated errors (e.g. any message mentioning "author" or
                    // OAuth flow noise) and inflated the auth_failed telemetry
                    // counter.
                    crate::telemetry::record_error(crate::telemetry::ErrorCategory::AuthFailed);
                }
            }
        }
    }
}

pub(super) async fn append_context_message(
    id: u64,
    content: &str,
    images: Vec<(String, String)>,
    client_session_id: &str,
    client_is_processing: bool,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let Ok(mut agent) = agent.try_lock() else {
        send_agent_busy_error(
            id,
            "context_message",
            client_session_id,
            client_is_processing,
            client_event_tx,
        );
        return;
    };
    let result = agent.append_user_context_message(content, images);
    let event = match result {
        Ok(()) => ServerEvent::ContextMessageAdded { id },
        Err(error) => ServerEvent::Error {
            id,
            message: crate::util::format_error_chain(&error),
            retry_after_secs: None,
        },
    };
    if let Err(error) = client_event_tx.send(event) {
        crate::logging::debug(&format!(
            "Client disconnected before event delivery: {error}"
        ));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn start_processing_message(
    message: ProcessingMessage,
    client_session_id: &str,
    state: &mut ProcessingState<'_>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    processing_done_tx: &mpsc::UnboundedSender<ProcessingCompletion>,
    client_terminal_env: Vec<(String, String)>,
    swarm: &SwarmStatusRefs<'_>,
) {
    let ProcessingMessage {
        id,
        content,
        images,
        system_reminder,
        active_skill,
        skill_arguments,
        server_failover,
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

    // Serialize recovery acceptance with the agent lock across connections.
    // A stale History frame may submit the same reminder after its first turn.
    let mut agent_guard = agent.lock().await;
    let turn_lease = match agent_guard.claim_server_turn() {
        Ok(lease) => lease,
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: error.to_string(),
                retry_after_secs: None,
            });
            return;
        }
    };
    if let Some(reminder) = system_reminder.as_deref() {
        match super::super::reload_recovery::is_delivered_continuation(client_session_id, reminder)
        {
            Ok(true) => {
                crate::logging::info(&format!(
                    "Suppressing delivered reload recovery replay session={} id={}",
                    client_session_id, id
                ));
                let _ = client_event_tx.send(ServerEvent::Done { id });
                return;
            }
            Ok(false) => {}
            Err(error) => crate::logging::warn(&format!(
                "Failed to inspect reload recovery replay session={}: {}",
                client_session_id, error
            )),
        }
    }
    if !agent_guard.set_remote_active_skill(active_skill.clone(), skill_arguments) {
        let skill_name = active_skill.as_deref().unwrap_or_default();
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!("Skill '{skill_name}' is not installed on the server"),
            retry_after_secs: None,
        });
        return;
    }

    if let Some(reminder) = system_reminder.as_deref()
        && let Err(error) = super::super::reload_recovery::mark_delivered_if_matching_continuation(
            client_session_id,
            reminder,
            "client_message_accepted",
        )
    {
        crate::logging::warn(&format!(
            "Failed to mark reload recovery intent delivered for accepted message session={} id={}: {}",
            client_session_id, id, error
        ));
    }
    drop(agent_guard);

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

    let owned_worker = swarm
        .members
        .read()
        .await
        .get(client_session_id)
        .is_some_and(|member| member.report_back_to_session_id.is_some());
    let (start_message_index, pending_queue) = {
        let mut agent_guard = agent.lock().await;
        if owned_worker {
            agent_guard.set_inline_output_tap(true);
        }
        (
            agent_guard.message_count(),
            agent_guard.soft_interrupt_queue(),
        )
    };
    let agent = Arc::clone(agent);
    let report_agent = Arc::clone(&agent);
    let events = super::super::state::TurnEventChannel::new(
        client_session_id.to_string(),
        Arc::clone(swarm.members),
        client_event_tx.clone(),
        &turn_lease,
    );
    let tx = events.sender();
    let done_tx = processing_done_tx.clone();
    crate::logging::info(&format!("Processing message id={} spawning task", id));
    *state.task = crate::runtime_activity::spawn_session(client_session_id, async move {
        let event_tx = tx.clone();
        let mut stop_reason = crate::protocol::TurnStopReason::Failure;
        let result = match std::panic::AssertUnwindSafe(crate::hooks::with_client_terminal_env(
            client_terminal_env,
            super::super::turn_failover::process_reserved_message_with_server_failover(
                agent,
                &turn_lease,
                &content,
                images,
                system_reminder,
                event_tx,
                server_failover,
            ),
        ))
        .catch_unwind()
        .await
        {
            Ok(result) => result,
            Err(panic_payload) => {
                stop_reason = crate::protocol::TurnStopReason::Crash;
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
        let completion_report = {
            let agent = report_agent.lock().await;
            agent.recent_assistant_output_tail_after(start_message_index)
        };
        // Keep the terminal event on the same ordered fanout channel as the
        // stream. Sending it later from the owning client's event loop could
        // race ahead of the final MessageEnd for newly attached clients.
        if let Err(error) = &result {
            let _ = tx.send(ServerEvent::TurnStopped {
                reason: stop_reason,
                message: crate::util::format_error_chain(error),
                provider_stop_reason: None,
            });
        }
        let terminal_event = match &result {
            Ok(()) => ServerEvent::Done { id },
            Err(error) => ServerEvent::Error {
                id,
                message: crate::util::format_error_chain(error),
                retry_after_secs: error
                    .downcast_ref::<StreamError>()
                    .and_then(|stream_error| stream_error.retry_after_secs),
            },
        };
        // Interrupts still queued here were never seen by this turn. Report
        // them on the ordered stream so a client can reconcile its own view.
        // Usually zero (the turn injects everything it can before ending), so
        // skip the event then and let clients default to zero.
        let pending = pending_queue.lock().map(|queue| queue.len()).unwrap_or(0);
        if pending > 0 {
            let _ = tx.send(ServerEvent::SoftInterruptQueue { pending });
        }
        // Publish completion first: observing Done/Error must imply that the
        // actor can reconcile this turn before dispatching the next request.
        // These sends do not await, and the terminal stays behind MessageEnd on
        // the same fanout channel (including for newly attached clients).
        // The actor owns final status publication after this task finishes.
        // Retain the reservation in its completion queue, so another connection
        // cannot start work before old ready/failed bookkeeping is settled.
        let _ = done_tx.send((
            id,
            result,
            completion_report,
            tokio::task::id(),
            Some(turn_lease.publication_pin()),
        ));
        let _ = tx.send(terminal_event);
        // Drain before releasing ownership: independently spawned fanout tasks
        // must not deliver an old terminal after a newer turn starts.
        events.finish().await;
        drop(turn_lease);
    });
}

pub(super) async fn cancel_processing_message(
    state: &mut ProcessingState<'_>,
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm: &SwarmStatusRefs<'_>,
    request_id: Option<u64>,
    request_decoded_at: Option<Instant>,
    sessions: Option<&SessionAgents>,
) {
    // Capture before cancellation releases the reservation. Modern clients
    // reject unscoped terminal events to protect newer turns from stale ones.
    let identity = sessions
        .and_then(|sessions| sessions.turn_observer(&session_control.session_id))
        .and_then(|observer| {
            let (latest, active) = observer.snapshot();
            active.or_else(|| {
                (latest > 0).then(|| crate::protocol::TurnIdentity {
                    runtime_generation: observer.runtime_generation().to_owned(),
                    turn_id: latest,
                })
            })
        });
    let scoped = |event| match &identity {
        Some(identity) => ServerEvent::TurnEvent {
            identity: identity.clone(),
            event: Box::new(event),
        },
        None => event,
    };
    let cancel_start = Instant::now();
    let session_label = state
        .session_id
        .as_deref()
        .unwrap_or(session_control.session_id.as_str())
        .to_string();
    // Stopping a session also stops the workers it spawned. This runs first
    // and on every path, including the idle one: a coordinator waiting on
    // `await_members` has no running turn of its own while its workers do.
    super::super::swarm::cancel_spawned_descendant_turns(
        &session_control.session_id,
        swarm.members,
        swarm.swarms_by_id,
        Some(swarm.event_history),
        Some(swarm.event_counter),
        Some(swarm.event_tx),
    )
    .await;
    crate::logging::info(&format!(
        "SERVER_INTERRUPT_CANCEL_RECEIVED request_id={:?} session={} control_session={} client_processing={} message_id={:?} has_task={} decoded_age_ms={:?}",
        request_id,
        session_label,
        session_control.session_id,
        *state.client_is_processing,
        *state.message_id,
        state.task.is_some(),
        request_decoded_at.map(|instant| instant.elapsed().as_millis())
    ));
    if let Some(mut handle) = state.task.take() {
        if handle.is_finished() {
            crate::logging::info(&format!(
                "SERVER_INTERRUPT_CANCEL_IGNORED_FINISHED request_id={:?} session={} message_id={:?} total_ms={}",
                request_id,
                session_label,
                *state.message_id,
                cancel_start.elapsed().as_millis()
            ));
            *state.task = Some(handle);
            return;
        }
        let stopped = ServerEvent::TurnStopped {
            reason: crate::protocol::TurnStopReason::Interrupted,
            message: "The turn was interrupted by a cancellation request.".into(),
            provider_stop_reason: None,
        };
        // Publish before signalling: the worker can finish cooperatively and
        // emit Done immediately after request_cancel.
        if super::super::state::fanout_session_event(swarm.members, &session_label, stopped.clone())
            .await
            == 0
        {
            let _ = client_event_tx.send(stopped);
        }
        let cancel_epoch = session_control.request_cancel();
        crate::logging::info(&format!(
            "SERVER_INTERRUPT_CANCEL_SIGNALLED request_id={:?} session={} message_id={:?} wait_ms=500",
            request_id, session_label, *state.message_id
        ));
        match tokio::time::timeout(std::time::Duration::from_millis(500), &mut handle).await {
            Ok(_) => {
                crate::logging::info(&format!(
                    "SERVER_INTERRUPT_CANCEL_COOPERATIVE_DONE request_id={:?} session={} message_id={:?} elapsed_ms={}",
                    request_id,
                    session_label,
                    *state.message_id,
                    cancel_start.elapsed().as_millis()
                ));
            }
            Err(_) => {
                crate::logging::warn(&format!(
                    "SERVER_INTERRUPT_CANCEL_COOPERATIVE_TIMEOUT request_id={:?} session={} message_id={:?} elapsed_ms={} action=abort_task",
                    request_id,
                    session_label,
                    *state.message_id,
                    cancel_start.elapsed().as_millis()
                ));
                handle.abort();
                match tokio::time::timeout(std::time::Duration::from_millis(2000), handle).await {
                    Ok(_) => crate::logging::info(&format!(
                        "SERVER_INTERRUPT_CANCEL_ABORT_RELEASED request_id={:?} session={} elapsed_ms={}",
                        request_id,
                        session_label,
                        cancel_start.elapsed().as_millis()
                    )),
                    Err(_) => crate::logging::warn(&format!(
                        "SERVER_INTERRUPT_CANCEL_ABORT_RELEASE_TIMEOUT request_id={:?} session={} elapsed_ms={} wait_ms=2000",
                        request_id,
                        session_label,
                        cancel_start.elapsed().as_millis()
                    )),
                }
            }
        }
        // Only clear the cancel we fired: a newer cancel (repeated Esc, jade
        // relay, another connection) must not be erased before its target
        // observes it (issue #428).
        session_control.reset_cancel_if_epoch(cancel_epoch);
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
            let _ = client_event_tx.send(scoped(ServerEvent::Interrupted));
            let _ = client_event_tx.send(scoped(ServerEvent::Done { id: message_id }));
            crate::logging::info(&format!(
                "SERVER_INTERRUPT_CANCEL_EVENTS_EMITTED request_id={:?} session={} interrupted=true done_id={} total_ms={}",
                request_id,
                session_label,
                message_id,
                cancel_start.elapsed().as_millis()
            ));
        }
    } else {
        crate::logging::warn(&format!(
            "SERVER_INTERRUPT_CANCEL_NO_LOCAL_TASK request_id={:?} session={} control_session={} client_processing={} message_id={:?}; signalling session cancel handle anyway",
            request_id,
            session_label,
            session_control.session_id,
            *state.client_is_processing,
            *state.message_id
        ));
        // Nothing is running anywhere for this session, so there is no turn to
        // interrupt and arming the signal can only harm the *next* one: the
        // deferred reset below runs 500ms later, and a message sent inside
        // that window starts with the cancel flag already set and dies
        // immediately, with no reply and no error. Report the interrupt and
        // stop. Sessions whose turn is owned by another connection still take
        // the signalling path, since the registry sees those turns.
        if !crate::turn_cancel_registry::has_active_turn(&session_control.session_id) {
            crate::logging::info(&format!(
                "SERVER_INTERRUPT_CANCEL_IDLE_NOOP request_id={:?} session={}",
                request_id, session_label
            ));
            *state.client_is_processing = false;
            let _ = client_event_tx.send(scoped(ServerEvent::Interrupted));
            if let Some(message_id) = state.message_id.take() {
                let _ = client_event_tx.send(scoped(ServerEvent::Done { id: message_id }));
            }
            return;
        }
        let stopped = ServerEvent::TurnStopped {
            reason: crate::protocol::TurnStopReason::Interrupted,
            message: "The turn was interrupted by a cancellation request.".into(),
            provider_stop_reason: None,
        };
        // Publish before signalling: the worker can finish cooperatively and
        // emit Done immediately after request_cancel.
        if super::super::state::fanout_session_event(swarm.members, &session_label, stopped.clone())
            .await
            == 0
        {
            let _ = client_event_tx.send(stopped);
        }
        let cancel_epoch = session_control.request_cancel();
        let reset_control = session_control.clone();
        tokio::spawn(async move {
            // The running turn is not owned by this connection (post-reload
            // recovery, server-initiated turn, or attach), so we cannot await
            // it. Clear the flag later so the *next* turn is not aborted by a
            // stale cancel, but only if no newer cancel fired in the meantime:
            // an unconditional reset here used to erase rapid repeated Esc
            // cancels before the busy turn observed them (issue #428).
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            reset_control.reset_cancel_if_epoch(cancel_epoch);
        });
        *state.client_is_processing = false;
        let status_session_id = state
            .session_id
            .take()
            .unwrap_or_else(|| session_control.session_id.clone());
        update_member_status(
            &status_session_id,
            "stopped",
            Some("cancelled".to_string()),
            swarm.members,
            swarm.swarms_by_id,
            Some(swarm.event_history),
            Some(swarm.event_counter),
            Some(swarm.event_tx),
        )
        .await;
        let _ = client_event_tx.send(scoped(ServerEvent::Interrupted));
        if let Some(message_id) = state.message_id.take() {
            let _ = client_event_tx.send(scoped(ServerEvent::Done { id: message_id }));
            crate::logging::info(&format!(
                "SERVER_INTERRUPT_CANCEL_EVENTS_EMITTED request_id={:?} session={} interrupted=true done_id={} total_ms={}",
                request_id,
                session_label,
                message_id,
                cancel_start.elapsed().as_millis()
            ));
        } else {
            crate::logging::info(&format!(
                "SERVER_INTERRUPT_CANCEL_EVENTS_EMITTED request_id={:?} session={} interrupted=true done_id=None total_ms={}",
                request_id,
                session_label,
                cancel_start.elapsed().as_millis()
            ));
        }
    }
}

pub(super) async fn try_available_models_snapshot(agent: &Arc<Mutex<Agent>>) -> Option<Arc<str>> {
    Some(Arc::clone(
        &try_available_models_catalog_offloaded(agent)
            .await?
            .dedup_key,
    ))
}

pub(super) fn queue_soft_interrupt(
    id: u64,
    content: String,
    images: Vec<(String, String)>,
    urgent: bool,
    client_id: Option<String>,
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let source = SoftInterruptSource::User;
    let content_bytes = content.len();
    let content_chars = content.chars().count();
    crate::logging::info(&format!(
        "SERVER_SOFT_INTERRUPT_QUEUE_REQUEST id={} session={} source={:?} urgent={} content_bytes={} content_chars={} client_id={}",
        id,
        session_control.session_id,
        source,
        urgent,
        content_bytes,
        content_chars,
        client_id.is_some()
    ));
    let queued =
        session_control.queue_soft_interrupt_with_id(content, images, urgent, source, client_id);
    let ack_queued = client_event_tx.send(ServerEvent::Ack { id }).is_ok();
    crate::logging::info(&format!(
        "SERVER_SOFT_INTERRUPT_QUEUE_RESULT id={} session={} queued={} ack_queued={}",
        id, session_control.session_id, queued, ack_queued
    ));
}

pub(super) fn cancel_soft_interrupts_by_id(
    id: u64,
    session_id: &str,
    client_ids: &[String],
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let (mut cancelled, mut not_queued) = session_control.cancel_soft_interrupts_by_id(client_ids);
    // An interrupt sent while no live queue was reachable is parked on disk
    // until the session restores it. It is just as cancellable.
    if !not_queued.is_empty() {
        match crate::soft_interrupt_store::remove_by_client_ids(session_id, &not_queued) {
            Ok(removed) if !removed.is_empty() => {
                not_queued.retain(|id| !removed.contains(id));
                cancelled.extend(removed);
            }
            Ok(_) => {}
            Err(err) => crate::logging::warn(&format!(
                "SERVER_SOFT_INTERRUPT_CANCEL_BY_ID_PERSISTED_FAILED id={} session={} error={}",
                id, session_id, err
            )),
        }
    }
    let _ = client_event_tx.send(ServerEvent::SoftInterruptsCancelled {
        id,
        cancelled,
        not_queued,
    });
}

pub(super) fn clear_soft_interrupts(
    id: u64,
    session_id: &str,
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    crate::logging::info(&format!(
        "SERVER_SOFT_INTERRUPT_CLEAR_REQUEST id={} session={} control_session={}",
        id, session_id, session_control.session_id
    ));
    session_control.clear_soft_interrupts();
    let persisted_clear = match crate::soft_interrupt_store::clear(session_id) {
        Ok(()) => true,
        Err(err) => {
            crate::logging::warn(&format!(
                "SERVER_SOFT_INTERRUPT_CLEAR_PERSISTED_FAILED id={} session={} error={}",
                id, session_id, err
            ));
            false
        }
    };
    let ack_queued = client_event_tx.send(ServerEvent::Ack { id }).is_ok();
    crate::logging::info(&format!(
        "SERVER_SOFT_INTERRUPT_CLEAR_RESULT id={} session={} persisted_clear={} ack_queued={}",
        id, session_id, persisted_clear, ack_queued
    ));
}

pub(super) fn move_tool_to_background(
    id: u64,
    session_control: &SessionControlHandle,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    crate::logging::info(&format!(
        "SERVER_BACKGROUND_TOOL_REQUEST id={} session={}",
        id, session_control.session_id
    ));
    let running = crate::running_tool_registry::current(&session_control.session_id);
    // Fire regardless: the agent resets the signal before every tool wait, so
    // an idle fire is harmless, and this keeps the pre-#1778 behavior.
    let signalled = session_control.request_background_current_tool();
    let ack_queued = client_event_tx.send(ServerEvent::Ack { id }).is_ok();
    let running = running.filter(|_| signalled);
    let moved = running.is_some();
    let (tool_call_id, tool_name) = match running {
        Some(tool) => (Some(tool.call_id), Some(tool.name)),
        None => (None, None),
    };
    let _ = client_event_tx.send(ServerEvent::BackgroundToolResult {
        id,
        moved,
        tool_call_id,
        tool_name,
    });
    crate::logging::info(&format!(
        "SERVER_BACKGROUND_TOOL_RESULT id={} session={} signalled={} ack_queued={}",
        id, session_control.session_id, signalled, ack_queued
    ));
}
