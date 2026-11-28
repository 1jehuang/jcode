//! Remote follow-up processing.

use super::*;

pub(in super::super) async fn process_remote_followups(
    app: &mut App,
    remote: &mut RemoteConnection,
) {
    if app
        .remote_queue_retry_not_before
        .is_some_and(|deadline| Instant::now() < deadline)
    {
        return;
    }
    app.remote_queue_retry_not_before = None;
    if app.prepare_local_tools(remote).await.is_err() {
        app.revoke_cloud_environment();
        return;
    }
    if app.cloud_handoff_blocks_work() {
        return;
    }
    if app.pending_provider_failover.is_some()
        || app.pending_automatic_failover_route.is_some()
        || app.pending_automatic_failover_request_id.is_some()
        || (app.pending_route_selection.is_some() && app.pending_fallback_resend.is_some())
        || (app.remote_model_switch_in_flight && app.pending_fallback_resend.is_some())
    {
        return;
    }
    // A pending *server* reload must be dispatched even when the bootstrap
    // History payload was intentionally deferred. The runtime-identity /
    // stale-binary guard in the History handler sets `pending_server_reload =
    // true` and returns WITHOUT marking history as loaded, by design: we want to
    // reload the server before applying any session state. If the
    // `has_loaded_history` gate below ran first, the reload would never fire,
    // history would stay unloaded forever, and every typed prompt would be stuck
    // behind "Loading session..." until the user restarted (server/client binary
    // mismatch reload-handoff stall).
    if app.pending_server_reload && !app.is_processing {
        dispatch_pending_server_reload(app, remote).await;
        return;
    }

    // Drain Subscribe's History before sending the staged startup prompt.
    // Both directions can carry large image payloads: awaiting a prompt write
    // here would prevent this event loop from reading History while the server
    // is still writing it, deadlocking both peers on socket backpressure.
    if !remote.has_loaded_history() {
        note_startup_submit_deferred(app, "remote history not loaded yet");
        return;
    }

    // Esc redirected to a pending prompt: the server sends Done before
    // Interrupted. Sending now would let the late Interrupted end the new turn.
    if app.awaiting_remote_interrupt_ack() {
        return;
    }

    let _ = recover_stranded_soft_interrupts(app, remote).await;
    if app.soft_interrupt_recovery_in_flight() {
        return;
    }

    if app.pending_queued_dispatch {
        note_startup_submit_deferred(app, "pending_queued_dispatch in progress");
        return;
    }

    if !app.remote_model_switch_in_flight
        && !app.is_processing
        && let Some(payload) = app.pending_fallback_resend.take()
    {
        // A fallback offer was accepted and the server confirmed the route
        // switch: resend the failed turn's payload on the new route. The
        // original user message is already in the transcript from the failed
        // attempt, so do not echo it again; just clear the input box if the
        // error path restored the prompt there.
        if let Some(raw_input) = payload.raw_input.as_deref()
            && app.input == raw_input
        {
            app.input.clear();
            app.cursor_pos = 0;
        }
        app.last_submitted_input = payload.raw_input.clone();
        crate::logging::info("Resending failed turn after accepted fallback route switch");
        app.log_recovery_event(
            crate::logging::LogLevel::Info,
            "RECOVERY_FALLBACK_DISPATCH",
            "route_confirmed",
        );
        let recovery_payload = payload.clone();
        if let Err(error) = begin_remote_send(
            app,
            remote,
            payload.content,
            payload.images,
            payload.is_system,
            payload.system_reminder,
            payload.auto_retry,
            payload.retry_attempts.saturating_add(1),
        )
        .await
        {
            app.log_recovery_event(
                crate::logging::LogLevel::Warn,
                "RECOVERY_FALLBACK_SEND_FAILED",
                "transport_write_failed",
            );
            app.restore_failover_payload(recovery_payload);
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to resend after fallback switch: {}",
                error
            )));
            app.set_status_notice("Fallback resend failed");
        }
        return;
    }

    if !app.remote_model_switch_in_flight
        && !app.auth_catalog_refresh_pending
        && !app.is_processing
        && let Some(prepared) = app.pending_prompt_after_model_switch.take()
    {
        if let Err(error) = submit_prepared_remote_input(app, remote, prepared).await {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to submit prompt after model switch: {}",
                error
            )));
            app.set_status_notice("Queued prompt failed");
        }
        return;
    }

    // Now that history is loaded (guaranteed by the gate above), dispatch any
    // prompt that the user submitted during the pre-history window. Submitting
    // it earlier would have been clobbered by the bootstrap History payload's
    // `session_changed` clear; deferring here fixes the intermittent
    // "first prompt vanishes / weird render" bug.
    if !app.is_processing
        && let Some(prepared) = app.pending_prompt_before_history.take()
    {
        crate::logging::info(
            "Dispatching prompt that was held until remote history finished loading",
        );
        if let Err(error) = submit_prepared_remote_input(app, remote, prepared).await {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to submit prompt after session load: {}",
                error
            )));
            app.set_status_notice("Prompt failed");
        }
        return;
    }

    let synthetic_startup_dispatch = app.is_processing
        // Only a locally staged send is synthetic. A resumed/external turn
        // has no request id either, and its resume marker is cleared as soon
        // as live stream events arrive. Never demote that running turn just
        // because a follow-up is queued.
        && matches!(app.status, ProcessingStatus::Sending)
        && app.current_message_id.is_none()
        && app.remote_resume_activity.is_none()
        && (app.submit_input_on_startup
            || !app.queued_messages.is_empty()
            || !app.hidden_queued_system_messages.is_empty());

    if synthetic_startup_dispatch {
        crate::logging::info(
            "Dispatching restored startup/queued followup without active remote message id",
        );
        app.is_processing = false;
        app.status = ProcessingStatus::Idle;
        app.processing_started = None;
        app.clear_visible_turn_started();
        app.replay_processing_started_ms = None;
        app.replay_elapsed_override = None;
    }

    if app.submit_input_on_startup && !app.is_processing {
        app.submit_input_on_startup = false;
        app.startup_submit_deferred_reason = None;
        if !app.input.is_empty() || !app.pending_images.is_empty() {
            crate::logging::info(&format!(
                "Startup auto-submit firing: input_chars={} pending_images={}",
                app.input.chars().count(),
                app.pending_images.len(),
            ));
            let prepared = input::take_prepared_input(app);
            if let Err(error) = submit_prepared_remote_input(app, remote, prepared).await {
                crate::logging::warn(&format!("Startup auto-submit failed: {error}"));
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to submit startup prompt: {}",
                    error
                )));
                app.set_status_notice("Startup prompt failed");
            }
            return;
        } else {
            crate::logging::warn(
                "Startup auto-submit skipped: submit flag was set but input and pending images are both empty",
            );
        }
    } else if app.submit_input_on_startup && app.is_processing {
        note_startup_submit_deferred(app, "session still processing (is_processing=true)");
    }

    if app.pending_background_client_reload.is_some() && !app.is_processing {
        app.maybe_finish_background_client_reload();
        return;
    }

    if app.pending_split_request && !app.is_processing {
        app.pending_split_request = false;
        let flow_label = app
            .pending_split_label
            .clone()
            .unwrap_or_else(|| "Split".to_string());
        begin_remote_split_launch(app, &flow_label);
        if let Err(error) = remote.split().await {
            finish_remote_split_launch(app);
            let had_startup = app.pending_split_startup_message.take().is_some();
            app.pending_split_parent_session_id = None;
            let had_prompt = app.pending_split_prompt.take().is_some();
            let label = app.pending_split_label.take();
            app.pending_split_model_override = None;
            app.pending_split_provider_key_override = None;
            let flow_label = label.unwrap_or(flow_label);
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to launch {} session: {}",
                flow_label.to_lowercase(),
                error
            )));
            if had_startup || had_prompt {
                app.set_status_notice(format!("{} launch failed", flow_label));
            }
        }
        return;
    }

    if app.pending_transfer_request && !app.is_processing {
        app.pending_transfer_request = false;
        let flow_label = app
            .pending_split_label
            .clone()
            .unwrap_or_else(|| "Transfer".to_string());
        begin_remote_split_launch(app, &flow_label);
        if let Err(error) = remote.transfer().await {
            finish_remote_split_launch(app);
            let label = app.pending_split_label.take().unwrap_or(flow_label);
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to launch {} session: {}",
                label.to_lowercase(),
                error
            )));
            app.set_status_notice(format!("{} launch failed", label));
        }
        return;
    }

    if app.is_processing {
        if let Some(interleave_msg) = app.interleave_message.take()
            && !interleave_msg.trim().is_empty()
        {
            let interleave_images = std::mem::take(&mut app.interleave_images);
            let msg_clone = interleave_msg.clone();
            let images_clone = interleave_images.clone();
            match remote
                .soft_interrupt(interleave_msg, interleave_images, false)
                .await
            {
                Err(e) => {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to queue soft interrupt: {}",
                        e
                    )));
                }
                Ok(request_id) => {
                    app.track_pending_soft_interrupt_with_images(
                        request_id,
                        msg_clone,
                        images_clone,
                    );
                }
            }
        }
        return;
    }

    if !app.queued_commands.is_empty() {
        run_next_queued_remote_command(app, remote).await;
        return;
    }

    if let Some(interleave_msg) = app.interleave_message.take() {
        // Carry the staged attachments through. A local revert of #627 had this
        // passing `vec![]`, which silently dropped every image on an interleaved
        // send while still compiling, so the comment marks why the take matters.
        let interleave_images = std::mem::take(&mut app.interleave_images);
        if !interleave_msg.trim().is_empty() {
            app.automatic_failover_targets.clear();
            app.automatic_failover_stopped = false;
            app.push_display_message(DisplayMessage {
                role: "user".to_string(),
                content: interleave_msg.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            if let Err(e) = begin_remote_send(
                app,
                remote,
                interleave_msg,
                interleave_images,
                false,
                None,
                false,
                0,
            )
            .await
            {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to send message: {}",
                    e
                )));
            }
        }
    } else if !app.queued_messages.is_empty() && !app.reload_recovery_blocks_queue() {
        let mut queued_messages = std::mem::take(&mut app.queued_messages);
        super::super::commands::drop_stale_incomplete_todo_pokes(app, &mut queued_messages);
        let hidden_reminders = std::mem::take(&mut app.hidden_queued_system_messages);
        let (messages, reminder, display_system_messages) =
            super::super::helpers::partition_queued_messages(queued_messages, hidden_reminders);
        if messages.is_empty() && reminder.is_none() {
            return;
        }
        let combined = messages.join("\n\n");
        let preserve_visible_turn =
            super::super::commands::queued_messages_are_only_pokes(&messages);
        if !messages.is_empty() && !preserve_visible_turn {
            app.automatic_failover_targets.clear();
            app.automatic_failover_stopped = false;
        }
        let auto_retry = reminder.is_some() && messages.is_empty();
        for msg in display_system_messages {
            app.push_display_message(DisplayMessage::system(msg));
        }
        for msg in &messages {
            if !super::super::commands::is_poke_message(msg) {
                app.push_display_message(DisplayMessage::user(msg.clone()));
            }
        }
        if !combined.is_empty() {
            if preserve_visible_turn {
                app.visible_turn_started.get_or_insert_with(Instant::now);
            } else {
                app.visible_turn_started = Some(Instant::now());
            }
        }
        let images = std::mem::take(&mut app.queued_images);
        if begin_remote_send(
            app,
            remote,
            combined.clone(),
            images.clone(),
            true,
            reminder.clone(),
            auto_retry,
            0,
        )
        .await
        .is_err()
        {
            // Do not drop a dequeued follow-up whose send never reached the
            // server (issue #391); restore it for redispatch after reconnect.
            crate::logging::error(
                "Failed to send queued continuation message; restoring it to the queue",
            );
            if let Some(reminder) = reminder {
                app.hidden_queued_system_messages.insert(0, reminder);
            }
            if !combined.is_empty() {
                app.queued_messages.insert(0, combined);
                app.queued_images = images;
            }
        }
    } else if !app.hidden_queued_system_messages.is_empty() || app.has_pending_cloud_recovery() {
        let reminders = std::mem::take(&mut app.hidden_queued_system_messages);
        app.reload_recovery_dispatch_pending = false;
        let combined = reminders.join("\n\n");
        if begin_remote_send(
            app,
            remote,
            String::new(),
            vec![],
            true,
            Some(combined.clone()),
            true,
            0,
        )
        .await
        .is_err()
        {
            crate::logging::error(
                "Failed to send hidden continuation reminder; restoring it to the queue",
            );
            if !combined.is_empty() {
                app.hidden_queued_system_messages.insert(0, combined);
            }
        }
    }
}
