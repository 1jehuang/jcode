//! Startup restoration of headless sessions.

use super::*;

impl Server {
    pub(super) async fn recover_headless_sessions_on_startup(&self) {
        let sessions_to_restore = {
            let members = self.swarm_state.members.read().await;
            members
                .values()
                .filter(|member| {
                    // Shutdown can persist `ready` after recording recovery
                    // intent. On load that becomes `stopped`, but the durable
                    // continuation must still take priority over that status.
                    member.is_headless
                        && (headless_member_should_restore(&member.status, true)
                            || reload_recovery::has_pending_for_session(&member.session_id))
                })
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
            let runtime_slot = self.sessions.reserve(&session_id).await;
            if runtime_slot.existing().await.is_some() {
                continue;
            }
            if let Err(error) = runtime_slot.ensure_vacant() {
                crate::logging::warn(&format!("Cannot recover session {session_id}: {error}"));
                continue;
            }
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

            if let Err(error) = runtime_slot.publish(Arc::clone(&agent)).await {
                crate::logging::warn(&format!(
                    "Cannot publish recovered session {session_id}: {error}"
                ));
                continue;
            }
            drop(runtime_slot);

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
                let lease = match agent.lock().await.claim_server_turn() {
                    Ok(lease) => lease,
                    Err(error) => {
                        crate::logging::info(&format!(
                            "Skipping duplicate startup recovery for {session_id}: {error}"
                        ));
                        return;
                    }
                };
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

                let (fallback, _) = mpsc::unbounded_channel();
                let events = self::state::TurnEventChannel::new(
                    session_id.clone(),
                    Arc::clone(&recover_swarm_members),
                    fallback,
                    &lease,
                );
                let result = self::turn_failover::process_reserved_message_with_server_failover(
                    Arc::clone(&agent),
                    &lease,
                    "",
                    vec![],
                    Some(reminder),
                    events.sender(),
                    true,
                )
                .await;

                let terminal = match &result {
                    Ok(()) => ServerEvent::Done { id: 0 },
                    Err(error) => ServerEvent::Error {
                        id: 0,
                        message: crate::util::format_error_chain(error),
                        retry_after_secs: None,
                    },
                };

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
                let _ = events.sender().send(terminal);
                events.finish().await;
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
}
