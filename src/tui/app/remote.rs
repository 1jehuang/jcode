#![cfg_attr(test, allow(clippy::items_after_test_module))]

use super::{
    App, DisplayMessage, PendingReloadReconnectStatus, ProcessingStatus, RemoteResumeActivity,
    SendAction, ctrl_bracket_fallback_to_esc, input, parse_rate_limit_error,
    remote_notifications::present_swarm_notification, spawn_in_new_terminal,
};
use crate::bus::BusEvent;
use crate::message::ToolCall;
use crate::protocol::{ServerEvent, TranscriptMode};
use crate::tui::backend::{RemoteConnection, RemoteDisconnectReason, RemoteEventState, RemoteRead};
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};
use ratatui::{DefaultTerminal, Terminal, backend::Backend};
use std::time::{Duration, Instant};

mod input_dispatch;
mod key_handling;
mod queue_recovery;
mod reconnect;
mod server_event_handlers;
mod server_events;
mod session_persistence;
mod swarm_plan_core;
mod workspace;

use queue_recovery::{recover_local_interleave_to_queue, recover_stranded_soft_interrupts};
// Re-export for sibling modules and tests that access reconnect state and helpers
// through `super::remote::*` without reaching into private submodules directly.
// `#[allow(unused_imports)]` is required because rustc flags re-exports as unused
// when none of the names are referenced within *this* module; they are consumed by
// sibling modules and the compiler does not look across module boundaries here.
#[allow(unused_imports)]
pub(super) use reconnect::{
    ConnectOutcome, PostConnectOutcome, ReloadReconnectHints, RemoteRunState, connect_with_retry,
    finalize_reload_reconnect, handle_post_connect, reload_handoff_active,
    should_allow_reconnect_takeover, should_use_same_session_fast_path,
};
use reconnect::{format_disconnect_reason, reconnect_status_message};
use session_persistence::{
    persist_remote_session_metadata, persist_replay_display_message, persist_swarm_plan_snapshot,
    persist_swarm_status_snapshot,
};
use workspace::{handle_workspace_command, handle_workspace_navigation_key};

// Re-export the remote input dispatch helpers for sibling modules/tests that go
// through the `remote` facade instead of private submodule paths.
// Same rationale as above: `#[allow(unused_imports)]` silences the false-positive
// lint for re-exports that are only consumed outside this module.
#[allow(unused_imports)]
pub(super) use input_dispatch::{
    apply_remote_transcript_event, apply_transcript_event, begin_remote_send,
    begin_remote_split_launch, finish_remote_split_launch, history_matches_pending_startup_prompt,
    route_prepared_input_to_new_remote_session, submit_prepared_remote_input,
};
pub(super) use key_handling::{
    handle_remote_char_input, handle_remote_key, handle_remote_key_event, send_interleave_now,
};
pub(super) use server_events::handle_server_event;

const CONNECTION_MESSAGE_TITLE: &str = "Connection";
const RELOAD_MARKER_MAX_AGE: Duration = Duration::from_secs(30);
pub(super) enum RemoteEventOutcome {
    Continue,
    Reconnect,
}

