use super::{App, DisplayMessage, ProcessingStatus, is_context_limit_error};
use crate::bus::{BackgroundTaskCompleted, BusEvent, InputShellCompleted, ManualToolCompleted};
use crate::message::{
    ContentBlock, Message, Role, background_task_status_notice,
    format_background_task_notification_markdown,
};
use crate::session::StoredDisplayRole;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use ratatui::DefaultTerminal;
use tokio::sync::broadcast::error::RecvError;

pub(super) async fn process_turn_with_input(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event_stream: &mut EventStream,
) {
    match app.run_turn_interactive(terminal, event_stream).await {
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

pub(super) fn handle_tick(app: &mut App) {
    if let Some(chunk) = app.stream_buffer.flush() {
        app.streaming_text.push_str(&chunk);
    }
    app.refresh_side_panel_linked_content_if_due();
    app.poll_compaction_completion();
    app.maybe_progress_provider_failover_countdown();
    app.check_debug_command();
    app.check_stable_version();
    app.maybe_finish_background_client_reload();
    if app.pending_migration.is_some() && !app.is_processing {
        app.execute_migration();
    }
    if let Some(reset_time) = app.rate_limit_reset {
        if std::time::Instant::now() >= reset_time {
            app.rate_limit_reset = None;
            let queued_count = app.queued_messages.len();
            let msg = if queued_count > 0 {
                format!("✓ Rate limit reset. Retrying... (+{} queued)", queued_count)
            } else {
                "✓ Rate limit reset. Retrying...".to_string()
            };
            app.push_display_message(DisplayMessage::system(msg));
            app.pending_turn = true;
        }
    }
}

pub(super) fn handle_terminal_event(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<()> {
    apply_terminal_event(app, terminal, event)?;
    while crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        if let Ok(event) = crossterm::event::read() {
            apply_terminal_event(app, terminal, Some(Ok(event)))?;
        }
    }
    // The main run loop already draws once at the top of the next iteration.
    // Avoid drawing again here, otherwise normal key input pays for two full
    // renders back-to-back (event handler + loop-top draw).
    Ok(())
}

pub(super) fn handle_bus_event(app: &mut App, bus_event: std::result::Result<BusEvent, RecvError>) {
    match bus_event {
        Ok(BusEvent::BackgroundTaskCompleted(task)) => {
            handle_background_task_completed(app, task);
        }
        Ok(BusEvent::InputShellCompleted(shell)) => {
            handle_input_shell_completed(app, shell);
        }
        Ok(BusEvent::UsageReport(results)) => {
            app.handle_usage_report(results);
        }
        Ok(BusEvent::LoginCompleted(login)) => {
            app.handle_login_completed(login);
        }
        Ok(BusEvent::UpdateStatus(status)) => {
            app.handle_update_status(status);
        }
        Ok(BusEvent::SessionUpdateStatus(status)) => {
            app.handle_session_update_status(status);
        }
        Ok(BusEvent::DictationCompleted { text, mode }) => {
            app.handle_local_dictation_completed(text, mode);
        }
        Ok(BusEvent::DictationFailed { message }) => {
            app.handle_dictation_failure(message);
        }
        Ok(BusEvent::CompactionFinished) => {
            app.poll_compaction_completion();
        }
        Ok(BusEvent::SidePanelUpdated(update)) => {
            if update.session_id == app.session.id {
                app.set_side_panel_snapshot(update.snapshot);
            }
        }
        Ok(BusEvent::ManualToolCompleted(result)) => {
            handle_manual_tool_completed(app, result);
        }
        _ => {}
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
    let _ = app.session.save();

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
            app.note_client_focus();
            Ok(false)
        }
        Some(Ok(Event::Key(key))) => {
            app.note_client_focus();
            app.update_copy_badge_key_event(key);
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                app.handle_key_press_event(key)?;
            }
            Ok(true)
        }
        Some(Ok(Event::Paste(text))) => {
            app.note_client_focus();
            app.handle_paste(text);
            Ok(true)
        }
        Some(Ok(Event::Mouse(mouse))) => {
            app.note_client_focus();
            app.handle_mouse_event(mouse);
            Ok(true)
        }
        Some(Ok(Event::Resize(_, _))) => Ok(true),
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
        let _ = app.session.save();

        if task.wake {
            app.pending_turn = true;
            app.is_processing = true;
            app.status = ProcessingStatus::Sending;
            if app.processing_started.is_none() {
                app.processing_started = Some(std::time::Instant::now());
            }
        }
    }
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
    app.processing_started = None;
    app.interleave_message = None;
    app.pending_soft_interrupts.clear();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
}
