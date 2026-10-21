//! Reconstruct headless runtimes, then hand their unfinished turns to the same
//! publication, retry, and completion machinery used by ordinary server wakes.
use super::live_turn::{LiveTurnSwarmContext, complete_live_turn};
use super::usage_limit_resume::{ResumeOutcome, ResumeState};
use super::*;

static RECOVERY: Mutex<()> = Mutex::const_new(());

pub(super) async fn recover(server: &Server) {
    // A losing startup sweep must not supersede the accepted worker turn.
    let _recovery = RECOVERY.lock().await;
    let candidates = {
        let members = server.swarm_state.members.read().await;
        members
            .values()
            .filter_map(|member| {
                let paused = matches!(
                    member.status.as_str(),
                    "stopped" | "completed" | "done" | "failed" | "suspended" | "paused"
                );
                if member.is_headless && paused {
                    usage_limit_resume::cancel_pending_resume(&member.session_id);
                }
                (member.is_headless
                    && !paused
                    && (headless_member_should_restore(&member.status, true)
                        || usage_limit_resume::has_pending_resume(&member.session_id)))
                .then(|| member.session_id.clone())
            })
            .collect::<Vec<_>>()
    };
    if candidates.is_empty() {
        return;
    }
    if let Some(delay) = startup_headless_recovery_test_delay() {
        tokio::time::sleep(delay).await;
    }
    let pool = get_shared_mcp_pool(&server.mcp_pool).await;
    let started = Instant::now();
    let mut stats = HeadlessRecoveryStats::default();
    for session_id in candidates {
        stats.candidates += 1;
        if server.sessions.read().await.contains_key(&session_id) {
            continue;
        }
        let pending_wait = match usage_limit_resume::pending_state(&session_id) {
            Ok(state) => state,
            Err(error) => {
                recovery_failed(server, &session_id, &error).await;
                stats.failed_to_load += 1;
                continue;
            }
        };
        let session = match crate::session::Session::load(&session_id) {
            Ok(session) => session,
            Err(error) => {
                recovery_failed(server, &session_id, &error).await;
                stats.failed_to_load += 1;
                continue;
            }
        };
        let previous_status = session.status.clone();
        let provider = server.provider.fork();
        let registry = crate::tool::Registry::new(Arc::clone(&provider)).await;
        if session.is_canary {
            registry.register_selfdev_tools().await;
        }
        registry
            .register_mcp_tools_for_dir(
                None,
                Some(Arc::clone(&pool)),
                Some("headless".into()),
                session.working_dir.as_ref().map(std::path::PathBuf::from),
            )
            .await;
        let agent = Arc::new(Mutex::new(Agent::new_with_session(
            Arc::clone(&provider),
            registry,
            session,
            None,
        )));
        client_state::register_agent_provider(&agent, provider);
        // Reserve before exposing the runtime. Concurrent startup sweeps, owner
        // resumes, and completion wakes must never enqueue a second continuation.
        let held = Arc::clone(&agent).lock_owned().await;
        let stored_directive = reload_recovery::pending_directive_for_session(&session_id)
            .ok()
            .flatten();
        let pending_conversation = held.messages().last().is_some_and(|message| {
            message.role == crate::message::Role::User
                || message
                    .content
                    .iter()
                    .any(|block| matches!(block, crate::message::ContentBlock::ToolUse { .. }))
        });
        let should_resume = pending_wait.is_some()
            || stored_directive.is_some()
            || pending_conversation
            || client_session::restored_session_was_interrupted(
                &session_id,
                &previous_status,
                &held,
            );
        let publication = swarm::begin_turn_publication(&session_id).await;
        let mut members = server.swarm_state.members.write().await;
        let Some(member) = members.get_mut(&session_id) else {
            continue;
        };
        if matches!(
            member.status.as_str(),
            "stopped" | "completed" | "done" | "failed" | "suspended" | "paused"
        ) {
            usage_limit_resume::cancel_pending_resume(&session_id);
            continue;
        }
        let mut sessions = server.sessions.write().await;
        if sessions.contains_key(&session_id) {
            continue;
        }
        // Recovered records intentionally have closed senders. Install a live
        // headless drain, just as creation of a fresh headless worker does.
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        member.event_tx = event_tx;
        tokio::spawn(async move { while event_rx.recv().await.is_some() {} });
        sessions.insert(session_id.clone(), Arc::clone(&agent));
        drop(sessions);
        drop(members);
        register_session_interrupt_queue(
            &server.soft_interrupt_queues,
            &session_id,
            held.soft_interrupt_queue(),
        )
        .await;
        server
            .shutdown_signals
            .write()
            .await
            .insert(session_id.clone(), held.graceful_shutdown_signal());
        register_background_tool_signal(&session_id, held.background_tool_signal());
        if !should_resume || (held.message_count() == 0 && pending_wait.is_none()) {
            stats.skipped += 1;
            update_member_status(
                &session_id,
                "ready",
                None,
                &server.swarm_state.members,
                &server.swarm_state.swarms_by_id,
                Some(&server.event_history),
                Some(&server.event_counter),
                Some(&server.swarm_event_tx),
            )
            .await;
            let swarm_id = {
                let members = server.swarm_state.members.read().await;
                members
                    .get(&session_id)
                    .and_then(|member| member.swarm_id.clone())
            };
            if let Some(swarm_id) = swarm_id {
                persist_swarm_state_for(&swarm_id, &server.swarm_state).await;
            }
            continue;
        }
        let mut swarm = LiveTurnSwarmContext::new(
            &server.swarm_state.members,
            &server.swarm_state.swarms_by_id,
            &server.event_history,
            &server.event_counter,
            &server.swarm_event_tx,
        );
        swarm.recovery_swarm = Some(server.swarm_state.clone());
        swarm.recovered_assignment =
            recovered_assignment::RecoveredAssignment::capture(&session_id, &server.swarm_state)
                .await;
        swarm.recovery_reload_id = reload_recovery::peek_for_session(&session_id)
            .ok()
            .flatten()
            .map(|record| record.reload_id);
        if let Some(reload_id) = &swarm.recovery_reload_id {
            reload_trace::record_value(
                reload_id,
                "startup_recovery_decision",
                serde_json::json!({
                    "session_id": session_id, "should_resume": true, "is_headless": true,
                    "restored_provider_wait": pending_wait.is_some(),
                }),
            );
            reload_trace::record_value(
                reload_id,
                "continuation_started",
                serde_json::json!({
                    "session_id": session_id, "source": "server_startup_headless",
                }),
            );
        }
        if let Some(state) = pending_wait {
            // The wait owns its original transcript and absolute deadline. A
            // reload reminder here would create a different user turn.
            spawn_wait_restore(&session_id, held, state, swarm, publication).await;
        } else {
            let reminder = stored_directive
                .as_ref()
                .map(|directive| directive.continuation_message.clone())
                .or_else(|| {
                    headless_reload_continuation_message(
                        ReloadContext::load_for_session(&session_id).ok().flatten(),
                    )
                });
            let Some(reminder) = reminder else {
                stats.skipped += 1;
                continue;
            };
            super::live_turn::spawn_tracked_live_turn_with_publication(
                &session_id,
                held,
                String::new(),
                Some(reminder),
                None,
                Some("resuming after reload".into()),
                swarm,
                publication,
            )
            .await;
        }
        if let Some(directive) = stored_directive {
            if let Err(error) = reload_recovery::mark_delivered_if_matching_continuation(
                &session_id,
                &directive.continuation_message,
                "server_startup_headless",
            ) {
                crate::logging::warn(&format!(
                    "Could not acknowledge recovery for {session_id}: {error}"
                ));
            }
        }
        stats.resumed += 1;
        ReloadContext::log_recovery_outcome(
            "server_startup_headless",
            &session_id,
            "resuming",
            "restored tracked headless turn",
        );
    }
    crate::logging::info(&format!(
        "[TIMING] headless reload startup recovery: candidates={}, resumed={}, skipped={}, failed_to_load={}, total={}ms",
        stats.candidates,
        stats.resumed,
        stats.skipped,
        stats.failed_to_load,
        started.elapsed().as_millis()
    ));
}

