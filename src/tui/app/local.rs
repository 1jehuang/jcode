use super::{App, DisplayMessage, ProcessingStatus, is_context_limit_error};
use crate::bus::{
    BackgroundTaskCompleted, BackgroundTaskProgressEvent, BusEvent, InputShellCompleted,
    ManualToolCompleted,
};
use crate::message::{
    ContentBlock, Message, Role, background_task_status_notice,
    format_background_task_notification_markdown, format_background_task_progress_markdown,
};
use crate::session::StoredDisplayRole;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use ratatui::DefaultTerminal;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

const BACKGROUND_PROGRESS_NOTICE_MIN_INTERVAL: Duration = Duration::from_millis(400);
const BACKGROUND_PROGRESS_IDENTICAL_NOTICE_TTL: Duration = Duration::from_secs(2);

pub(super) async fn process_turn_with_input(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event_stream: &mut EventStream,
    bus_receiver: &mut Receiver<BusEvent>,
) {
    match app
        .run_turn_interactive(terminal, event_stream, Some(bus_receiver))
        .await
    {
        Ok(()) => {
            app.last_stream_error = None;
        }
        Err(error) => {
            let err_str = crate::util::format_error_chain(&error);
            if is_context_limit_error(&err_str) {
                if !app.try_auto_compact_and_retry(terminal, event_stream).await {
                    app.handle_turn_error(err_str);
                }
            } else {
                app.handle_turn_error(err_str);
            }
        }
    }

    if app.pending_queued_dispatch {
        finish_turn(app);
        return;
    }

    app.process_queued_messages(terminal, event_stream).await;
    finish_turn(app);
}

pub(super) fn handle_tick(app: &mut App) -> bool {
    let mut needs_redraw = crate::tui::periodic_redraw_required(app);
    app.maybe_capture_runtime_memory_heartbeat();
    app.progress_mouse_scroll_animation();
    if app.submit_input_on_startup && !app.is_processing {
        app.submit_input_on_startup = false;
        app.submit_input();
        needs_redraw = true;
    }
    if let Some(chunk) = app.stream_buffer.flush() {
        app.append_streaming_text(&chunk);
        needs_redraw = true;
    }
    needs_redraw |= app.refresh_todos_view_if_needed();
    needs_redraw |= app.refresh_side_panel_linked_content_if_due();
    needs_redraw |= app.poll_model_picker_load();
    needs_redraw |= app.poll_session_picker_load();
    needs_redraw |= app.poll_compaction_completion();
    needs_redraw |= app.maybe_refresh_overnight_display_card();
    needs_redraw |= super::commands::poll_local_transfer_prepare(app);
    needs_redraw |= super::commands::maybe_begin_pending_local_transfer(app);
    needs_redraw |= app.maybe_progress_provider_failover_countdown();
    app.check_debug_command();
    needs_redraw |= app.check_stable_version();
    needs_redraw |= app.maybe_finish_background_client_reload();
    if app.pending_migration.is_some() && !app.is_processing {
        app.execute_migration();
        needs_redraw = true;
    }
    if let Some(reset_time) = app.rate_limit_reset
        && std::time::Instant::now() >= reset_time
    {
        app.rate_limit_reset = None;
        let queued_count = app.queued_messages.len();
        let msg = if queued_count > 0 {
            format!("✓ Rate limit reset. Retrying... (+{} queued)", queued_count)
        } else {
            "✓ Rate limit reset. Retrying...".to_string()
        };
        app.push_display_message(DisplayMessage::system(msg));
        app.pending_turn = true;
        needs_redraw = true;
    }

    needs_redraw
}

pub(super) fn handle_terminal_event(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<bool> {
    let mut needs_redraw = apply_terminal_event(app, terminal, event)?;
    const MAX_DRAINED_EVENTS_PER_WAKE: usize = 32;
    for _ in 0..MAX_DRAINED_EVENTS_PER_WAKE {
        if !crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
            break;
        }
        if let Ok(event) = crossterm::event::read() {
            needs_redraw |= apply_terminal_event(app, terminal, Some(Ok(event)))?;
        }
    }
    Ok(needs_redraw)
}