pub(super) async fn handle_tick(app: &mut App, remote: &mut RemoteConnection) -> bool {
    let mut needs_redraw = crate::tui::periodic_redraw_required(app);
    app.maybe_capture_runtime_memory_heartbeat();
    app.progress_mouse_scroll_animation();
    needs_redraw |= dispatch_compacted_history_load(app, remote).await;
    if let Some(chunk) = app.stream_buffer.flush() {
        app.append_streaming_text(&chunk);
        needs_redraw = true;
    }

    needs_redraw |= app.refresh_todos_view_if_needed();
    needs_redraw |= app.refresh_side_panel_linked_content_if_due();
    needs_redraw |= app.poll_model_picker_load();
    needs_redraw |= app.poll_session_picker_load();

    let _ = check_debug_command(app, remote).await;

    if !app.is_processing {
        if let Some(request) = app.take_pending_catchup_resume() {
            match remote.resume_session(&request.target_session_id).await {
                Ok(()) => {
                    let label = crate::id::extract_session_name(&request.target_session_id)
                        .map(|name| name.to_string())
                        .unwrap_or_else(|| request.target_session_id.clone());
                    let show_brief = request.show_brief;
                    app.begin_in_flight_catchup_resume(request);
                    app.set_status_notice(if show_brief {
                        format!("Catch Up → {}", label)
                    } else {
                        format!("Back → {}", label)
                    });
                    return true;
                }
                Err(err) => {
                    app.clear_in_flight_catchup_resume();
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to switch Catch Up session: {}",
                        err
                    )));
                    needs_redraw = true;
                }
            }
        }

        if let Some(target_session) = crate::tui::workspace_client::take_pending_resume_session() {
            match remote.resume_session(&target_session).await {
                Ok(()) => {
                    let label = crate::id::extract_session_name(&target_session)
                        .map(|name| name.to_string())
                        .unwrap_or(target_session);
                    app.set_status_notice(format!("Workspace → {}", label));
                    return true;
                }
                Err(err) => {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to switch workspace session: {}",
                        err
                    )));
                    needs_redraw = true;
                }
            }
        }
    }

    if let Some(reset_time) = app.rate_limit_reset
        && Instant::now() >= reset_time
    {
        app.rate_limit_reset = None;
        if !app.is_processing
            && let Some(pending) = app.rate_limit_pending_message.clone()
        {
            let status = if pending.auto_retry {
                format!(
                    "✓ Retrying continuation...{}",
                    if pending.is_system {
                        " (system message)"
                    } else {
                        ""
                    }
                )
            } else {
                format!(
                    "✓ Rate limit reset. Retrying...{}",
                    if pending.is_system {
                        " (system message)"
                    } else {
                        ""
                    }
                )
            };
            app.push_display_message(DisplayMessage::system(status));
            let _ = begin_remote_send(
                app,
                remote,
                pending.content,
                pending.images,
                pending.is_system,
                pending.system_reminder,
                pending.auto_retry,
                pending.retry_attempts,
            )
            .await;
            return true;
        }
    }

    if app.pending_queued_dispatch {
        return needs_redraw;
    }

    if !app.is_processing && !app.queued_messages.is_empty() {
        let queued_messages = std::mem::take(&mut app.queued_messages);
        let hidden_reminders = std::mem::take(&mut app.hidden_queued_system_messages);
        let (messages, reminder, display_system_messages) =
            super::helpers::partition_queued_messages(queued_messages, hidden_reminders);
        let combined = messages.join("\n\n");
        let auto_retry = reminder.is_some() && messages.is_empty();
        crate::logging::info(&format!(
            "Sending queued continuation message ({} chars)",
            combined.len()
        ));
        for msg in display_system_messages {
            app.push_display_message(DisplayMessage::system(msg));
        }
        for msg in &messages {
            app.push_display_message(DisplayMessage::user(msg.clone()));
        }
        if begin_remote_send(app, remote, combined, vec![], true, reminder, auto_retry, 0)
            .await
            .is_err()
        {
            crate::logging::error("Failed to send queued continuation message");
        }
        needs_redraw = true;
    }

    if !app.is_processing && !app.hidden_queued_system_messages.is_empty() {
        let reminders = std::mem::take(&mut app.hidden_queued_system_messages);
        let combined = reminders.join("\n\n");
        crate::logging::info(&format!(
            "Sending hidden continuation reminder ({} chars)",
            combined.len()
        ));
        if begin_remote_send(
            app,
            remote,
            String::new(),
            vec![],
            true,
            Some(combined),
            true,
            0,
        )
        .await
        .is_err()
        {
            crate::logging::error("Failed to send hidden continuation reminder");
        }
        needs_redraw = true;
    }

    detect_and_cancel_stall(app, remote).await;
    needs_redraw
}

