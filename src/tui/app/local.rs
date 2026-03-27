use super::{App, DisplayMessage, ProcessingStatus, is_context_limit_error};
use crate::bus::{
    BackgroundTaskCompleted, BackgroundTaskStatus, BusEvent, InputShellCompleted,
    ManualToolCompleted,
};
use crate::message::{ContentBlock, Message, Role};
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
    app.check_debug_command();
    app.check_stable_version();
    app.maybe_finish_background_update_reload();
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

    if let Some(dm) = app.display_messages.iter_mut().rev().find(|dm| {
        dm.tool_data.as_ref().map(|td| td.id.as_str()) == Some(result.tool_call.id.as_str())
    }) {
        dm.content = result.output.clone();
        dm.title = result.title.clone();
    }
    app.bump_display_messages_version();

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

    let notification = format_background_task_notification(&task);
    app.push_display_message(DisplayMessage::system(notification.clone()));

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
                text: format_background_task_notification(&task),
                cache_control: None,
            }],
            Some(StoredDisplayRole::System),
        );
        let _ = app.session.save();
    }
}

fn format_background_task_notification(task: &BackgroundTaskCompleted) -> String {
    let status_str = match task.status {
        BackgroundTaskStatus::Completed => "✓ completed",
        BackgroundTaskStatus::Failed => "✗ failed",
        BackgroundTaskStatus::Running => "running",
    };
    format!(
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
        task.exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "N/A".to_string()),
        task.output_preview,
        task.task_id,
    )
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
