use super::*;

pub(super) fn handle_tool_done(
    app: &mut App,
    remote: &mut impl RemoteEventState,
    id: String,
    name: String,
    output: String,
    error: Option<String>,
) -> bool {
    // Read the raw output before client-side diff rendering replaces it.
    let observed_duration = remote.take_tool_duration(&id);
    let duration_secs = DisplayMessage::tool_duration_from_output(&output).or(observed_duration);
    let display_output = remote.handle_tool_done(&id, &name, &output);
    let display_output = if error.is_some()
        && !display_output.starts_with("Error:")
        && !display_output.starts_with("error:")
        && !display_output.starts_with("Failed:")
    {
        format!("Error: {}", display_output)
    } else {
        display_output
    };
    let existing_tool_call = app
        .streaming_tool_calls
        .iter()
        .find(|tc| tc.id == id)
        .cloned();
    let tool_call = existing_tool_call.unwrap_or_else(|| ToolCall {
        id: id.clone(),
        name: name.clone(),
        input: serde_json::Value::Null,
        intent: None,
        thought_signature: None,
    });
    app.commit_pending_streaming_assistant_message();
    crate::tui::mermaid::clear_streaming_preview_diagram();
    let is_batch = tool_call.name == "batch";
    app.observe_tool_result(&tool_call, &output, error.is_some(), None);
    app.note_tool_completed(&tool_call, error.is_some());
    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: display_output,
        tool_calls: vec![],
        duration_secs,
        title: None,
        tool_data: Some(tool_call.clone()),
    });
    app.note_todo_gate_result(&tool_call, &output, error.is_some());
    if is_batch {
        app.batch_progress = None;
    }
    // Only remove the completed call. When the model emits several tool calls
    // in one assistant message, siblings that already streamed their parsed
    // input/intent are still waiting for their own ToolDone; clearing the
    // whole list here made their rows render with no intent or summary.
    app.streaming_tool_calls.retain(|tc| tc.id != id);
    app.status = ProcessingStatus::Streaming;
    true
}

pub(super) fn handle_generated_image(
    app: &mut App,
    id: String,
    path: String,
    metadata_path: Option<String>,
    output_format: String,
    revised_prompt: Option<String>,
) -> bool {
    app.pause_streaming_tps(false);
    app.commit_pending_streaming_assistant_message();
    let input = crate::message::generated_image_tool_input(
        &path,
        metadata_path.as_deref(),
        &output_format,
        revised_prompt.as_deref(),
    );
    let tool_call = ToolCall {
        id: id.clone(),
        name: crate::message::GENERATED_IMAGE_TOOL_NAME.to_string(),
        input,
        intent: Some("OpenAI native image generation".to_string()),
        thought_signature: None,
    };
    let summary = crate::message::generated_image_summary(
        &path,
        metadata_path.as_deref(),
        &output_format,
        revised_prompt.as_deref(),
    );
    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: summary,
        tool_calls: vec![],
        duration_secs: None,
        title: Some("Generated image".to_string()),
        tool_data: Some(tool_call),
    });
    app.status = ProcessingStatus::Streaming;
    true
}

pub(in super::super) fn handle_disconnect(
    app: &mut App,
    state: &mut RemoteRunState,
    reason: Option<RemoteDisconnectReason>,
) {
    app.invalidate_rewind_ui();
    // The daemon may have applied the switch, but the acknowledgement was
    // lost. Never wait on an old connection's request ID or blindly resend.
    if app.pending_automatic_failover_request_id.is_some() {
        fail_pending_automatic_failover(app, "Connection lost before route confirmation");
    }
    app.revoke_cloud_environment();
    let detail = if state.server_reload_in_progress {
        "server reload in progress".to_string()
    } else if let Some(reason) = reason.as_ref() {
        format_disconnect_reason(reason)
    } else {
        "connection to server dropped".to_string()
    };
    crate::logging::warn(&format!(
        "handle_disconnect: session={:?}, remote_session_id={:?}, reason={:?}, detail={}",
        app.resume_session_id, app.remote_session_id, reason, detail
    ));
    // A disconnect can drop the final auth catalog completion event. Keep any
    // queued prompt for reconnect, but release the transient model-setup gate so
    // it cannot remain blocked forever waiting for an event that was lost.
    app.finish_auth_catalog_refresh();
    // The cancel answer cannot arrive on a dead connection. Ask again later.
    app.soft_interrupt_recovery = None;
    state.last_disconnect_reason = Some(detail.clone());

    let scheduled_retry =
        app.schedule_pending_remote_retry(&format!("⚡ Connection lost ({detail})."));
    if !scheduled_retry {
        // A queued follow-up that was already dispatched (dequeued into an
        // in-flight send) has no auto-retry path. Dropping it here would lose
        // the user's queued message when a reload/disconnect races the
        // turn-end dispatch (issue #391); put it back on the queue instead so
        // it is re-sent once the turn is proven idle after reconnect.
        if !recover_undelivered_queued_continuation(app, "disconnect") {
            app.clear_pending_remote_retry();
        }
    }
    let recovered_local = recover_local_interleave_to_queue(app, "disconnect");
    app.current_message_id = None;
    app.last_stream_activity = None;
    app.remote_resume_activity = None;
    let ops = app.stream_buffer.flush();
    app.apply_stream_ops(ops);
    if !app.streaming.streaming_text.is_empty() {
        let content = app.take_streaming_text();
        let content = app.collapse_reasoning_for_commit(content);
        if !content.trim().is_empty() {
            app.push_display_message(DisplayMessage {
                role: "assistant".to_string(),
                content,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
        }
    }
    app.clear_streaming_render_state();
    app.streaming_tool_calls.clear();
    app.batch_progress = None;
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    if recovered_local || !app.pending_soft_interrupts.is_empty() {
        crate::logging::info(&format!(
            "Preserving {} pending soft interrupt(s) across disconnect",
            app.pending_soft_interrupts.len()
        ));
    }
    app.reset_streaming_tps();
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.stream_message_ended = false;
    app.clear_visible_turn_started();
    state.disconnect_start = Some(Instant::now());
    state.reconnect_attempts = state.reconnect_attempts.max(1);
    state.reload_recovery_attempted = false;
    app.push_display_message(DisplayMessage {
        role: "system".to_string(),
        content: reconnect_status_message(app, state, &detail),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some(CONNECTION_MESSAGE_TITLE.to_string()),
        tool_data: None,
    });
    state.disconnect_msg_idx = Some(app.display_messages.len() - 1);
    state.reconnect_attempts = 1;
}