pub(super) fn handle_bus_event(
    app: &mut App,
    bus_event: std::result::Result<BusEvent, RecvError>,
) -> bool {
    match bus_event {
        Ok(BusEvent::BackgroundTaskCompleted(task)) => {
            handle_background_task_completed(app, task);
            true
        }
        Ok(BusEvent::BackgroundTaskProgress(progress)) => {
            handle_background_task_progress(app, progress);
            true
        }
        Ok(BusEvent::InputShellCompleted(shell)) => {
            handle_input_shell_completed(app, shell);
            true
        }
        Ok(BusEvent::ClipboardPasteCompleted(result)) => {
            app.handle_clipboard_paste_completed(result)
        }
        Ok(BusEvent::ModelRefreshCompleted(result)) => {
            app.handle_model_refresh_completed(result);
            true
        }
        Ok(BusEvent::GitStatusCompleted(result)) => {
            super::commands::handle_git_status_completed(app, result);
            true
        }
        Ok(BusEvent::MermaidRenderCompleted) => true,
        Ok(BusEvent::UsageReport(results)) => {
            app.handle_usage_report(results);
            true
        }
        Ok(BusEvent::UsageReportProgress(progress)) => {
            app.handle_usage_report_progress(progress);
            true
        }
        Ok(BusEvent::LoginCompleted(login)) => {
            app.handle_login_completed(login);
            true
        }
        Ok(BusEvent::ModelsUpdated) => {
            app.invalidate_model_picker_cache();
            true
        }
        Ok(BusEvent::ProviderModelActivated {
            session_id,
            model,
            message,
            open_picker,
        }) => {
            if session_id != app.session.id {
                return false;
            }
            app.provider_session_id = None;
            app.session.provider_session_id = None;
            app.upstream_provider = None;
            app.invalidate_model_picker_cache();
            app.update_context_limit_for_model(&model);
            app.session.model = Some(model.clone());
            if let Err(e) = app.session.save() {
                crate::logging::warn(&format!("Failed to save session: {}", e));
            }
            app.push_display_message(crate::tui::DisplayMessage::system(message));
            app.set_status_notice(format!("Model → {}", model));
            if open_picker {
                app.open_model_picker();
            }
            true
        }
        Ok(BusEvent::UpdateStatus(status)) => {
            app.handle_update_status(status);
            true
        }
        Ok(BusEvent::SessionUpdateStatus(status)) => {
            app.handle_session_update_status(status);
            true
        }
        Ok(BusEvent::DictationCompleted {
            dictation_id,
            session_id,
            text,
            mode,
        }) => {
            if !app.owns_dictation_event(&dictation_id, session_id.as_deref()) {
                return false;
            }
            app.handle_local_dictation_completed(text, mode);
            true
        }
        Ok(BusEvent::DictationFailed {
            dictation_id,
            session_id,
            message,
        }) => {
            if !app.owns_dictation_event(&dictation_id, session_id.as_deref()) {
                return false;
            }
            app.handle_dictation_failure(message);
            true
        }
        Ok(BusEvent::CompactionFinished) => app.poll_compaction_completion(),
        Ok(BusEvent::SidePanelUpdated(update)) => {
            if update.session_id == app.session.id {
                app.set_side_panel_snapshot(update.snapshot);
                true
            } else {
                false
            }
        }
        Ok(BusEvent::TodoUpdated(event)) => {
            if event.session_id == app.session.id {
                app.refresh_todos_view_now()
            } else {
                false
            }
        }
        Ok(BusEvent::ManualToolCompleted(result)) => {
            handle_manual_tool_completed(app, result);
            true
        }
        _ => false,
    }
}

fn handle_manual_tool_completed(app: &mut App, result: ManualToolCompleted) {
    if result.session_id != app.session.id {
        return;
    }

    let display_output = if result.is_error
        && !result.output.starts_with("Error:")
        && !result.output.starts_with("error:")
        && !result.output.starts_with("Failed:")
    {
        format!("Error: {}", result.output)
    } else {
        result.output.clone()
    };
    let _ = app.replace_latest_tool_display_message(
        result.tool_call.id.as_str(),
        result.title.clone(),
        display_output,
    );

    app.add_provider_message(Message::tool_result_with_duration(
        &result.tool_call.id,
        &result.output,
        result.is_error,
        Some(result.duration_ms),
    ));
    app.session.add_message_with_duration(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: result.tool_call.id.clone(),
            content: result.output.clone(),
            is_error: if result.is_error { Some(true) } else { None },
        }],
        Some(result.duration_ms),
    );
    if let Err(e) = app.session.save() {
        crate::logging::warn(&format!("Failed to save session: {}", e));
    }

    if result.tool_call.name == "subagent" {
        app.subagent_status = None;
        app.set_status_notice(if result.is_error {
            "Subagent failed"
        } else {
            "Subagent completed"
        });
    }
}