async fn recovery_failed(server: &Server, session_id: &str, error: &anyhow::Error) {
    crate::logging::warn(&format!(
        "Failed to recover headless session {session_id}: {error}"
    ));
    update_member_status(
        session_id,
        "failed",
        Some(truncate_detail(&error.to_string(), 120)),
        &server.swarm_state.members,
        &server.swarm_state.swarms_by_id,
        Some(&server.event_history),
        Some(&server.event_counter),
        Some(&server.swarm_event_tx),
    )
    .await;
    let swarm_id = {
        let members = server.swarm_state.members.read().await;
        members
            .get(session_id)
            .and_then(|member| member.swarm_id.clone())
    };
    if let Some(swarm_id) = swarm_id {
        persist_swarm_state_for(&swarm_id, &server.swarm_state).await;
    }
}

async fn spawn_wait_restore(
    session_id: &str,
    held: tokio::sync::OwnedMutexGuard<Agent>,
    state: ResumeState,
    swarm: LiveTurnSwarmContext,
    publication: Arc<swarm::TurnPublication>,
) {
    let agent = tokio::sync::OwnedMutexGuard::mutex(&held).clone();
    let session_id = session_id.to_string();
    let event_tx = session_event_fanout_sender(session_id.clone(), Arc::clone(&swarm.members));
    tokio::spawn(async move {
        let assignment = swarm.recovered_assignment.clone();
        let on_wait = move |plan: &usage_limit_resume::UsageLimitResumePlan| -> futures::future::BoxFuture<'static, ()> {
            let assignment = assignment.clone();
            let plan = plan.clone();
            Box::pin(async move { if let Some(assignment) = assignment { assignment.on_wait(&plan).await; } })
        };
        let outcome = usage_limit_resume::restore_turn_after_provider_wait(
            agent,
            Some(held),
            state,
            &event_tx,
            &swarm,
            &on_wait,
            Some(&publication),
        )
        .await;
        let (result, report, reservation) = match outcome {
            ResumeOutcome::Completed {
                guard,
                completion_report,
            } => (Ok(()), completion_report, Some(guard)),
            ResumeOutcome::Failed {
                error,
                retry_after_secs,
                guard,
            } => (Err((error, retry_after_secs)), None, guard),
            ResumeOutcome::NotUsageLimit { error, guard } => (Err((error, None)), None, guard),
            ResumeOutcome::Superseded => return,
        };
        complete_live_turn(
            &session_id,
            &publication,
            result,
            report,
            crate::protocol::TurnStopReason::Failure,
            &event_tx,
            &swarm,
        )
        .await;
        drop(reservation);
    });
}
