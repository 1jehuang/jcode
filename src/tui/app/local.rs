use super::{is_context_limit_error, App, DisplayMessage, ProcessingStatus};
use crate::bus::{BackgroundTaskCompleted, BackgroundTaskStatus, BusEvent};
use crate::message::{ContentBlock, Message, Role};
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

    app.process_queued_messages(terminal, event_stream).await;
    finish_turn(app);
}

pub(super) fn handle_tick(app: &mut App) {
    if let Some(chunk) = app.stream_buffer.flush() {
        app.streaming_text.push_str(&chunk);
    }
    app.poll_compaction_completion();
    app.check_debug_command();
    app.check_stable_version();
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
    let mut needs_redraw = apply_terminal_event(app, terminal, event)?;
    while crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        if let Ok(event) = crossterm::event::read() {
            needs_redraw |= apply_terminal_event(app, terminal, Some(Ok(event)))?;
        }
    }
    if needs_redraw {
        terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
    }
    Ok(())
}

pub(super) fn handle_bus_event(app: &mut App, bus_event: std::result::Result<BusEvent, RecvError>) {
    match bus_event {
        Ok(BusEvent::BackgroundTaskCompleted(task)) => {
            handle_background_task_completed(app, task);
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
        Ok(BusEvent::CompactionFinished) => {
            app.poll_compaction_completion();
        }
        _ => {}
    }
}

fn apply_terminal_event(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<bool> {
    match event {
        Some(Ok(Event::Key(key))) => {
            app.update_copy_badge_key_event(key);
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                app.handle_key(key.code, key.modifiers)?;
            }
            Ok(true)
        }
        Some(Ok(Event::Paste(text))) => {
            app.handle_paste(text);
            Ok(true)
        }
        Some(Ok(Event::Mouse(mouse))) => {
            app.handle_mouse_event(mouse);
            Ok(true)
        }
        Some(Ok(Event::Resize(_, _))) => {
            let _ = terminal.clear();
            Ok(true)
        }
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
        });
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("[Background task {} completed]", task.task_id),
                cache_control: None,
            }],
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

fn finish_turn(app: &mut App) {
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