pub(super) async fn handle_terminal_event(
    app: &mut App,
    _terminal: &mut DefaultTerminal,
    remote: &mut RemoteConnection,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<bool> {
    let mut needs_redraw = false;
    match event {
        Some(Ok(Event::FocusGained)) => {
            app.note_client_focus(true);
        }
        Some(Ok(Event::Key(key))) => {
            app.note_client_interaction();
            app.update_copy_badge_key_event(key);
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                handle_remote_key_event(app, key, remote).await?;
                if let Some(spec) = app.pending_model_switch.take() {
                    let _ = remote.set_model(&spec).await;
                }
                if let Some(selection) = app.pending_account_picker_action.take() {
                    match selection {
                        crate::tui::AccountPickerAction::Switch { provider_id, label } => {
                            match provider_id.as_str() {
                                "claude" => {
                                    if let Err(e) = crate::auth::claude::set_active_account(&label)
                                    {
                                        app.push_display_message(DisplayMessage::error(format!(
                                            "Failed to switch account: {}",
                                            e
                                        )));
                                    } else {
                                        crate::auth::AuthStatus::invalidate_cache();
                                        app.context_limit = app.provider.context_window() as u64;
                                        app.context_warning_shown = false;
                                        let _ = remote.switch_anthropic_account(&label).await;
                                        app.push_display_message(DisplayMessage::system(format!(
                                            "Switched to Anthropic account `{}`.",
                                            label
                                        )));
                                        app.set_status_notice(format!(
                                            "Account: switched to {}",
                                            label
                                        ));
                                    }
                                }
                                "openai" => {
                                    if let Err(e) = crate::auth::codex::set_active_account(&label) {
                                        app.push_display_message(DisplayMessage::error(format!(
                                            "Failed to switch OpenAI account: {}",
                                            e
                                        )));
                                    } else {
                                        crate::auth::AuthStatus::invalidate_cache();
                                        app.context_limit = app.provider.context_window() as u64;
                                        app.context_warning_shown = false;
                                        let _ = remote.switch_openai_account(&label).await;
                                        app.push_display_message(DisplayMessage::system(format!(
                                            "Switched to OpenAI account `{}`.",
                                            label
                                        )));
                                        app.set_status_notice(format!(
                                            "OpenAI account: switched to {}",
                                            label
                                        ));
                                    }
                                }
                                _ => app.push_display_message(DisplayMessage::error(format!(
                                    "Provider `{}` does not support account switching.",
                                    provider_id
                                ))),
                            }
                        }
                        crate::tui::AccountPickerAction::Add { .. }
                        | crate::tui::AccountPickerAction::Replace { .. }
                        | crate::tui::AccountPickerAction::OpenCenter { .. } => {}
                    }
                }
            }
            needs_redraw = true;
            needs_redraw |= dispatch_compacted_history_load(app, remote).await;
        }
        Some(Ok(Event::Paste(text))) => {
            app.note_client_interaction();
            app.handle_paste(text);
            needs_redraw = true;
        }
        Some(Ok(Event::Mouse(mouse))) => {
            app.note_client_interaction();
            handle_mouse_event(app, mouse);
            needs_redraw = true;
            needs_redraw |= dispatch_compacted_history_load(app, remote).await;
        }
        Some(Ok(Event::Resize(_, _))) => {
            needs_redraw = app.should_redraw_after_resize();
        }
        _ => {}
    }
    Ok(needs_redraw)
}