fn apply_terminal_event(
    app: &mut App,
    _terminal: &mut DefaultTerminal,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<bool> {
    match event {
        Some(Ok(Event::FocusGained)) => {
            app.note_client_focus(true);
            Ok(false)
        }
        Some(Ok(Event::Key(key))) => {
            app.note_client_interaction();
            app.update_copy_badge_key_event(key);
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                app.handle_key_press_event(key)?;
            }
            Ok(true)
        }
        Some(Ok(Event::Paste(text))) => {
            app.note_client_interaction();
            app.handle_paste(text);
            Ok(true)
        }
        Some(Ok(Event::Mouse(mouse))) => {
            app.note_client_interaction();
            app.handle_mouse_event(mouse);
            Ok(true)
        }
        Some(Ok(Event::Resize(_, _))) => Ok(app.should_redraw_after_resize()),
        _ => Ok(false),
    }
}

fn handle_background_task_completed(app: &mut App, task: BackgroundTaskCompleted) {
    if !task.notify || task.session_id != app.session.id {
        return;
    }

    let notification = format_background_task_notification_markdown(&task);
    app.push_display_message(DisplayMessage::background_task(notification.clone()));
    app.set_status_notice(background_task_status_notice(&task));

    if !app.is_processing {
        app.add_provider_message(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: notification,
                cache_control: None,
            }],
            timestamp: Some(chrono::Utc::now()),
            tool_duration_ms: None,
        });
        app.session.add_message_with_display_role(
            Role::User,
            vec![ContentBlock::Text {
                text: format_background_task_notification_markdown(&task),
                cache_control: None,
            }],
            Some(StoredDisplayRole::BackgroundTask),
        );
        if let Err(e) = app.session.save() {
            crate::logging::warn(&format!("Failed to save session: {}", e));
        }

        if task.wake {
            app.pending_turn = true;
            app.is_processing = true;
            app.status = ProcessingStatus::Sending;
            if app.processing_started.is_none() {
                app.processing_started = Some(std::time::Instant::now());
            }
            app.visible_turn_started = Some(std::time::Instant::now());
        }
    }
}

fn handle_background_task_progress(app: &mut App, event: BackgroundTaskProgressEvent) {
    if event.session_id != app.session.id {
        return;
    }

    app.upsert_background_task_progress_message(format_background_task_progress_markdown(&event));

    let notice = format!(
        "Background task · {} · {}",
        crate::message::background_task_display_label(
            &event.tool_name,
            event.display_name.as_deref()
        ),
        crate::background::format_progress_summary(&event.progress)
    );
    maybe_set_background_progress_notice(app, notice);
}

fn maybe_set_background_progress_notice(app: &mut App, notice: String) {
    let now = Instant::now();
    if let Some((current, at)) = app.status_notice.as_ref() {
        let age = now.saturating_duration_since(*at);
        if current == &notice && age < BACKGROUND_PROGRESS_IDENTICAL_NOTICE_TTL {
            return;
        }
        if current.starts_with("Background task ·") && age < BACKGROUND_PROGRESS_NOTICE_MIN_INTERVAL
        {
            return;
        }
    }

    app.set_status_notice(notice);
}

fn handle_input_shell_completed(app: &mut App, shell: InputShellCompleted) {
    if shell.session_id != app.session.id {
        return;
    }

    app.push_display_message(DisplayMessage::system(
        crate::message::format_input_shell_result_markdown(&shell.result),
    ));
    app.set_status_notice(crate::message::input_shell_status_notice(&shell.result));
}

pub(super) fn finish_turn(app: &mut App) {
    app.total_input_tokens += app.streaming_input_tokens;
    app.total_output_tokens += app.streaming_output_tokens;
    app.update_cost_impl();
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.stream_message_ended = false;
    app.processing_started = None;
    app.interleave_message = None;
    app.pending_soft_interrupts.clear();
    app.pending_soft_interrupt_requests.clear();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.note_runtime_memory_event_force("turn_completed", "local_turn_finished");
    if !app.schedule_auto_poke_followup_if_needed()
        && !app.schedule_overnight_poke_followup_if_needed()
    {
        app.clear_visible_turn_started();
    }
    let _ = super::commands::maybe_begin_pending_local_transfer(app);
}