async fn dispatch_compacted_history_load(app: &mut App, remote: &mut RemoteConnection) -> bool {
    let Some(visible_messages) = app.take_pending_compacted_history_load() else {
        return false;
    };
    match remote.get_compacted_history(visible_messages).await {
        Ok(_) => true,
        Err(error) => {
            app.restore_pending_compacted_history_load(visible_messages);
            app.set_status_notice(format!("Failed to request older history: {}", error));
            true
        }
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;

pub(super) async fn handle_bus_event(
    app: &mut App,
    remote: &mut RemoteConnection,
    bus_event: std::result::Result<BusEvent, tokio::sync::broadcast::error::RecvError>,
) {
    match bus_event {
        Ok(BusEvent::UsageReport(results)) => {
            app.handle_usage_report(results);
        }
        Ok(BusEvent::ClipboardPasteCompleted(result)) => {
            app.handle_clipboard_paste_completed(result);
        }
        Ok(BusEvent::ModelRefreshCompleted(result)) => {
            app.handle_model_refresh_completed(result);
        }
        Ok(BusEvent::GitStatusCompleted(result)) => {
            super::commands::handle_git_status_completed(app, result);
        }
        Ok(BusEvent::MermaidRenderCompleted) => {}
        Ok(BusEvent::UsageReportProgress(progress)) => {
            app.handle_usage_report_progress(progress);
        }
        Ok(BusEvent::LoginCompleted(login)) => {
            let success = login.success && login.provider != "copilot_code";
            app.handle_login_completed(login);
            if success {
                remote.notify_auth_changed_detached();
            }
        }
        Ok(BusEvent::UpdateStatus(status)) => {
            app.handle_update_status(status);
        }
        Ok(BusEvent::SessionUpdateStatus(status)) => {
            app.handle_session_update_status(status);
        }
        Ok(BusEvent::DictationCompleted {
            dictation_id,
            session_id,
            text,
            mode,
        }) => {
            if !app.owns_dictation_event(&dictation_id, session_id.as_deref()) {
                return;
            }
            match remote.send_transcript(text, mode).await {
                Ok(()) => app.mark_dictation_delivered(),
                Err(error) => app.handle_dictation_failure(error.to_string()),
            }
        }
        Ok(BusEvent::DictationFailed {
            dictation_id,
            session_id,
            message,
        }) => {
            if !app.owns_dictation_event(&dictation_id, session_id.as_deref()) {
                return;
            }
            app.handle_dictation_failure(message);
        }
        _ => {}
    }
}

pub(super) async fn check_debug_command(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Option<String> {
    let cmd_path = super::debug_cmd_path();
    if let Ok(cmd) = std::fs::read_to_string(&cmd_path) {
        let _ = std::fs::remove_file(&cmd_path);
        let cmd = cmd.trim();

        app.debug_trace.record("cmd", cmd.to_string());

        let response = handle_debug_command(app, cmd, remote).await;
        let _ = std::fs::write(super::debug_response_path(), &response);
        return Some(response);
    }
    None
}

fn handle_terminal_event_while_disconnected(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<bool> {
    let mut needs_redraw = false;

    match event {
        Some(Ok(Event::FocusGained)) => {
            app.note_client_focus(true);
        }
        Some(Ok(Event::Key(key))) => {
            app.note_client_interaction();
            app.update_copy_badge_key_event(key);
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                handle_disconnected_key_event(app, key)?;
            }
            needs_redraw = true;
        }
        Some(Ok(Event::Paste(text))) => {
            app.note_client_interaction();
            app.handle_paste(text);
            needs_redraw = true;
        }
        Some(Ok(Event::Mouse(mouse))) => {
            app.note_client_interaction();
            handle_mouse_event(app, mouse);
            needs_redraw = true;
        }
        Some(Ok(Event::Resize(_, _))) => {
            needs_redraw = app.should_redraw_after_resize();
        }
        _ => {}
    }

    if needs_redraw {
        terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
    }

    Ok(app.should_quit)
}

pub(super) async fn handle_remote_event<B: Backend>(
    app: &mut App,
    _terminal: &mut Terminal<B>,
    remote: &mut RemoteConnection,
    state: &mut RemoteRunState,
    event: RemoteRead,
) -> Result<(RemoteEventOutcome, bool)> {
    match event {
        RemoteRead::Disconnected(reason) => {
            handle_disconnect(app, state, Some(reason));
            Ok((RemoteEventOutcome::Reconnect, true))
        }
        RemoteRead::Event(ServerEvent::Reloading { new_socket }) => {
            let _ = new_socket;
            state.server_reload_in_progress = true;
            state.reload_recovery_attempted = false;
            state.last_disconnect_reason = Some("server reload in progress".to_string());
            let needs_redraw =
                handle_server_event(app, ServerEvent::Reloading { new_socket: None }, remote);
            process_remote_followups(app, remote).await;
            Ok((RemoteEventOutcome::Continue, needs_redraw))
        }
        RemoteRead::Event(ServerEvent::ClientDebugRequest { id, command }) => {
            let output = handle_debug_command(app, &command, remote).await;
            let _ = remote.send_client_debug_response(id, output).await;
            process_remote_followups(app, remote).await;
            Ok((RemoteEventOutcome::Continue, false))
        }
        RemoteRead::Event(ServerEvent::Transcript { text, mode }) => {
            let mut needs_redraw = false;
            if let Err(error) = apply_remote_transcript_event(app, remote, text, mode).await {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to apply transcript: {}",
                    error
                )));
                app.set_status_notice("Transcript failed");
                needs_redraw = true;
            }
            process_remote_followups(app, remote).await;
            Ok((RemoteEventOutcome::Continue, needs_redraw))
        }
        RemoteRead::Event(server_event) => {
            let needs_redraw = handle_server_event(app, server_event, remote);
            process_remote_followups(app, remote).await;
            Ok((RemoteEventOutcome::Continue, needs_redraw))
        }
    }
}

pub(super) fn handle_disconnect(
    app: &mut App,
    state: &mut RemoteRunState,
    reason: Option<RemoteDisconnectReason>,
) {
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
    state.last_disconnect_reason = Some(detail.clone());

    let scheduled_retry =
        app.schedule_pending_remote_retry(&format!("⚡ Connection lost ({detail})."));
    if !scheduled_retry {
        app.clear_pending_remote_retry();
    }
    let recovered_local = recover_local_interleave_to_queue(app, "disconnect");
    app.current_message_id = None;
    app.last_stream_activity = None;
    app.remote_resume_activity = None;
    if let Some(chunk) = app.stream_buffer.flush() {
        app.append_streaming_text(&chunk);
    }
    if !app.streaming_text.is_empty() {
        let content = app.take_streaming_text();
        app.push_display_message(DisplayMessage {
            role: "assistant".to_string(),
            content,
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
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

pub(super) async fn process_remote_followups(app: &mut App, remote: &mut RemoteConnection) {
    if !remote.has_loaded_history() {
        return;
    }

    let _ = recover_stranded_soft_interrupts(app, remote).await;

    if app.pending_queued_dispatch {
        return;
    }

    let synthetic_startup_dispatch = app.is_processing
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
        if !app.input.is_empty() || !app.pending_images.is_empty() {
            let prepared = input::take_prepared_input(app);
            if let Err(error) = submit_prepared_remote_input(app, remote, prepared).await {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to submit startup prompt: {}",
                    error
                )));
                app.set_status_notice("Startup prompt failed");
            }
            return;
        }
    }

    if app.pending_background_client_reload.is_some() && !app.is_processing {
        app.maybe_finish_background_client_reload();
        return;
    }

    if app.pending_server_reload && !app.is_processing {
        app.pending_server_reload = false;
        if app.auto_server_reload {
            app.append_reload_message("Reloading server with newer binary...");
            if let Err(err) = remote.reload().await {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to auto-reload server: {}. Use `/reload` to retry.",
                    err
                )));
                app.set_status_notice("Server update available — auto reload failed");
            }
        } else {
            app.push_display_message(DisplayMessage::system(
                "ℹ Newer server binary detected. Auto-reload is disabled by `display.auto_server_reload = false`. Use `/reload` manually when you're ready.".to_string(),
            ));
            app.set_status_notice("Server update available — manual /reload recommended");
        }
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
            let msg_clone = interleave_msg.clone();
            match remote.soft_interrupt(interleave_msg, false).await {
                Err(e) => {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to queue soft interrupt: {}",
                        e
                    )));
                }
                Ok(request_id) => {
                    app.track_pending_soft_interrupt(request_id, msg_clone);
                }
            }
        }
        return;
    }

    if let Some(interleave_msg) = app.interleave_message.take() {
        if !interleave_msg.trim().is_empty() {
            app.push_display_message(DisplayMessage {
                role: "user".to_string(),
                content: interleave_msg.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            if let Err(e) =
                begin_remote_send(app, remote, interleave_msg, vec![], false, None, false, 0).await
            {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to send message: {}",
                    e
                )));
            }
        }
    } else if !app.queued_messages.is_empty() {
        let queued_messages = std::mem::take(&mut app.queued_messages);
        let hidden_reminders = std::mem::take(&mut app.hidden_queued_system_messages);
        let (messages, reminder, display_system_messages) =
            super::helpers::partition_queued_messages(queued_messages, hidden_reminders);
        let combined = messages.join("\n\n");
        let preserve_visible_turn = super::commands::queued_messages_are_only_pokes(&messages);
        let auto_retry = reminder.is_some() && messages.is_empty();
        for msg in display_system_messages {
            app.push_display_message(DisplayMessage::system(msg));
        }
        for msg in &messages {
            if !super::commands::is_poke_message(msg) {
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
        let _ =
            begin_remote_send(app, remote, combined, vec![], true, reminder, auto_retry, 0).await;
    } else if !app.hidden_queued_system_messages.is_empty() {
        let reminders = std::mem::take(&mut app.hidden_queued_system_messages);
        let combined = reminders.join("\n\n");
        let _ = begin_remote_send(
            app,
            remote,
            String::new(),
            vec![],
            true,
            Some(combined),
            true,
            0,
        )
        .await;
    }
}

async fn detect_and_cancel_stall(app: &mut App, remote: &mut RemoteConnection) {
    const STALL_TIMEOUT: Duration = Duration::from_secs(2 * 60);
    let is_running_tool = matches!(app.status, ProcessingStatus::RunningTool(_));
    if app.is_processing && !is_running_tool {
        let stalled = app
            .last_stream_activity
            .map(|t| t.elapsed() > STALL_TIMEOUT)
            .unwrap_or_else(|| {
                app.processing_started
                    .map(|t| t.elapsed() > STALL_TIMEOUT)
                    .unwrap_or(false)
            });
        if stalled {
            if let Some(snapshot) = app.remote_resume_activity.clone() {
                let elapsed = app
                    .last_stream_activity
                    .map(|t| t.elapsed())
                    .or(app.processing_started.map(|t| t.elapsed()));
                crate::logging::warn(&format!(
                    "Protocol stall guard: resumed session {} is still marked processing by history snapshot (tool={:?}, snapshot_age={:?}) but no corroborating live events arrived after {:?}; deferring client-side cancel",
                    snapshot.session_id,
                    snapshot.current_tool_name,
                    snapshot.observed_at.elapsed(),
                    elapsed
                ));
                app.last_stream_activity = Some(Instant::now());
                app.status = match snapshot.current_tool_name {
                    Some(tool_name) => ProcessingStatus::RunningTool(tool_name),
                    None => ProcessingStatus::Thinking(Instant::now()),
                };
                return;
            }
            crate::logging::warn(&format!(
                "Stream stall detected: no server events for {:?}, cancelling",
                app.last_stream_activity
                    .map(|t| t.elapsed())
                    .or(app.processing_started.map(|t| t.elapsed()))
            ));
            let _ = remote.cancel().await;
            app.is_processing = false;
            app.clear_visible_turn_started();
            app.status = ProcessingStatus::Idle;
            app.current_message_id = None;
            app.processing_started = None;
            app.last_stream_activity = None;
            if !app.streaming_text.is_empty() {
                let content = app.take_streaming_text();
                app.push_display_message(DisplayMessage {
                    role: "assistant".to_string(),
                    content,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            if !app.schedule_pending_remote_retry(
                "⚠ Stream stalled (no response for 2 minutes). Processing cancelled.",
            ) {
                app.clear_pending_remote_retry();
                app.push_display_message(DisplayMessage::system(
                    "⚠ Stream stalled (no response for 2 minutes). Processing cancelled. You can resend your message.".to_string(),
                ));
            }
        }
    }
}

fn handle_mouse_event(app: &mut App, mouse: MouseEvent) {
    app.handle_mouse_event(mouse);
}

async fn handle_debug_command(app: &mut App, cmd: &str, remote: &mut RemoteConnection) -> String {
    let cmd = cmd.trim();
    if cmd.starts_with("message:") {
        let msg = cmd.strip_prefix("message:").unwrap_or("");
        app.input = msg.to_string();
        let result = handle_remote_key(app, KeyCode::Enter, KeyModifiers::empty(), remote).await;
        if let Err(e) = result {
            return format!("ERR: {}", e);
        }
        app.debug_trace
            .record("message", format!("submitted:{}", msg));
        return format!("OK: queued message '{}'", msg);
    }
    if cmd == "reload" {
        app.input = "/reload".to_string();
        let result = handle_remote_key(app, KeyCode::Enter, KeyModifiers::empty(), remote).await;
        if let Err(e) = result {
            return format!("ERR: {}", e);
        }
        app.debug_trace.record("reload", "triggered".to_string());
        return "OK: reload triggered".to_string();
    }
    if cmd == "state" {
        return serde_json::json!({
            "processing": app.is_processing,
            "messages": app.messages.len(),
            "display_messages": app.display_messages.len(),
            "input": app.input,
            "cursor_pos": app.cursor_pos,
            "scroll_offset": app.scroll_offset,
            "queued_messages": app.queued_messages.len(),
            "provider_session_id": app.provider_session_id,
            "provider_name": app.remote_provider_name.clone(),
            "model": app.remote_provider_model.as_deref().unwrap_or(app.provider.name()),
            "connection_type": app.connection_type.clone(),
            "remote_transport": app.remote_transport.clone(),
            "diagram_mode": format!("{:?}", app.diagram_mode),
            "diagram_focus": app.diagram_focus,
            "diagram_index": app.diagram_index,
            "diagram_scroll": [app.diagram_scroll_x, app.diagram_scroll_y],
            "diagram_pane_ratio": app.diagram_pane_ratio_target,
            "diagram_pane_enabled": app.diagram_pane_enabled,
            "diagram_pane_position": format!("{:?}", app.diagram_pane_position),
            "diagram_zoom": app.diagram_zoom,
            "diagram_count": crate::tui::mermaid::get_active_diagrams().len(),
            "remote": true,
            "server_version": app.remote_server_version.clone(),
            "server_has_update": app.remote_server_has_update,
            "version": env!("JCODE_VERSION"),
            "diagram_mode": format!("{:?}", app.diagram_mode),
        })
        .to_string();
    }
    if cmd.starts_with("keys:") {
        let keys_str = cmd.strip_prefix("keys:").unwrap_or("");
        let mut results = Vec::new();
        for key_spec in keys_str.split(',') {
            match parse_and_inject_key(app, key_spec.trim(), remote).await {
                Ok(desc) => {
                    app.debug_trace.record("key", desc.clone());
                    results.push(format!("OK: {}", desc));
                }
                Err(e) => results.push(format!("ERR: {}", e)),
            }
        }
        return results.join("\n");
    }
    if cmd == "submit" {
        if app.input.is_empty() {
            return "submit error: input is empty".to_string();
        }
        let result = handle_remote_key(app, KeyCode::Enter, KeyModifiers::empty(), remote).await;
        if let Err(e) = result {
            return format!("ERR: {}", e);
        }
        app.debug_trace.record("input", "submitted".to_string());
        return "OK: submitted".to_string();
    }
    if cmd.starts_with("run:") || cmd.starts_with("script:") {
        return "ERR: script/run not supported in remote debug mode".to_string();
    }
    app.handle_debug_command(cmd)
}

async fn parse_and_inject_key(
    app: &mut App,
    key_spec: &str,
    remote: &mut RemoteConnection,
) -> std::result::Result<String, String> {
    let (key_code, modifiers) = app.parse_key_spec(key_spec)?;
    handle_remote_key(app, key_code, modifiers, remote)
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!("injected {:?} with {:?}", key_code, modifiers))
}

fn handle_disconnected_local_command(app: &mut App, trimmed: &str) -> bool {
    let handled = super::commands::handle_help_command(app, trimmed)
        || super::commands::handle_session_command(app, trimmed)
        || super::commands::handle_goals_command(app, trimmed)
        || super::commands::handle_config_command(app, trimmed)
        || super::commands::handle_debug_command(app, trimmed)
        || super::commands::handle_model_command(app, trimmed)
        || super::commands::handle_usage_command(app, trimmed)
        || super::commands::handle_feedback_command(app, trimmed)
        || super::state_ui::handle_info_command(app, trimmed)
        || super::auth::handle_auth_command(app, trimmed)
        || super::commands::handle_dev_command(app, trimmed);

    if handled {
        if trimmed.starts_with('/') {
            crate::telemetry::record_command_family(trimmed);
        }
        app.input.clear();
        app.cursor_pos = 0;
        app.reset_tab_completion();
        app.sync_model_picker_preview_from_input();
        app.clear_input_undo_history();
    }

    handled
}

fn queue_message_for_reconnect(app: &mut App) {
    let trimmed = app.input.trim().to_string();
    if trimmed.is_empty() {
        return;
    }

    if trimmed.starts_with('/') {
        if handle_disconnected_local_command(app, &trimmed) {
            return;
        }
        app.set_status_notice("This command requires a live connection");
        return;
    }

    let prepared = input::take_prepared_input(app);
    app.queued_messages.push(prepared.expanded);

    let queued_count = app.queued_messages.len();
    app.set_status_notice(format!(
        "Queued for send after reconnect ({} message{})",
        queued_count,
        if queued_count == 1 { "" } else { "s" }
    ));
}

#[cfg(test)]
pub(super) fn handle_disconnected_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<()> {
    handle_disconnected_key_internal(app, code, modifiers, None)
}

pub(super) fn handle_disconnected_key_event(app: &mut App, event: KeyEvent) -> Result<()> {
    handle_disconnected_key_internal(
        app,
        event.code,
        event.modifiers,
        input::text_input_for_key_event(&event),
    )
}

fn handle_disconnected_key_internal(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    text_input: Option<String>,
) -> Result<()> {
    let mut code = code;
    let mut modifiers = modifiers;
    ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

    if input::handle_navigation_shortcuts(app, code, modifiers) {
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') | KeyCode::Char('d') => {
                app.handle_quit_request();
                return Ok(());
            }
            KeyCode::Char('l') if !app.diff_pane_visible() => {
                app.clear_display_messages();
                app.queued_messages.clear();
                return Ok(());
            }
            _ => {
                if input::handle_control_key(app, code) {
                    return Ok(());
                }
            }
        }
    }

    if modifiers.contains(KeyModifiers::ALT) && input::handle_alt_key(app, code) {
        return Ok(());
    }

    if code == KeyCode::Enter && modifiers.contains(KeyModifiers::CONTROL) {
        queue_message_for_reconnect(app);
        return Ok(());
    }

    if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
        input::insert_input_text(app, "\n");
        return Ok(());
    }

    // Never fall through and insert literal text for unhandled Ctrl+key chords.
    if modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(());
    }

    if let Some(text) = text_input.or_else(|| input::text_input_for_key(code, modifiers)) {
        input::handle_text_input(app, &text);
        app.follow_chat_bottom_for_typing();
        return Ok(());
    }

    match code {
        KeyCode::Char(c) => handle_remote_char_input(app, c),
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                let prev = super::super::core::prev_char_boundary(&app.input, app.cursor_pos);
                app.remember_input_undo_state();
                app.input.drain(prev..app.cursor_pos);
                app.cursor_pos = prev;
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.input.len() {
                let next = super::super::core::next_char_boundary(&app.input, app.cursor_pos);
                app.remember_input_undo_state();
                app.input.drain(app.cursor_pos..next);
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = super::super::core::prev_char_boundary(&app.input, app.cursor_pos);
            }
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = super::super::core::next_char_boundary(&app.input, app.cursor_pos);
            }
        }
        KeyCode::Home => app.cursor_pos = 0,
        KeyCode::End => app.cursor_pos = app.input.len(),
        KeyCode::Tab => {
            app.autocomplete();
        }
        KeyCode::Enter => {
            queue_message_for_reconnect(app);
        }
        KeyCode::Up | KeyCode::PageUp => {
            let inc = if code == KeyCode::PageUp { 10 } else { 1 };
            app.scroll_up(inc);
        }
        KeyCode::Down | KeyCode::PageDown => {
            let dec = if code == KeyCode::PageDown { 10 } else { 1 };
            app.scroll_down(dec);
        }
        KeyCode::Esc => {
            app.follow_chat_bottom();
            input::clear_input_for_escape(app);
        }
        _ => {}
    }

    Ok(())
}
